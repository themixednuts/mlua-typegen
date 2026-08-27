use mlua_typegen::{LuaType, make_union};
use rustc_hir as hir;
use rustc_hir::def_id::LocalDefId;
use rustc_middle::ty::TyCtxt;

use super::emit::extract_string_literal_from_expr;
use super::table::expr_refers_to_binding;
use super::{find_in_recursive_expr_children, infer_value_expr_lua_type};

pub(super) fn extract_string_literal(expr: &hir::Expr<'_>) -> Option<String> {
    if let hir::ExprKind::Lit(lit) = &expr.kind
        && let rustc_ast::ast::LitKind::Str(sym, _) = &lit.node
    {
        return Some(sym.as_str().to_string());
    }
    None
}

pub(super) fn peel_expr<'tcx>(mut expr: &'tcx hir::Expr<'tcx>) -> &'tcx hir::Expr<'tcx> {
    loop {
        match &expr.kind {
            hir::ExprKind::DropTemps(inner)
            | hir::ExprKind::Use(inner, _)
            | hir::ExprKind::Cast(inner, _)
            | hir::ExprKind::Type(inner, _)
            | hir::ExprKind::AddrOf(_, _, inner)
            | hir::ExprKind::Unary(_, inner) => expr = inner,
            _ => return expr,
        }
    }
}

pub(super) fn peel_try_expr<'tcx>(mut expr: &'tcx hir::Expr<'tcx>) -> &'tcx hir::Expr<'tcx> {
    loop {
        let peeled = peel_expr(expr);
        let unwrapped = unwrap_try_expr(peeled);
        if std::ptr::eq(unwrapped, expr) {
            return expr;
        }
        expr = unwrapped;
    }
}

/// Check whether an expression is the HIR desugaring of `?`.
pub(super) fn is_try_desugar_expr(expr: &hir::Expr<'_>) -> bool {
    let expr = peel_expr(expr);
    matches!(
        &expr.kind,
        hir::ExprKind::Match(_, _, hir::MatchSource::TryDesugar(_))
    ) || matches!(&expr.kind, hir::ExprKind::Call(_, args) if args.first().is_some_and(|arg|
        matches!(&arg.kind, hir::ExprKind::Match(_, _, hir::MatchSource::TryDesugar(_)))))
}

/// Project the success expression out of a `?` desugaring.
pub(super) fn unwrap_try_expr<'tcx>(expr: &'tcx hir::Expr<'tcx>) -> &'tcx hir::Expr<'tcx> {
    if let hir::ExprKind::Match(scrutinee, _, hir::MatchSource::TryDesugar(_)) = &expr.kind {
        return unwrap_try_expr(scrutinee);
    }
    if let hir::ExprKind::Call(callee, args) = &expr.kind {
        if let Some(first) = args.first()
            && let hir::ExprKind::Match(scrutinee, _, hir::MatchSource::TryDesugar(_)) = &first.kind
        {
            return unwrap_try_expr(scrutinee);
        }
        if args.len() == 1
            && path_tail_name(callee).is_some_and(|name| {
                matches!(name.as_str(), "branch" | "from_residual" | "from_output")
            })
        {
            return unwrap_try_expr(&args[0]);
        }
    }
    expr
}

/// Resolve a function receiver to a `table.get("name")` call, returning the full
/// `module.function_name` path for cross-crate lookup.
/// Traces through local variable bindings: `let func: Function = table.get("name")?`
pub(super) fn resolve_table_get_function_name(
    tcx: TyCtxt<'_>,
    expr: &hir::Expr<'_>,
) -> Option<String> {
    // Direct: table.get("name")
    if let Some(name) = extract_table_get_name(tcx, expr) {
        return Some(name);
    }
    // Local variable: resolve to initializer
    if let hir::ExprKind::Path(hir::QPath::Resolved(_, path)) = &expr.kind
        && let rustc_hir::def::Res::Local(hir_id) = path.res
        && let rustc_hir::Node::Pat(pat) = tcx.hir_node(hir_id)
        && let rustc_hir::Node::LetStmt(local) = tcx.hir_node(tcx.parent_hir_id(pat.hir_id))
        && let Some(init) = local.init
    {
        let init = peel_try_expr(init);
        return extract_table_get_name(tcx, init);
    }
    None
}

