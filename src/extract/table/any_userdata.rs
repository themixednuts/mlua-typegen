use mlua_typegen::{LuaType, make_union};
use rustc_hir as hir;
use rustc_middle::ty::TyCtxt;

use super::super::{is_informative, map_ty_to_lua, peel_try_expr};
use super::{
    called_path_name, expr_refers_to_binding, extract_type_id_guard_lua_type,
    infer_value_converter_types_from_hir, is_type_id_call_on_binding, json_like_lua_type,
    path_contains_segment,
};

pub(in crate::extract) fn infer_any_userdata_param_type_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Match(scrutinee, arms, _)
            if is_type_id_call_on_binding(scrutinee, param_name) =>
        {
            let mut tys: Vec<_> = arms
                .iter()
                .filter_map(|arm| extract_type_id_guard_lua_type(tcx, arm.guard))
                .collect();
            if tys.is_empty() {
                tys = infer_value_converter_types_from_hir(tcx, expr);
            }
            (!tys.is_empty()).then(|| make_union(tys))
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .find_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local.init.and_then(|init| {
                    infer_any_userdata_param_type_from_body(tcx, init, param_name)
                }),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    infer_any_userdata_param_type_from_body(tcx, expr, param_name)
                }
                _ => None,
            })
            .or_else(|| {
                block
                    .expr
                    .and_then(|expr| infer_any_userdata_param_type_from_body(tcx, expr, param_name))
            }),
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            infer_any_userdata_param_type_from_body(tcx, cond, param_name)
                .or_else(|| infer_any_userdata_param_type_from_body(tcx, then_expr, param_name))
                .or_else(|| {
                    else_expr.and_then(|expr| {
                        infer_any_userdata_param_type_from_body(tcx, expr, param_name)
                    })
                })
        }
        hir::ExprKind::Call(callee, args) => {
            infer_any_userdata_param_type_from_body(tcx, callee, param_name).or_else(|| {
                args.iter()
                    .find_map(|arg| infer_any_userdata_param_type_from_body(tcx, arg, param_name))
            })
        }
        hir::ExprKind::Binary(_, lhs, rhs) => {
            infer_any_userdata_param_type_from_body(tcx, lhs, param_name)
                .or_else(|| infer_any_userdata_param_type_from_body(tcx, rhs, param_name))
        }
        hir::ExprKind::Let(let_expr) => {
            infer_any_userdata_param_type_from_body(tcx, let_expr.init, param_name)
        }
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            infer_any_userdata_param_type_from_body(tcx, receiver, param_name).or_else(|| {
                args.iter()
                    .find_map(|arg| infer_any_userdata_param_type_from_body(tcx, arg, param_name))
            })
        }
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            infer_any_userdata_param_type_from_body(tcx, body.value, param_name)
        }
        hir::ExprKind::Ret(Some(inner)) => {
            infer_any_userdata_param_type_from_body(tcx, inner, param_name)
        }
        _ => None,
    }
}

/// Infer a param's type from `from_lua::<T>(param)` or `let x: T = from_lua(param)?` patterns.
/// When a `Value` param is immediately converted via `from_lua`, the target type reveals the
/// actual expected Lua type.
pub(in crate::extract) fn infer_from_lua_conversion_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Block(block, _) => {
            for stmt in block.stmts {
                // Look for: let var: T = from_lua(param)?;
                if let hir::StmtKind::Let(local) = &stmt.kind
                    && let Some(init) = local.init
                {
                    let init = peel_try_expr(init);
                    if is_from_lua_call_on_param(init, param_name) {
                        // Get the type of the let binding from typeck
                        let typeck = tcx.typeck(local.pat.hir_id.owner.def_id);
                        let ty = typeck.node_type(local.pat.hir_id);
                        let lua_ty = map_ty_to_lua(tcx, ty);
                        if is_informative(&lua_ty) {
                            return Some(lua_ty);
                        }
                    }
                }
                // Recurse into expressions
                if let hir::StmtKind::Expr(e) | hir::StmtKind::Semi(e) = &stmt.kind
                    && let Some(ty) = infer_from_lua_conversion_type(tcx, e, param_name)
                {
                    return Some(ty);
                }
            }
            block
                .expr
                .and_then(|e| infer_from_lua_conversion_type(tcx, e, param_name))
        }
        _ => None,
    }
}

