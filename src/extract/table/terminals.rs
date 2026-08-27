use mlua_typegen::LuaType;
use rustc_hir as hir;
use rustc_middle::ty::TyCtxt;

use super::super::{
    infer_wrapper_method_lua_type, is_any_user_data, is_informative, peel_lua_conversion_expr,
    peel_try_expr, unwrap_result_ty,
};

/// Get a name from a simple binding pattern, falling back to `_`.
pub(in crate::extract) fn pat_to_name(pat: &hir::Pat<'_>) -> String {
    match &pat.kind {
        hir::PatKind::Binding(_, _, ident, _) => {
            let name = ident.name.as_str();
            // Strip leading underscores (Rust unused-variable convention)
            let stripped = name.trim_start_matches('_');
            if stripped.is_empty() {
                name.to_string()
            } else {
                stripped.to_string()
            }
        }
        _ => "_".to_string(),
    }
}

pub(in crate::extract) fn terminal_return_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<&'tcx hir::Expr<'tcx>> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Block(block, _) => {
            block.expr.and_then(|expr| terminal_return_expr(tcx, expr))
        }
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            terminal_return_expr(tcx, body.value)
        }
        hir::ExprKind::Ret(Some(inner)) => terminal_return_expr(tcx, inner),
        hir::ExprKind::Call(callee, args) => {
            if let hir::ExprKind::Path(qpath) = &callee.kind {
                let name = match qpath {
                    hir::QPath::Resolved(_, path) => {
                        path.segments.last().map(|seg| seg.ident.name.as_str())
                    }
                    hir::QPath::TypeRelative(_, seg) => Some(seg.ident.name.as_str()),
                };
                if matches!(name, Some("Ok" | "Some")) && args.len() == 1 {
                    return terminal_return_expr(tcx, &args[0]);
                }
            }
            Some(expr)
        }
        hir::ExprKind::Match(_, arms, _) => arms
            .first()
            .and_then(|arm| terminal_return_expr(tcx, arm.body)),
        hir::ExprKind::If(_, then_expr, else_expr) => terminal_return_expr(tcx, then_expr)
            .or_else(|| else_expr.and_then(|expr| terminal_return_expr(tcx, expr))),
        _ => Some(expr),
    }
}

pub(in crate::extract) fn terminal_return_is_opaque_any_userdata<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> bool {
    let Some(expr) = terminal_return_expr(tcx, expr) else {
        return false;
    };

    if infer_wrapper_method_lua_type(tcx, peel_lua_conversion_expr(expr))
        .is_some_and(|ty| is_informative(&ty) && !matches!(ty, LuaType::Any))
    {
        return false;
    }

    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    let ty = unwrap_result_ty(tcx, typeck.expr_ty(expr));
    is_any_user_data(tcx, ty)
}

pub(in crate::extract) fn is_method_registration_name(name: &str) -> bool {
    matches!(
        name,
        "add_method"
            | "add_method_mut"
            | "add_method_once"
            | "add_async_method"
            | "add_async_method_mut"
            | "add_async_method_once"
            | "add_function"
            | "add_function_mut"
            | "add_async_function"
    )
}

pub(in crate::extract) fn body_returns_named_userdata_self(
    expr: &hir::Expr<'_>,
    self_name: &str,
) -> bool {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Block(block, _) => block
            .expr
            .is_some_and(|expr| body_returns_named_userdata_self(expr, self_name)),
        hir::ExprKind::Ret(Some(inner)) => body_returns_named_userdata_self(inner, self_name),
        hir::ExprKind::Call(callee, args) => {
            if let hir::ExprKind::Path(qpath) = &callee.kind {
                let name = match qpath {
                    hir::QPath::Resolved(_, path) => {
                        path.segments.last().map(|seg| seg.ident.name.as_str())
                    }
                    hir::QPath::TypeRelative(_, seg) => Some(seg.ident.name.as_str()),
                };
                if matches!(name, Some("Ok" | "Some")) && args.len() == 1 {
                    return body_returns_named_userdata_self(&args[0], self_name);
                }
            }
            false
        }
        hir::ExprKind::MethodCall(segment, receiver, _, _) => {
            matches!(
                segment.ident.name.as_str(),
                "into_lua" | "into" | "clone" | "to_owned" | "borrow" | "as_ref"
            ) && body_returns_named_userdata_self(receiver, self_name)
        }
        hir::ExprKind::Match(_, arms, _) => arms
            .iter()
            .any(|arm| body_returns_named_userdata_self(arm.body, self_name)),
        hir::ExprKind::If(_, then_expr, else_expr) => {
            body_returns_named_userdata_self(then_expr, self_name)
                || else_expr.is_some_and(|expr| body_returns_named_userdata_self(expr, self_name))
        }
        hir::ExprKind::Path(hir::QPath::Resolved(_, path)) => path
            .segments
            .last()
            .is_some_and(|seg| seg.ident.name.as_str() == self_name),
        _ => false,
    }
}
