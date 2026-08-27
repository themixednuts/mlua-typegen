use mlua_typegen::LuaType;
use rustc_hir as hir;
use rustc_middle::ty::{self, TyCtxt};

use super::super::{is_informative, map_return_ty, map_ty_to_lua, peel_try_expr, unwrap_result_ty};
use super::{expr_refers_to_binding, method_generic_lua_type};

/// Infer a callback's signature by finding `.call()` / `.call_async()` invocations
/// on the given param variable within the body expression.
/// Returns `LuaType::FunctionSig` if a call is found, or `None`.
pub(in crate::extract) fn infer_callback_signature_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            let method_name = segment.ident.name.as_str();
            if matches!(method_name, "call" | "call_async")
                && expr_refers_to_binding(receiver, param_name)
            {
                // Found callback.call(args) or callback.call_async(args)
                // Use typeck to get the return type of the call expression
                let typeck = tcx.typeck(expr.hir_id.owner.def_id);
                let call_ty = typeck.node_type(expr.hir_id);
                let returns = map_return_ty(tcx, call_ty);

                // Extract param types from the argument expression
                let params = if let Some(arg_expr) = args.first() {
                    infer_callback_params_from_arg(tcx, arg_expr)
                } else {
                    vec![]
                };

                return Some(LuaType::FunctionSig {
                    params,
                    returns: returns.into_iter().map(|r| r.ty).collect(),
                });
            }

            // Recurse into receiver and args
            infer_callback_signature_from_body(tcx, receiver, param_name).or_else(|| {
                args.iter()
                    .find_map(|arg| infer_callback_signature_from_body(tcx, arg, param_name))
            })
        }
        hir::ExprKind::Block(block, _) => {
            // Check statements then tail expression
            for stmt in block.stmts {
                if let hir::StmtKind::Expr(e) | hir::StmtKind::Semi(e) = stmt.kind
                    && let Some(sig) = infer_callback_signature_from_body(tcx, e, param_name)
                {
                    return Some(sig);
                }
                if let hir::StmtKind::Let(local) = stmt.kind
                    && let Some(init) = local.init
                    && let Some(sig) = infer_callback_signature_from_body(tcx, init, param_name)
                {
                    return Some(sig);
                }
            }
            block
                .expr
                .and_then(|e| infer_callback_signature_from_body(tcx, e, param_name))
        }
        hir::ExprKind::Match(scrutinee, arms, _) => {
            infer_callback_signature_from_body(tcx, scrutinee, param_name).or_else(|| {
                arms.iter()
                    .find_map(|arm| infer_callback_signature_from_body(tcx, arm.body, param_name))
            })
        }
        hir::ExprKind::Call(callee, args) => {
            infer_callback_signature_from_body(tcx, callee, param_name).or_else(|| {
                args.iter()
                    .find_map(|a| infer_callback_signature_from_body(tcx, a, param_name))
            })
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            infer_callback_signature_from_body(tcx, cond, param_name)
                .or_else(|| infer_callback_signature_from_body(tcx, then_expr, param_name))
                .or_else(|| {
                    else_expr.and_then(|e| infer_callback_signature_from_body(tcx, e, param_name))
                })
        }
        _ => None,
    }
}

/// Infer callback param types from the argument expression passed to `.call(args)`.
/// The argument can be a tuple `(a, b, c)` or a single value.
pub(in crate::extract) fn infer_callback_params_from_arg<'tcx>(
    tcx: TyCtxt<'tcx>,
    arg_expr: &'tcx hir::Expr<'tcx>,
) -> Vec<LuaType> {
    let typeck = tcx.typeck(arg_expr.hir_id.owner.def_id);
    let ty = typeck.node_type(arg_expr.hir_id);

    match ty.kind() {
        // Empty tuple = no params
        ty::TyKind::Tuple(fields) if fields.is_empty() => vec![],
        // Tuple = multiple params
        ty::TyKind::Tuple(fields) => fields.iter().map(|t| map_ty_to_lua(tcx, t)).collect(),
        // Single non-unit value = one param
        _ => {
            let lua_ty = map_ty_to_lua(tcx, ty);
            if lua_ty == LuaType::Nil {
                vec![]
            } else {
                vec![lua_ty]
            }
        }
    }
}

pub(in crate::extract) fn infer_take_binding_type(
    tcx: TyCtxt<'_>,
    expr: &hir::Expr<'_>,
    binding_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);
    let hir::ExprKind::MethodCall(segment, receiver, _, _) = &expr.kind else {
        return None;
    };

    if segment.ident.name.as_str() != "take" || !expr_refers_to_binding(receiver, binding_name) {
        return None;
    }

    method_generic_lua_type(tcx, expr, segment)
}

pub(in crate::extract) fn infer_from_value_binding_type(
    tcx: TyCtxt<'_>,
    expr: &hir::Expr<'_>,
    args: &[hir::Expr<'_>],
    binding_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);
    let hir::ExprKind::MethodCall(segment, _receiver, _, _) = &expr.kind else {
        return None;
    };

    if segment.ident.name.as_str() != "from_value"
        || !args
            .first()
            .is_some_and(|arg| expr_refers_to_binding(arg, binding_name))
    {
        return None;
    }

    let ty = tcx.typeck(expr.hir_id.owner.def_id).expr_ty(expr);
    let lua_ty = map_ty_to_lua(tcx, unwrap_result_ty(tcx, ty));
    is_informative(&lua_ty).then_some(lua_ty)
}