/// Check if an expression is a `from_lua(param)` or `from_dynamic(param)` call.
pub(in crate::extract) fn is_from_lua_call_on_param(
    expr: &hir::Expr<'_>,
    param_name: &str,
) -> bool {
    match &expr.kind {
        hir::ExprKind::Call(callee, args) => {
            let name = called_path_name(callee);
            if name.as_deref().is_some_and(|n| {
                n == "from_lua" || n == "from_dynamic" || n == "from_lua_value_dynamic"
            }) && args
                .iter()
                .any(|arg| expr_refers_to_binding(arg, param_name))
            {
                return true;
            }
            false
        }
        _ => false,
    }
}

pub(in crate::extract) fn infer_json_like_param_type_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Call(callee, args) => {
            if let Some(name) = called_path_name(callee)
                && matches!(
                    name.as_str(),
                    "to_string" | "to_string_pretty" | "to_vec" | "to_vec_pretty"
                )
                && args
                    .first()
                    .is_some_and(|arg| expr_refers_to_binding(arg, param_name))
                && path_contains_segment(callee, "serde_json")
            {
                return Some(json_like_lua_type());
            }

            infer_json_like_param_type_from_body(tcx, callee, param_name).or_else(|| {
                args.iter()
                    .find_map(|arg| infer_json_like_param_type_from_body(tcx, arg, param_name))
            })
        }
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            infer_json_like_param_type_from_body(tcx, receiver, param_name).or_else(|| {
                args.iter()
                    .find_map(|arg| infer_json_like_param_type_from_body(tcx, arg, param_name))
            })
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .find_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local
                    .init
                    .and_then(|init| infer_json_like_param_type_from_body(tcx, init, param_name)),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    infer_json_like_param_type_from_body(tcx, expr, param_name)
                }
                _ => None,
            })
            .or_else(|| {
                block
                    .expr
                    .and_then(|expr| infer_json_like_param_type_from_body(tcx, expr, param_name))
            }),
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            infer_json_like_param_type_from_body(tcx, cond, param_name)
                .or_else(|| infer_json_like_param_type_from_body(tcx, then_expr, param_name))
                .or_else(|| {
                    else_expr.and_then(|expr| {
                        infer_json_like_param_type_from_body(tcx, expr, param_name)
                    })
                })
        }
        hir::ExprKind::Match(scrutinee, arms, _) => {
            infer_json_like_param_type_from_body(tcx, scrutinee, param_name).or_else(|| {
                arms.iter()
                    .find_map(|arm| infer_json_like_param_type_from_body(tcx, arm.body, param_name))
            })
        }
        hir::ExprKind::Assign(lhs, rhs, _) | hir::ExprKind::AssignOp(_, lhs, rhs) => {
            infer_json_like_param_type_from_body(tcx, lhs, param_name)
                .or_else(|| infer_json_like_param_type_from_body(tcx, rhs, param_name))
        }
        hir::ExprKind::Binary(_, lhs, rhs) => {
            infer_json_like_param_type_from_body(tcx, lhs, param_name)
                .or_else(|| infer_json_like_param_type_from_body(tcx, rhs, param_name))
        }
        hir::ExprKind::Let(let_expr) => {
            infer_json_like_param_type_from_body(tcx, let_expr.init, param_name)
        }
        hir::ExprKind::Struct(_, fields, tail) => fields
            .iter()
            .find_map(|field| infer_json_like_param_type_from_body(tcx, field.expr, param_name))
            .or_else(|| match tail {
                hir::StructTailExpr::Base(base) => {
                    infer_json_like_param_type_from_body(tcx, base, param_name)
                }
                _ => None,
            }),
        hir::ExprKind::Array(items) | hir::ExprKind::Tup(items) => items
            .iter()
            .find_map(|item| infer_json_like_param_type_from_body(tcx, item, param_name)),
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            infer_json_like_param_type_from_body(tcx, body.value, param_name)
        }
        hir::ExprKind::Ret(Some(inner)) => {
            infer_json_like_param_type_from_body(tcx, inner, param_name)
        }
        _ => None,
    }
}
