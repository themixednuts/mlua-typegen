use mlua_typegen::{LuaReturn, LuaType, make_union};
use rustc_hir as hir;
use rustc_middle::ty::{self, TyCtxt};

use super::super::{
    infer_from_expr, infer_value_expr_lua_type, infer_wrapper_method_lua_type, is_informative,
    is_try_desugar_expr, map_ty_to_lua, peel_expr, peel_lua_conversion_expr, peel_to_dynamic,
    peel_try_expr, resolve_table_get_function_name,
};
use super::concrete::infer_forwarded_multivalue_returns;
use super::names::extract_tuple_element_names;

pub(in crate::extract) fn infer_returns_from_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<Vec<LuaReturn>> {
    let expr = peel_expr(peel_try_expr(expr));

    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            match segment.ident.name.as_str() {
                "into_lua_multi" => Some(infer_into_lua_multi_returns(tcx, receiver)),
                "into_lua" => Some(vec![infer_value_expr_lua_type(tcx, receiver).into()]),
                // lua.to_value(&data) → infer type from the data argument
                "to_value" | "to_value_with" if !args.is_empty() => {
                    let typeck = tcx.typeck(args[0].hir_id.owner.def_id);
                    let ty = typeck.node_type(args[0].hir_id);
                    let lua_ty = map_ty_to_lua(tcx, ty);
                    if is_informative(&lua_ty) {
                        Some(vec![lua_ty.into()])
                    } else {
                        None
                    }
                }
                // func.call(args) / func.call_async(args) where func comes from
                // table.get("name") — emit deferred lookup marker for cross-crate resolution
                "call" | "call_async" => {
                    if let Some(func_name) = resolve_table_get_function_name(tcx, receiver) {
                        Some(vec![LuaType::Class(format!("@lookup:{func_name}")).into()])
                    } else {
                        infer_forwarded_multivalue_returns(tcx, expr, segment, args).or_else(|| {
                            infer_wrapper_method_lua_type(tcx, expr).map(|ty| vec![ty.into()])
                        })
                    }
                }
                _ => infer_forwarded_multivalue_returns(tcx, expr, segment, args)
                    .or_else(|| infer_wrapper_method_lua_type(tcx, expr).map(|ty| vec![ty.into()])),
            }
        }
        hir::ExprKind::Call(callee, args) => {
            if let hir::ExprKind::Path(qpath) = &callee.kind {
                let name = match qpath {
                    hir::QPath::Resolved(_, path) => {
                        path.segments.last().map(|seg| seg.ident.name.as_str())
                    }
                    hir::QPath::TypeRelative(_, seg) => Some(seg.ident.name.as_str()),
                };
                if matches!(name, Some("Ok" | "Some")) && args.len() == 1 {
                    return infer_returns_from_expr(tcx, &args[0]);
                }
                // to_lua / dynamic_to_lua_value / to_dynamic → infer from data arg
                if matches!(
                    name,
                    Some(
                        "to_lua" | "to_dynamic" | "from_lua_value_dynamic" | "dynamic_to_lua_value"
                    )
                ) && !args.is_empty()
                {
                    // Last arg is the data (first might be lua context)
                    let data_arg = args.last().unwrap();
                    // Peel .to_dynamic() to get the original typed value
                    let data_arg = peel_to_dynamic(data_arg);
                    let typeck = tcx.typeck(data_arg.hir_id.owner.def_id);
                    let ty = typeck.node_type(data_arg.hir_id);
                    let lua_ty = map_ty_to_lua(tcx, ty);
                    if is_informative(&lua_ty) {
                        return Some(vec![lua_ty.into()]);
                    }
                }
            }
            None
        }
        hir::ExprKind::Tup([]) => Some(Vec::new()),
        hir::ExprKind::Block(block, _) => {
            let mut branches = Vec::new();

            for stmt in block.stmts {
                match &stmt.kind {
                    hir::StmtKind::Let(local) => {
                        // Skip ? desugaring in let bindings — those are error paths
                        if let Some(init) = local.init
                            && !is_try_desugar_expr(init)
                            && expr_contains_return(init)
                            && let Some(returns) = infer_returns_from_expr(tcx, init)
                        {
                            branches.push(returns);
                        }
                    }
                    hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                        // Skip ? desugaring statements — those are error paths
                        if !is_try_desugar_expr(expr)
                            && expr_contains_return(expr)
                            && let Some(returns) = infer_returns_from_expr(tcx, expr)
                        {
                            branches.push(returns);
                        }
                    }
                    _ => {}
                }
            }

            if let Some(tail) = block
                .expr
                .and_then(|tail| infer_returns_from_expr(tcx, tail))
            {
                branches.push(tail);
            }

            merge_return_branches(branches)
        }
        hir::ExprKind::Match(scrutinee, arms, source) => {
            let mut branches = Vec::new();
            if (matches!(source, hir::MatchSource::TryDesugar(_))
                || is_result_like_expr(tcx, scrutinee))
                && let Some(returns) = infer_returns_from_expr(tcx, scrutinee)
            {
                branches.push(returns);
            }
            branches.extend(
                arms.iter()
                    .filter_map(|arm| infer_returns_from_expr(tcx, arm.body)),
            );
            merge_return_branches(branches)
        }
        hir::ExprKind::If(_, then_expr, else_expr) => {
            let mut branches = Vec::new();
            if let Some(returns) = infer_returns_from_expr(tcx, then_expr) {
                branches.push(returns);
            }
            if let Some(else_expr) = else_expr.and_then(|expr| infer_returns_from_expr(tcx, expr)) {
                branches.push(else_expr);
            }
            merge_return_branches(branches)
        }
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            infer_returns_from_expr(tcx, body.value)
        }
        hir::ExprKind::Ret(Some(expr)) => infer_returns_from_expr(tcx, expr),
        // Resolve local variable references to their initializer
        hir::ExprKind::Path(hir::QPath::Resolved(_, path))
            if matches!(path.res, rustc_hir::def::Res::Local(_)) =>
        {
            if let rustc_hir::def::Res::Local(hir_id) = path.res
                && let rustc_hir::Node::Pat(pat) = tcx.hir_node(hir_id)
                && let rustc_hir::Node::LetStmt(local) = tcx.hir_node(tcx.parent_hir_id(pat.hir_id))
                && let Some(init) = local.init
            {
                let init = peel_try_expr(init);
                infer_returns_from_expr(tcx, init)
            } else {
                None
            }
        }
        _ => {
            let typeck = tcx.typeck(expr.hir_id.owner.def_id);
            infer_from_expr(tcx, typeck, expr).map(|ty| vec![ty.into()])
        }
    }
}

