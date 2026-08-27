use mlua_typegen::{LuaType, make_union};
use rustc_hir as hir;
use rustc_middle::ty::TyCtxt;

use super::super::{
    def_snippet, expr_def_id, extract_string_literal, infer_value_expr_lua_type, is_informative,
    map_ty_to_lua, normalize_inferred_lua_type, normalize_table_value_types, path_tail_name,
    peel_expr, peel_try_expr, unwrap_result_ty, unwrap_try_expr,
};
use super::{
    called_path_name, expr_refers_to_binding, infer_value_converter_types_from_hir,
    infer_value_converter_types_from_snippet, json_like_lua_type, lua_type_from_hir_ty,
    lua_type_from_hir_ty_with_snippet, method_generic_lua_type, pat_to_name,
};

pub(in crate::extract) fn infer_table_param_type_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Option<LuaType> {
    let mut value_types = collect_table_param_value_types(tcx, expr, param_name);
    value_types.extend(collect_nested_table_param_value_types(
        tcx, expr, param_name,
    ));
    value_types = normalize_table_value_types(value_types);
    if value_types.is_empty() {
        None
    } else {
        Some(normalize_inferred_lua_type(LuaType::Map(
            Box::new(LuaType::String),
            Box::new(make_union(value_types)),
        )))
    }
}

pub(in crate::extract) fn infer_table_param_type_from_converter_calls<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Call(callee, args) => {
            infer_table_converter_call_type(tcx, callee, args, param_name)
                .or_else(|| infer_table_param_type_from_converter_calls(tcx, callee, param_name))
                .or_else(|| {
                    args.iter().find_map(|arg| {
                        infer_table_param_type_from_converter_calls(tcx, arg, param_name)
                    })
                })
        }
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            infer_table_param_type_from_converter_calls(tcx, receiver, param_name).or_else(|| {
                args.iter().find_map(|arg| {
                    infer_table_param_type_from_converter_calls(tcx, arg, param_name)
                })
            })
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .find_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local.init.and_then(|init| {
                    infer_table_param_type_from_converter_calls(tcx, init, param_name)
                }),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    infer_table_param_type_from_converter_calls(tcx, expr, param_name)
                }
                _ => None,
            })
            .or_else(|| {
                block.expr.and_then(|expr| {
                    infer_table_param_type_from_converter_calls(tcx, expr, param_name)
                })
            }),
        hir::ExprKind::Match(scrutinee, arms, _) => {
            infer_table_param_type_from_converter_calls(tcx, scrutinee, param_name).or_else(|| {
                arms.iter().find_map(|arm| {
                    infer_table_param_type_from_converter_calls(tcx, arm.body, param_name)
                })
            })
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            infer_table_param_type_from_converter_calls(tcx, cond, param_name)
                .or_else(|| infer_table_param_type_from_converter_calls(tcx, then_expr, param_name))
                .or_else(|| {
                    else_expr.and_then(|expr| {
                        infer_table_param_type_from_converter_calls(tcx, expr, param_name)
                    })
                })
        }
        hir::ExprKind::Assign(lhs, rhs, _)
        | hir::ExprKind::AssignOp(_, lhs, rhs)
        | hir::ExprKind::Binary(_, lhs, rhs) => {
            infer_table_param_type_from_converter_calls(tcx, lhs, param_name)
                .or_else(|| infer_table_param_type_from_converter_calls(tcx, rhs, param_name))
        }
        hir::ExprKind::Let(let_expr) => {
            infer_table_param_type_from_converter_calls(tcx, let_expr.init, param_name)
        }
        hir::ExprKind::Array(items) | hir::ExprKind::Tup(items) => items
            .iter()
            .find_map(|item| infer_table_param_type_from_converter_calls(tcx, item, param_name)),
        hir::ExprKind::Struct(_, fields, tail) => fields
            .iter()
            .find_map(|field| {
                infer_table_param_type_from_converter_calls(tcx, field.expr, param_name)
            })
            .or_else(|| match tail {
                hir::StructTailExpr::Base(base) => {
                    infer_table_param_type_from_converter_calls(tcx, base, param_name)
                }
                _ => None,
            }),
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            infer_table_param_type_from_converter_calls(tcx, body.value, param_name)
        }
        hir::ExprKind::Ret(Some(inner)) => {
            infer_table_param_type_from_converter_calls(tcx, inner, param_name)
        }
        _ => None,
    }
}