/// Extract the function name from `table.get("name")` method calls.
/// Returns `module_name.function_name` if the receiver is itself a `get("module")` on a module.
pub(super) fn extract_table_get_name(tcx: TyCtxt<'_>, expr: &hir::Expr<'_>) -> Option<String> {
    let hir::ExprKind::MethodCall(segment, receiver, args, _) = &expr.kind else {
        return None;
    };
    if segment.ident.name.as_str() != "get" || args.is_empty() {
        return None;
    }
    let func_name = extract_string_literal_from_expr(tcx, &args[0])?;

    // Try to get module name from receiver: module_table.get("func_name")
    // where module_table = parent.get("module_name")
    let module_name = resolve_table_get_module_name(tcx, receiver);
    if let Some(module) = module_name {
        Some(format!("{module}.{func_name}"))
    } else {
        Some(func_name)
    }
}

/// Resolve the module name from a table receiver: `wezterm_mod.get("gui")` → "gui"
pub(super) fn resolve_table_get_module_name(
    tcx: TyCtxt<'_>,
    expr: &hir::Expr<'_>,
) -> Option<String> {
    // Direct table.get("module_name")
    if let hir::ExprKind::MethodCall(segment, _, args, _) = &expr.kind
        && segment.ident.name.as_str() == "get"
        && !args.is_empty()
    {
        return extract_string_literal_from_expr(tcx, &args[0]);
    }
    // Local variable
    if let hir::ExprKind::Path(hir::QPath::Resolved(_, path)) = &expr.kind
        && let rustc_hir::def::Res::Local(hir_id) = path.res
        && let rustc_hir::Node::Pat(pat) = tcx.hir_node(hir_id)
        && let rustc_hir::Node::LetStmt(local) = tcx.hir_node(tcx.parent_hir_id(pat.hir_id))
        && let Some(init) = local.init
    {
        let init = peel_try_expr(init);
        return resolve_table_get_module_name(tcx, init);
    }
    None
}

/// Extract a cross-module function call from a closure snippet.
/// Walk HIR to find cross-module function calls: `table.get("name")` → `func.call_async()`.
/// Handles async closures by walking through Closure and Block desugaring.
pub(super) fn find_cross_module_call_in_hir<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<String> {
    let expr = peel_expr(expr);
    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, _args, _) => {
            let method = segment.ident.name.as_str();
            if matches!(method, "call" | "call_async") {
                return resolve_table_get_function_name(tcx, receiver);
            }
            find_cross_module_call_in_hir(tcx, receiver)
        }
        hir::ExprKind::Block(block, _) => {
            if let Some(tail) = block.expr
                && let Some(name) = find_cross_module_call_in_hir(tcx, tail)
            {
                return Some(name);
            }
            for stmt in block.stmts.iter().rev() {
                match &stmt.kind {
                    hir::StmtKind::Expr(e) | hir::StmtKind::Semi(e) => {
                        if let Some(name) = find_cross_module_call_in_hir(tcx, e) {
                            return Some(name);
                        }
                    }
                    hir::StmtKind::Let(local) => {
                        if let Some(init) = local.init
                            && let Some(name) = find_cross_module_call_in_hir(tcx, init)
                        {
                            return Some(name);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            find_cross_module_call_in_hir(tcx, body.value)
        }
        hir::ExprKind::Match(scrutinee, arms, _) => find_cross_module_call_in_hir(tcx, scrutinee)
            .or_else(|| {
                arms.iter()
                    .find_map(|arm| find_cross_module_call_in_hir(tcx, arm.body))
            }),
        hir::ExprKind::Call(callee, args) => {
            find_cross_module_call_in_hir(tcx, callee).or_else(|| {
                args.iter()
                    .find_map(|a| find_cross_module_call_in_hir(tcx, a))
            })
        }
        hir::ExprKind::DropTemps(inner) | hir::ExprKind::AddrOf(_, _, inner) => {
            find_cross_module_call_in_hir(tcx, inner)
        }
        _ => None,
    }
}

/// Peel `.to_dynamic()` method calls to get the original typed receiver.
pub(super) fn peel_to_dynamic<'tcx>(expr: &'tcx hir::Expr<'tcx>) -> &'tcx hir::Expr<'tcx> {
    if let hir::ExprKind::MethodCall(segment, receiver, _, _) = &expr.kind
        && segment.ident.name.as_str() == "to_dynamic"
    {
        return receiver;
    }
    expr
}

pub(super) fn peel_lua_conversion_expr<'tcx>(
    mut expr: &'tcx hir::Expr<'tcx>,
) -> &'tcx hir::Expr<'tcx> {
    loop {
        expr = peel_try_expr(expr);
        match &expr.kind {
            hir::ExprKind::MethodCall(segment, receiver, _, _)
                if matches!(segment.ident.name.as_str(), "into_lua" | "into_lua_multi") =>
            {
                expr = receiver;
            }
            _ => return expr,
        }
    }
}

