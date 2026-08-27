use mlua_typegen::{LuaType, make_union};
use rustc_hir as hir;
use rustc_middle::ty::TyCtxt;

use super::{
    callee_try_from_type_name, expr_refers_to_binding, infer_from_value_binding_type,
    infer_param_type_from_converter_call, infer_param_type_from_local_call,
    infer_take_binding_type, lua_type_from_value_arm, path_tail_name, peel_try_expr,
    string_bytes_binding_name, string_literal_from_pat, tuple_arm_param_type,
    tuple_scrutinee_param_positions,
};

pub(super) fn infer_param_type_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Match(scrutinee, arms, _) => {
            let tuple_positions = tuple_scrutinee_param_positions(scrutinee, param_name);
            if !tuple_positions.is_empty() {
                let mut tys = Vec::new();
                for arm in arms.iter() {
                    if let Some(ty) = tuple_arm_param_type(tcx, arm.pat, &tuple_positions) {
                        tys.push(ty);
                    }
                }
                if !tys.is_empty() {
                    return Some(make_union(tys));
                }
            }

            if string_bytes_binding_name(scrutinee).as_deref() == Some(param_name) {
                let literals: Vec<_> = arms
                    .iter()
                    .filter_map(|arm| string_literal_from_pat(tcx, arm.pat))
                    .collect();
                if !literals.is_empty() {
                    return Some(make_union(
                        literals
                            .into_iter()
                            .map(|literal| LuaType::StringLiteral(vec![literal]))
                            .collect(),
                    ));
                }
            }

            if path_tail_name(scrutinee).as_deref() == Some(param_name) {
                let mut tys = Vec::new();
                let mut found = false;

                for arm in arms.iter() {
                    if matches!(arm.pat.kind, hir::PatKind::Wild) {
                        continue;
                    }

                    if let Some(ty) = lua_type_from_value_arm(tcx, arm) {
                        tys.push(ty);
                        found = true;
                    }
                }

                if found {
                    return Some(make_union(tys));
                }
            }

            let mut tys = Vec::new();
            if let Some(ty) = infer_param_type_from_body(tcx, scrutinee, param_name) {
                tys.push(ty);
            }
            for arm in arms.iter() {
                if let Some(ty) = infer_param_type_from_body(tcx, arm.body, param_name) {
                    tys.push(ty);
                }
            }
            if tys.is_empty() {
                None
            } else {
                Some(make_union(tys))
            }
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .find_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local
                    .init
                    .and_then(|init| infer_param_type_from_body(tcx, init, param_name)),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    infer_param_type_from_body(tcx, expr, param_name)
                }
                _ => None,
            })
            .or_else(|| {
                block
                    .expr
                    .and_then(|expr| infer_param_type_from_body(tcx, expr, param_name))
            }),
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            let mut tys = Vec::new();
            if let Some(ty) = infer_param_type_from_body(tcx, cond, param_name) {
                tys.push(ty);
            }
            if let Some(ty) = infer_param_type_from_body(tcx, then_expr, param_name) {
                tys.push(ty);
            }
            if let Some(ty) =
                else_expr.and_then(|expr| infer_param_type_from_body(tcx, expr, param_name))
            {
                tys.push(ty);
            }
            if tys.is_empty() {
                None
            } else {
                Some(make_union(tys))
            }
        }
        hir::ExprKind::Call(callee, args) => infer_param_type_from_body(tcx, callee, param_name)
            .or_else(|| {
                callee_try_from_type_name(tcx, callee).and_then(|name| {
                    args.first()
                        .filter(|arg| expr_refers_to_binding(arg, param_name))
                        .map(|_| LuaType::Class(name))
                })
            })
            .or_else(|| infer_param_type_from_local_call(tcx, callee, args, param_name))
            .or_else(|| infer_param_type_from_converter_call(tcx, callee, args, param_name))
            .or_else(|| {
                args.iter()
                    .find_map(|arg| infer_param_type_from_body(tcx, arg, param_name))
            }),
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            infer_from_value_binding_type(tcx, expr, args, param_name)
                .or_else(|| infer_take_binding_type(tcx, expr, param_name))
                .or_else(|| infer_param_type_from_body(tcx, receiver, param_name))
                .or_else(|| {
                    args.iter()
                        .find_map(|arg| infer_param_type_from_body(tcx, arg, param_name))
                })
        }
        hir::ExprKind::Assign(lhs, rhs, _) => infer_param_type_from_body(tcx, lhs, param_name)
            .or_else(|| infer_param_type_from_body(tcx, rhs, param_name)),
        hir::ExprKind::AssignOp(_, lhs, rhs) => infer_param_type_from_body(tcx, lhs, param_name)
            .or_else(|| infer_param_type_from_body(tcx, rhs, param_name)),
        hir::ExprKind::Binary(_, lhs, rhs) => infer_param_type_from_body(tcx, lhs, param_name)
            .or_else(|| infer_param_type_from_body(tcx, rhs, param_name)),
        hir::ExprKind::Let(let_expr) => infer_param_type_from_body(tcx, let_expr.init, param_name),
        hir::ExprKind::Struct(_, fields, tail) => fields
            .iter()
            .find_map(|field| infer_param_type_from_body(tcx, field.expr, param_name))
            .or_else(|| match tail {
                hir::StructTailExpr::Base(base) => {
                    infer_param_type_from_body(tcx, base, param_name)
                }
                _ => None,
            }),
        hir::ExprKind::Array(items) => items
            .iter()
            .find_map(|item| infer_param_type_from_body(tcx, item, param_name)),
        hir::ExprKind::Tup(items) => items
            .iter()
            .find_map(|item| infer_param_type_from_body(tcx, item, param_name)),
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            infer_param_type_from_body(tcx, body.value, param_name)
        }
        hir::ExprKind::Ret(Some(inner)) => infer_param_type_from_body(tcx, inner, param_name),
        _ => None,
    }
}