pub(in crate::extract) fn infer_table_converter_call_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    callee: &'tcx hir::Expr<'tcx>,
    args: &'tcx [hir::Expr<'tcx>],
    param_name: &str,
) -> Option<LuaType> {
    let _arg_index = args
        .iter()
        .position(|arg| expr_refers_to_binding(arg, param_name))?;
    let callee_name = called_path_name(callee);
    let snippet = expr_def_id(tcx, callee).and_then(|def_id| def_snippet(tcx, def_id));

    if callee_name.as_deref() == Some("table_to_args") {
        let key_ty = make_union(vec![LuaType::Integer, LuaType::String]);
        // Try HIR-based inference first, fall back to snippet
        let hir_types = expr_def_id(tcx, callee)
            .and_then(|did| did.as_local())
            .and_then(|local| {
                let node = tcx.hir_node_by_def_id(local);
                if let rustc_hir::Node::Item(item) = node
                    && let hir::ItemKind::Fn { body, .. } = &item.kind
                {
                    let body = tcx.hir_body(*body);
                    let tys = infer_value_converter_types_from_hir(tcx, body.value);
                    (!tys.is_empty()).then_some(tys)
                } else {
                    None
                }
            });
        let value_ty = match hir_types.or_else(|| {
            snippet
                .as_deref()
                .map(infer_value_converter_types_from_snippet)
                .filter(|tys| !tys.is_empty())
        }) {
            Some(tys) => make_union(normalize_table_value_types(tys)),
            None => json_like_lua_type(),
        };

        return Some(LuaType::Map(Box::new(key_ty), Box::new(value_ty)));
    }

    let snippet = snippet?;

    if !snippet.contains("pairs::<Value, Value>()")
        && !snippet.contains("pairs::<mlua::Value, mlua::Value>()")
    {
        return None;
    }

    let mut key_types = Vec::new();
    if snippet.contains("Value::Integer") || snippet.contains("DataKey::Integer") {
        key_types.push(LuaType::Integer);
    }
    if snippet.contains("Value::String") || snippet.contains("DataKey::String") {
        key_types.push(LuaType::String);
    }
    if key_types.is_empty() {
        key_types.push(LuaType::String);
    }

    let mut value_types = infer_value_converter_types_from_snippet(&snippet);
    value_types = normalize_table_value_types(value_types);
    if value_types.is_empty() {
        value_types.push(LuaType::Any);
    }

    Some(normalize_inferred_lua_type(LuaType::Map(
        Box::new(make_union(key_types)),
        Box::new(make_union(value_types)),
    )))
}

pub(in crate::extract) fn is_any_userdata_type_snippet(snippet: &str) -> bool {
    let snippet = snippet.trim();
    matches!(
        snippet,
        "AnyUserData"
            | "mlua::AnyUserData"
            | "mlua::userdata::AnyUserData"
            | "mlua::prelude::LuaAnyUserData"
    )
}

