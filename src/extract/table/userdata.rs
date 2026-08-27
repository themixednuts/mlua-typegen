use mlua_typegen::{LuaType, make_union};
use rustc_hir as hir;
use rustc_middle::ty::TyCtxt;

use super::super::{is_informative, path_tail_name, peel_try_expr};
use super::{
    callee_try_from_type_name, hir_ty_to_lua_type, method_generic_lua_type,
    typeck_method_generic_type,
};

pub(in crate::extract) fn infer_sequence_values_item_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    binding_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, _, _) => {
            if segment.ident.name.as_str() == "sequence_values"
                && path_tail_name(receiver).as_deref() == Some(binding_name)
            {
                // Try HIR generic args first
                if let Some(args) = segment.args
                    && let Some(hir::GenericArg::Type(ty)) = args.args.first()
                    && let Some(lua_ty) = hir_ty_to_lua_type(tcx, ty)
                    && is_informative(&lua_ty)
                {
                    return Some(lua_ty);
                }
                // Try typeck-resolved generics
                return typeck_method_generic_type(tcx, expr);
            }

            infer_sequence_values_item_type(tcx, receiver, binding_name)
        }
        hir::ExprKind::Call(callee, args) => {
            infer_sequence_values_item_type(tcx, callee, binding_name).or_else(|| {
                args.iter()
                    .find_map(|arg| infer_sequence_values_item_type(tcx, arg, binding_name))
            })
        }
        hir::ExprKind::Assign(lhs, rhs, _) => {
            infer_sequence_values_item_type(tcx, lhs, binding_name)
                .or_else(|| infer_sequence_values_item_type(tcx, rhs, binding_name))
        }
        hir::ExprKind::AssignOp(_, lhs, rhs) => {
            infer_sequence_values_item_type(tcx, lhs, binding_name)
                .or_else(|| infer_sequence_values_item_type(tcx, rhs, binding_name))
        }
        hir::ExprKind::Binary(_, lhs, rhs) => {
            infer_sequence_values_item_type(tcx, lhs, binding_name)
                .or_else(|| infer_sequence_values_item_type(tcx, rhs, binding_name))
        }
        hir::ExprKind::Let(let_expr) => {
            infer_sequence_values_item_type(tcx, let_expr.init, binding_name)
        }
        hir::ExprKind::Struct(_, fields, tail) => fields
            .iter()
            .find_map(|field| infer_sequence_values_item_type(tcx, field.expr, binding_name))
            .or_else(|| match tail {
                hir::StructTailExpr::Base(base) => {
                    infer_sequence_values_item_type(tcx, base, binding_name)
                }
                _ => None,
            }),
        hir::ExprKind::Match(scrutinee, arms, _) => {
            infer_sequence_values_item_type(tcx, scrutinee, binding_name).or_else(|| {
                arms.iter()
                    .find_map(|arm| infer_sequence_values_item_type(tcx, arm.body, binding_name))
            })
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .find_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local
                    .init
                    .and_then(|init| infer_sequence_values_item_type(tcx, init, binding_name)),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    infer_sequence_values_item_type(tcx, expr, binding_name)
                }
                _ => None,
            })
            .or_else(|| {
                block
                    .expr
                    .and_then(|expr| infer_sequence_values_item_type(tcx, expr, binding_name))
            }),
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            infer_sequence_values_item_type(tcx, cond, binding_name)
                .or_else(|| infer_sequence_values_item_type(tcx, then_expr, binding_name))
                .or_else(|| {
                    else_expr
                        .and_then(|expr| infer_sequence_values_item_type(tcx, expr, binding_name))
                })
        }
        hir::ExprKind::Ret(Some(inner)) => {
            infer_sequence_values_item_type(tcx, inner, binding_name)
        }
        hir::ExprKind::Array(items) => items
            .iter()
            .find_map(|item| infer_sequence_values_item_type(tcx, item, binding_name)),
        hir::ExprKind::Tup(items) => items
            .iter()
            .find_map(|item| infer_sequence_values_item_type(tcx, item, binding_name)),
        _ => None,
    }
}

pub(in crate::extract) fn infer_userdata_binding_type_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    binding_name: &str,
) -> Option<LuaType> {
    let types = collect_userdata_binding_types_from_body(tcx, expr, binding_name);
    (!types.is_empty()).then(|| make_union(types))
}