pub(super) fn path_tail_name(expr: &hir::Expr<'_>) -> Option<String> {
    let expr = peel_expr(expr);
    match &expr.kind {
        hir::ExprKind::Path(hir::QPath::Resolved(_, path)) => path
            .segments
            .last()
            .map(|seg| seg.ident.name.as_str().to_string()),
        hir::ExprKind::Path(hir::QPath::TypeRelative(_, seg)) => {
            Some(seg.ident.name.as_str().to_string())
        }
        _ => None,
    }
}

pub(super) fn expr_def_id<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<rustc_hir::def_id::DefId> {
    let expr = peel_expr(expr);
    let hir::ExprKind::Path(qpath) = &expr.kind else {
        return None;
    };

    match qpath {
        hir::QPath::Resolved(_, path) => path.res.opt_def_id(),
        hir::QPath::TypeRelative(_, _) => {
            let typeck = tcx.typeck(expr.hir_id.owner.def_id);
            match typeck.qpath_res(qpath, expr.hir_id) {
                rustc_hir::def::Res::Def(_, def_id) => Some(def_id),
                _ => None,
            }
        }
    }
}

pub(super) fn owner_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    owner: LocalDefId,
) -> Option<&'tcx hir::Body<'tcx>> {
    match tcx.hir_node_by_def_id(owner) {
        rustc_hir::Node::Item(item) => {
            let hir::ItemKind::Fn { body, .. } = item.kind else {
                return None;
            };
            Some(tcx.hir_body(body))
        }
        rustc_hir::Node::ImplItem(item) => {
            let hir::ImplItemKind::Fn(_, body) = item.kind else {
                return None;
            };
            Some(tcx.hir_body(body))
        }
        rustc_hir::Node::Expr(expr) => {
            let hir::ExprKind::Closure(closure) = expr.kind else {
                return None;
            };
            Some(tcx.hir_body(closure.body))
        }
        _ => None,
    }
}

pub(super) fn find_local_binding_init<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<&'tcx hir::Expr<'tcx>> {
    let expr = peel_expr(expr);
    let hir::ExprKind::Path(qpath) = &expr.kind else {
        return None;
    };

    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    let rustc_hir::def::Res::Local(target) = typeck.qpath_res(qpath, expr.hir_id) else {
        return None;
    };

    let body = owner_body(tcx, expr.hir_id.owner.def_id)?;
    find_binding_init_in_expr(body.value, target)
}

pub(super) fn local_binding_info<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<(hir::HirId, &'tcx hir::Body<'tcx>, String)> {
    let expr = peel_expr(expr);
    let hir::ExprKind::Path(qpath) = &expr.kind else {
        return None;
    };

    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    let rustc_hir::def::Res::Local(target) = typeck.qpath_res(qpath, expr.hir_id) else {
        return None;
    };

    let body = owner_body(tcx, expr.hir_id.owner.def_id)?;
    Some((target, body, path_tail_name(expr)?))
}

pub(super) fn infer_local_table_binding_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let (target, body, binding_name) = local_binding_info(tcx, expr)?;
    let init = find_binding_init_in_expr(body.value, target)?;
    let init = peel_lua_conversion_expr(init);
    let hir::ExprKind::MethodCall(segment, _receiver, _args, _) = &init.kind else {
        return None;
    };

    if !matches!(
        segment.ident.name.as_str(),
        "create_table" | "create_table_with_capacity"
    ) {
        return None;
    }

    let mut value_types = collect_table_binding_value_types(tcx, body.value, &binding_name);
    if value_types.is_empty() {
        return Some(LuaType::Table);
    }

    Some(LuaType::Map(
        Box::new(LuaType::String),
        Box::new(make_union(std::mem::take(&mut value_types))),
    ))
}