pub(in crate::extract) fn collect_nested_table_param_value_types<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Vec<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Block(block, _) => {
            let mut tys = Vec::new();

            for stmt in block.stmts {
                if let hir::StmtKind::Let(local) = &stmt.kind
                    && let Some(init) = local.init
                    && let Some(_key) = raw_get_table_binding_key(init, param_name)
                    && let Some(ty) = infer_table_binding_usage_type(
                        tcx,
                        block.expr.unwrap_or(expr),
                        &pat_to_name(local.pat),
                    )
                {
                    tys.push(ty);
                }
            }

            for stmt in block.stmts {
                match &stmt.kind {
                    hir::StmtKind::Let(local) => {
                        if let Some(init) = local.init {
                            tys.extend(collect_nested_table_param_value_types(
                                tcx, init, param_name,
                            ));
                        }
                    }
                    hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                        tys.extend(collect_nested_table_param_value_types(
                            tcx, expr, param_name,
                        ));
                    }
                    _ => {}
                }
            }
            if let Some(tail) = block.expr {
                tys.extend(collect_nested_table_param_value_types(
                    tcx, tail, param_name,
                ));
            }
            tys
        }
        hir::ExprKind::Match(scrutinee, arms, _) => {
            let mut tys = collect_nested_table_param_value_types(tcx, scrutinee, param_name);
            for arm in arms.iter() {
                tys.extend(collect_nested_table_param_value_types(
                    tcx, arm.body, param_name,
                ));
            }
            tys
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            let mut tys = collect_nested_table_param_value_types(tcx, cond, param_name);
            tys.extend(collect_nested_table_param_value_types(
                tcx, then_expr, param_name,
            ));
            if let Some(else_expr) = else_expr {
                tys.extend(collect_nested_table_param_value_types(
                    tcx, else_expr, param_name,
                ));
            }
            tys
        }
        hir::ExprKind::Call(callee, args) => {
            let mut tys = collect_nested_table_param_value_types(tcx, callee, param_name);
            for arg in args.iter() {
                tys.extend(collect_nested_table_param_value_types(tcx, arg, param_name));
            }
            tys
        }
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            let mut tys = collect_nested_table_param_value_types(tcx, receiver, param_name);
            for arg in args.iter() {
                tys.extend(collect_nested_table_param_value_types(tcx, arg, param_name));
            }
            tys
        }
        hir::ExprKind::Assign(lhs, rhs, _) | hir::ExprKind::AssignOp(_, lhs, rhs) => {
            let mut tys = collect_nested_table_param_value_types(tcx, lhs, param_name);
            tys.extend(collect_nested_table_param_value_types(tcx, rhs, param_name));
            tys
        }
        hir::ExprKind::Binary(_, lhs, rhs) => {
            let mut tys = collect_nested_table_param_value_types(tcx, lhs, param_name);
            tys.extend(collect_nested_table_param_value_types(tcx, rhs, param_name));
            tys
        }
        hir::ExprKind::Let(let_expr) => {
            collect_nested_table_param_value_types(tcx, let_expr.init, param_name)
        }
        hir::ExprKind::Array(items) | hir::ExprKind::Tup(items) => items
            .iter()
            .flat_map(|item| collect_nested_table_param_value_types(tcx, item, param_name))
            .collect(),
        hir::ExprKind::Struct(_, fields, tail) => fields
            .iter()
            .flat_map(|field| collect_nested_table_param_value_types(tcx, field.expr, param_name))
            .chain(match tail {
                hir::StructTailExpr::Base(base) => {
                    collect_nested_table_param_value_types(tcx, base, param_name)
                }
                _ => Vec::new(),
            })
            .collect(),
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            collect_nested_table_param_value_types(tcx, body.value, param_name)
        }
        hir::ExprKind::Ret(Some(inner)) => {
            collect_nested_table_param_value_types(tcx, inner, param_name)
        }
        _ => Vec::new(),
    }
}

pub(in crate::extract) fn raw_get_table_binding_key<'tcx>(
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Option<String> {
    let expr = peel_try_expr(expr);
    let hir::ExprKind::MethodCall(segment, receiver, args, _) = &expr.kind else {
        return None;
    };
    if !matches!(segment.ident.name.as_str(), "get" | "raw_get")
        || !expr_refers_to_binding(receiver, param_name)
        || args.is_empty()
    {
        return None;
    }
    extract_string_literal(&args[0])
}