pub(in crate::extract) fn expr_contains_return(expr: &hir::Expr<'_>) -> bool {
    match &expr.kind {
        hir::ExprKind::Ret(_) => true,
        hir::ExprKind::Block(block, _) => {
            block.stmts.iter().any(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local.init.is_some_and(expr_contains_return),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => expr_contains_return(expr),
                _ => false,
            }) || block.expr.is_some_and(expr_contains_return)
        }
        hir::ExprKind::Call(callee, args) => {
            expr_contains_return(callee) || args.iter().any(|arg| expr_contains_return(arg))
        }
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            expr_contains_return(receiver) || args.iter().any(|arg| expr_contains_return(arg))
        }
        hir::ExprKind::Match(scrutinee, arms, _) => {
            expr_contains_return(scrutinee) || arms.iter().any(|arm| expr_contains_return(arm.body))
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            expr_contains_return(cond)
                || expr_contains_return(then_expr)
                || else_expr.is_some_and(expr_contains_return)
        }
        hir::ExprKind::Closure(_) => false,
        _ => false,
    }
}

pub(in crate::extract) fn infer_into_lua_multi_returns<'tcx>(
    tcx: TyCtxt<'tcx>,
    receiver: &'tcx hir::Expr<'tcx>,
) -> Vec<LuaReturn> {
    let receiver = peel_lua_conversion_expr(receiver);
    let typeck = tcx.typeck(receiver.hir_id.owner.def_id);
    let receiver_ty = typeck.expr_ty(receiver);

    if let ty::TyKind::Tuple(fields) = receiver_ty.kind() {
        let names = extract_tuple_element_names(receiver);
        return fields
            .iter()
            .enumerate()
            .map(|(i, ty)| LuaReturn {
                ty: map_ty_to_lua(tcx, ty),
                name: names.get(i).and_then(|name| name.clone()),
            })
            .collect();
    }

    vec![infer_value_expr_lua_type(tcx, receiver).into()]
}

pub(in crate::extract) fn is_result_like_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> bool {
    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    let ty = typeck.expr_ty(expr);

    matches!(ty.kind(), ty::TyKind::Adt(adt_def, _) if {
        let path = tcx.def_path_str(adt_def.did());
        path == "std::result::Result" || path == "core::result::Result"
    })
}

pub(in crate::extract) fn merge_return_branches(
    branches: Vec<Vec<LuaReturn>>,
) -> Option<Vec<LuaReturn>> {
    let max_len = branches.iter().map(Vec::len).max()?;
    let mut merged = Vec::with_capacity(max_len);

    for index in 0..max_len {
        let mut tys = Vec::new();
        let mut name: Option<String> = None;
        let mut same_name = true;

        for branch in &branches {
            let item = branch.get(index);
            let ty = item.map(|ret| ret.ty.clone()).unwrap_or(LuaType::Nil);
            tys.push(ty);

            let branch_name = item.and_then(|ret| ret.name.clone());
            if name.is_none() {
                name = branch_name.clone();
            } else if name != branch_name {
                same_name = false;
            }
        }

        merged.push(LuaReturn {
            ty: make_union(tys),
            name: if same_name { name } else { None },
        });
    }

    Some(merged)
}