pub(super) fn collect_table_binding_value_types<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    binding_name: &str,
) -> Vec<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            let mut tys = Vec::new();
            if matches!(segment.ident.name.as_str(), "set" | "raw_set")
                && args.len() >= 2
                && expr_refers_to_binding(receiver, binding_name)
                && extract_string_literal(&args[0]).is_some()
            {
                tys.push(infer_value_expr_lua_type(tcx, &args[1]));
            }
            tys.extend(collect_table_binding_value_types(
                tcx,
                receiver,
                binding_name,
            ));
            for arg in *args {
                tys.extend(collect_table_binding_value_types(tcx, arg, binding_name));
            }
            tys
        }
        hir::ExprKind::Call(callee, args) => {
            let mut tys = collect_table_binding_value_types(tcx, callee, binding_name);
            for arg in *args {
                tys.extend(collect_table_binding_value_types(tcx, arg, binding_name));
            }
            tys
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .flat_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local
                    .init
                    .map(|init| collect_table_binding_value_types(tcx, init, binding_name))
                    .unwrap_or_default(),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    collect_table_binding_value_types(tcx, expr, binding_name)
                }
                _ => Vec::new(),
            })
            .chain(
                block
                    .expr
                    .into_iter()
                    .flat_map(|expr| collect_table_binding_value_types(tcx, expr, binding_name)),
            )
            .collect(),
        hir::ExprKind::Match(scrutinee, arms, _) => {
            let mut tys = collect_table_binding_value_types(tcx, scrutinee, binding_name);
            for arm in *arms {
                tys.extend(collect_table_binding_value_types(
                    tcx,
                    arm.body,
                    binding_name,
                ));
            }
            tys
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            let mut tys = collect_table_binding_value_types(tcx, cond, binding_name);
            tys.extend(collect_table_binding_value_types(
                tcx,
                then_expr,
                binding_name,
            ));
            if let Some(else_expr) = else_expr {
                tys.extend(collect_table_binding_value_types(
                    tcx,
                    else_expr,
                    binding_name,
                ));
            }
            tys
        }
        hir::ExprKind::Assign(lhs, rhs, _) | hir::ExprKind::AssignOp(_, lhs, rhs) => {
            let mut tys = collect_table_binding_value_types(tcx, lhs, binding_name);
            tys.extend(collect_table_binding_value_types(tcx, rhs, binding_name));
            tys
        }
        hir::ExprKind::Let(let_expr) => {
            collect_table_binding_value_types(tcx, let_expr.init, binding_name)
        }
        hir::ExprKind::Array(items) | hir::ExprKind::Tup(items) => items
            .iter()
            .flat_map(|item| collect_table_binding_value_types(tcx, item, binding_name))
            .collect(),
        hir::ExprKind::Ret(Some(inner)) => {
            collect_table_binding_value_types(tcx, inner, binding_name)
        }
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            collect_table_binding_value_types(tcx, body.value, binding_name)
        }
        _ => Vec::new(),
    }
}

pub(super) fn find_binding_init_in_expr<'tcx>(
    expr: &'tcx hir::Expr<'tcx>,
    target: hir::HirId,
) -> Option<&'tcx hir::Expr<'tcx>> {
    match &expr.kind {
        hir::ExprKind::Block(block, _) => {
            for stmt in block.stmts {
                if let Some(init) = find_binding_init_in_stmt(stmt, target) {
                    return Some(init);
                }
            }
            block
                .expr
                .and_then(|tail| find_binding_init_in_expr(tail, target))
        }
        hir::ExprKind::Closure(_) => None,
        _ => {
            find_in_recursive_expr_children(expr, |child| find_binding_init_in_expr(child, target))
        }
    }
}

pub(super) fn find_binding_init_in_stmt<'tcx>(
    stmt: &'tcx hir::Stmt<'tcx>,
    target: hir::HirId,
) -> Option<&'tcx hir::Expr<'tcx>> {
    match &stmt.kind {
        hir::StmtKind::Let(hir::LetStmt {
            pat,
            init: Some(init),
            ..
        }) => {
            if pat_contains_hir_id(pat, target) {
                Some(init)
            } else {
                find_binding_init_in_expr(init, target)
            }
        }
        hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
            find_binding_init_in_expr(expr, target)
        }
        _ => None,
    }
}

pub(super) fn pat_contains_hir_id(pat: &hir::Pat<'_>, target: hir::HirId) -> bool {
    match &pat.kind {
        hir::PatKind::Binding(_, hir_id, _, _) => *hir_id == target,
        hir::PatKind::Tuple(pats, _)
        | hir::PatKind::Or(pats)
        | hir::PatKind::TupleStruct(_, pats, _) => {
            pats.iter().any(|pat| pat_contains_hir_id(pat, target))
        }
        hir::PatKind::Slice(front, middle, back) => front
            .iter()
            .chain(middle.iter().copied())
            .chain(back.iter())
            .any(|pat| pat_contains_hir_id(pat, target)),
        hir::PatKind::Struct(_, fields, _) => fields
            .iter()
            .any(|field| pat_contains_hir_id(field.pat, target)),
        hir::PatKind::Box(inner)
        | hir::PatKind::Deref(inner)
        | hir::PatKind::Guard(inner, _)
        | hir::PatKind::Ref(inner, ..) => pat_contains_hir_id(inner, target),
        _ => false,
    }
}