pub(in crate::extract) fn infer_table_binding_usage_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    binding_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, _args, _) => {
            if path_tail_name(receiver).as_deref() == Some(binding_name) {
                match segment.ident.name.as_str() {
                    "sequence_values" => {
                        if let Some(ty) = method_generic_lua_type(tcx, expr, segment) {
                            return Some(LuaType::Array(Box::new(ty)));
                        }
                    }
                    "pairs" => {
                        if let Some(args) = segment.args {
                            let generic_types: Vec<_> = args
                                .args
                                .iter()
                                .filter_map(|arg| match arg {
                                    hir::GenericArg::Type(ty) => lua_type_from_hir_ty(*ty),
                                    _ => None,
                                })
                                .collect();
                            if generic_types.len() == 2 {
                                return Some(LuaType::Map(
                                    Box::new(generic_types[0].clone()),
                                    Box::new(generic_types[1].clone()),
                                ));
                            }
                        }
                    }
                    _ => {}
                }
            }

            infer_table_binding_usage_type(tcx, receiver, binding_name)
        }
        hir::ExprKind::Call(callee, args) => {
            infer_table_binding_usage_type(tcx, callee, binding_name).or_else(|| {
                args.iter()
                    .find_map(|arg| infer_table_binding_usage_type(tcx, arg, binding_name))
            })
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .find_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local
                    .init
                    .and_then(|init| infer_table_binding_usage_type(tcx, init, binding_name)),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    infer_table_binding_usage_type(tcx, expr, binding_name)
                }
                _ => None,
            })
            .or_else(|| {
                block
                    .expr
                    .and_then(|expr| infer_table_binding_usage_type(tcx, expr, binding_name))
            }),
        hir::ExprKind::Match(scrutinee, arms, _) => {
            infer_table_binding_usage_type(tcx, scrutinee, binding_name).or_else(|| {
                arms.iter()
                    .find_map(|arm| infer_table_binding_usage_type(tcx, arm.body, binding_name))
            })
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            infer_table_binding_usage_type(tcx, cond, binding_name)
                .or_else(|| infer_table_binding_usage_type(tcx, then_expr, binding_name))
                .or_else(|| {
                    else_expr
                        .and_then(|expr| infer_table_binding_usage_type(tcx, expr, binding_name))
                })
        }
        hir::ExprKind::Assign(lhs, rhs, _) | hir::ExprKind::AssignOp(_, lhs, rhs) => {
            infer_table_binding_usage_type(tcx, lhs, binding_name)
                .or_else(|| infer_table_binding_usage_type(tcx, rhs, binding_name))
        }
        hir::ExprKind::Binary(_, lhs, rhs) => {
            infer_table_binding_usage_type(tcx, lhs, binding_name)
                .or_else(|| infer_table_binding_usage_type(tcx, rhs, binding_name))
        }
        hir::ExprKind::Let(let_expr) => {
            infer_table_binding_usage_type(tcx, let_expr.init, binding_name)
        }
        hir::ExprKind::Array(items) | hir::ExprKind::Tup(items) => items
            .iter()
            .find_map(|item| infer_table_binding_usage_type(tcx, item, binding_name)),
        hir::ExprKind::Struct(_, fields, tail) => fields
            .iter()
            .find_map(|field| infer_table_binding_usage_type(tcx, field.expr, binding_name))
            .or_else(|| match tail {
                hir::StructTailExpr::Base(base) => {
                    infer_table_binding_usage_type(tcx, base, binding_name)
                }
                _ => None,
            }),
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            infer_table_binding_usage_type(tcx, body.value, binding_name)
        }
        hir::ExprKind::Ret(Some(inner)) => infer_table_binding_usage_type(tcx, inner, binding_name),
        _ => None,
    }
}