pub(in crate::extract) fn collect_userdata_binding_types_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    binding_name: &str,
) -> Vec<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, _, _) => {
            let mut types = Vec::new();

            if matches!(
                segment.ident.name.as_str(),
                "take" | "borrow" | "borrow_mut" | "try_borrow" | "try_borrow_mut"
            ) && expr_refers_to_binding(receiver, binding_name)
                && let Some(LuaType::Class(name)) = method_generic_lua_type(tcx, expr, segment)
            {
                types.push(LuaType::Class(name));
            }

            types.extend(collect_userdata_binding_types_from_body(
                tcx,
                receiver,
                binding_name,
            ));
            if let hir::ExprKind::MethodCall(_, _, args, _) = &expr.kind {
                for arg in args.iter() {
                    types.extend(collect_userdata_binding_types_from_body(
                        tcx,
                        arg,
                        binding_name,
                    ));
                }
            }
            types
        }
        hir::ExprKind::Call(callee, args) => {
            let mut types = Vec::new();

            if let Some(name) = callee_try_from_type_name(tcx, callee)
                && args
                    .first()
                    .is_some_and(|arg| expr_refers_to_binding(arg, binding_name))
            {
                types.push(LuaType::Class(name));
            }

            types.extend(collect_userdata_binding_types_from_body(
                tcx,
                callee,
                binding_name,
            ));
            for arg in args.iter() {
                types.extend(collect_userdata_binding_types_from_body(
                    tcx,
                    arg,
                    binding_name,
                ));
            }
            types
        }
        hir::ExprKind::Assign(lhs, rhs, _) | hir::ExprKind::AssignOp(_, lhs, rhs) => {
            let mut types = collect_userdata_binding_types_from_body(tcx, lhs, binding_name);
            types.extend(collect_userdata_binding_types_from_body(
                tcx,
                rhs,
                binding_name,
            ));
            types
        }
        hir::ExprKind::Binary(_, lhs, rhs) => {
            let mut types = collect_userdata_binding_types_from_body(tcx, lhs, binding_name);
            types.extend(collect_userdata_binding_types_from_body(
                tcx,
                rhs,
                binding_name,
            ));
            types
        }
        hir::ExprKind::Let(let_expr) => {
            collect_userdata_binding_types_from_body(tcx, let_expr.init, binding_name)
        }
        hir::ExprKind::Struct(_, fields, tail) => fields
            .iter()
            .flat_map(|field| {
                collect_userdata_binding_types_from_body(tcx, field.expr, binding_name)
            })
            .chain(match tail {
                hir::StructTailExpr::Base(base) => {
                    collect_userdata_binding_types_from_body(tcx, base, binding_name)
                }
                _ => Vec::new(),
            })
            .collect(),
        hir::ExprKind::Match(scrutinee, arms, _) => {
            let mut types = collect_userdata_binding_types_from_body(tcx, scrutinee, binding_name);
            for arm in arms.iter() {
                types.extend(collect_userdata_binding_types_from_body(
                    tcx,
                    arm.body,
                    binding_name,
                ));
            }
            types
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .flat_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local
                    .init
                    .map(|init| collect_userdata_binding_types_from_body(tcx, init, binding_name))
                    .unwrap_or_default(),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    collect_userdata_binding_types_from_body(tcx, expr, binding_name)
                }
                _ => Vec::new(),
            })
            .chain(
                block.expr.into_iter().flat_map(|expr| {
                    collect_userdata_binding_types_from_body(tcx, expr, binding_name)
                }),
            )
            .collect(),
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            let mut types = collect_userdata_binding_types_from_body(tcx, cond, binding_name);
            types.extend(collect_userdata_binding_types_from_body(
                tcx,
                then_expr,
                binding_name,
            ));
            if let Some(else_expr) = else_expr {
                types.extend(collect_userdata_binding_types_from_body(
                    tcx,
                    else_expr,
                    binding_name,
                ));
            }
            types
        }
        hir::ExprKind::Ret(Some(inner)) => {
            collect_userdata_binding_types_from_body(tcx, inner, binding_name)
        }
        hir::ExprKind::Array(items) => items
            .iter()
            .flat_map(|item| collect_userdata_binding_types_from_body(tcx, item, binding_name))
            .collect(),
        hir::ExprKind::Tup(items) => items
            .iter()
            .flat_map(|item| collect_userdata_binding_types_from_body(tcx, item, binding_name))
            .collect(),
        _ => Vec::new(),
    }
}

pub(in crate::extract) fn expr_refers_to_binding(expr: &hir::Expr<'_>, binding_name: &str) -> bool {
    path_tail_name(expr).as_deref() == Some(binding_name)
}