pub(in crate::extract) fn collect_table_param_value_types<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Vec<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            let mut tys = Vec::new();

            if matches!(segment.ident.name.as_str(), "get" | "raw_get")
                && expr_refers_to_binding(receiver, param_name)
                && !args.is_empty()
                && extract_string_literal(&args[0]).is_some()
            {
                let lua_ty = method_generic_lua_type(tcx, expr, segment).unwrap_or_else(|| {
                    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
                    let ty = unwrap_result_ty(tcx, typeck.expr_ty(expr));
                    map_ty_to_lua(tcx, ty)
                });
                if is_informative(&lua_ty) {
                    tys.push(lua_ty);
                }
            }

            if matches!(segment.ident.name.as_str(), "set" | "raw_set")
                && expr_refers_to_binding(receiver, param_name)
                && args.len() >= 2
                && extract_string_literal(&args[0]).is_some()
            {
                tys.push(infer_value_expr_lua_type(tcx, &args[1]));
            }

            tys.extend(collect_table_param_value_types(tcx, receiver, param_name));
            for arg in args.iter() {
                tys.extend(collect_table_param_value_types(tcx, arg, param_name));
            }
            tys
        }
        hir::ExprKind::Call(callee, args) => {
            let mut tys = collect_table_param_value_types(tcx, callee, param_name);
            for arg in args.iter() {
                tys.extend(collect_table_param_value_types(tcx, arg, param_name));
            }
            tys
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .flat_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => {
                    let mut tys = local
                        .init
                        .map(|init| collect_table_param_value_types(tcx, init, param_name))
                        .unwrap_or_default();
                    if let Some(ty) = infer_table_param_let_binding_type(tcx, local, param_name) {
                        tys.push(ty);
                    }
                    tys
                }
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    collect_table_param_value_types(tcx, expr, param_name)
                }
                _ => Vec::new(),
            })
            .chain(
                block
                    .expr
                    .into_iter()
                    .flat_map(|expr| collect_table_param_value_types(tcx, expr, param_name)),
            )
            .collect(),
        hir::ExprKind::Match(scrutinee, arms, _) => {
            let mut tys = collect_table_param_value_types(tcx, scrutinee, param_name);
            for arm in arms.iter() {
                tys.extend(collect_table_param_value_types(tcx, arm.body, param_name));
            }
            tys
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            let mut tys = collect_table_param_value_types(tcx, cond, param_name);
            tys.extend(collect_table_param_value_types(tcx, then_expr, param_name));
            if let Some(else_expr) = else_expr {
                tys.extend(collect_table_param_value_types(tcx, else_expr, param_name));
            }
            tys
        }
        hir::ExprKind::Assign(lhs, rhs, _) | hir::ExprKind::AssignOp(_, lhs, rhs) => {
            let mut tys = collect_table_param_value_types(tcx, lhs, param_name);
            tys.extend(collect_table_param_value_types(tcx, rhs, param_name));
            tys
        }
        hir::ExprKind::Binary(_, lhs, rhs) => {
            let mut tys = collect_table_param_value_types(tcx, lhs, param_name);
            tys.extend(collect_table_param_value_types(tcx, rhs, param_name));
            tys
        }
        hir::ExprKind::Let(let_expr) => {
            collect_table_param_value_types(tcx, let_expr.init, param_name)
        }
        hir::ExprKind::Struct(_, fields, tail) => fields
            .iter()
            .flat_map(|field| collect_table_param_value_types(tcx, field.expr, param_name))
            .chain(match tail {
                hir::StructTailExpr::Base(base) => {
                    collect_table_param_value_types(tcx, base, param_name)
                }
                _ => Vec::new(),
            })
            .collect(),
        hir::ExprKind::Array(items) | hir::ExprKind::Tup(items) => items
            .iter()
            .flat_map(|item| collect_table_param_value_types(tcx, item, param_name))
            .collect(),
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            collect_table_param_value_types(tcx, body.value, param_name)
        }
        hir::ExprKind::Ret(Some(inner)) => collect_table_param_value_types(tcx, inner, param_name),
        _ => Vec::new(),
    }
}

pub(in crate::extract) fn infer_table_param_let_binding_type(
    tcx: TyCtxt<'_>,
    local: &hir::LetStmt<'_>,
    param_name: &str,
) -> Option<LuaType> {
    let init = peel_expr(unwrap_try_expr(local.init?));
    let hir::ExprKind::MethodCall(segment, receiver, args, _) = &init.kind else {
        return None;
    };

    if !matches!(segment.ident.name.as_str(), "get" | "raw_get")
        || !expr_refers_to_binding(receiver, param_name)
        || args.is_empty()
        || extract_string_literal(&args[0]).is_none()
    {
        return None;
    }

    local
        .ty
        .and_then(|ty| lua_type_from_hir_ty_with_snippet(tcx, ty))
}
