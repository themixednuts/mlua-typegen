use mlua_typegen::{LuaReturn, LuaType};
use rustc_hir as hir;
use rustc_middle::ty::{self, TyCtxt};

use super::super::{
    enclosing_impl_self_type_name, extract_meta_method_name, extract_string_literal,
    extract_type_from_closure_expr, find_into_lua_multi_tuple, find_into_lua_receiver_type,
    infer_expr_lua_type, infer_from_expr, is_informative, is_passthrough_method,
    merge_body_inference, peel_try_expr, resolve_any_user_data_class, type_display_name,
};
use super::expression::infer_returns_from_expr;
use super::pairs::{infer_pairs_forward_returns, infer_pairs_state_type};

/// Walk a closure body to infer the concrete type when the signature says `Value` (any).
/// Recursively searches the entire expression tree for `.into_lua()` calls and
/// returns the type of the receiver — the concrete type before Lua conversion.
pub(in crate::extract) fn infer_concrete_type_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    let mut best = infer_from_expr(tcx, typeck, expr);

    // Fall back to a deep search for .into_lua() calls in the entire tree.
    if let Some(candidate) = find_into_lua_receiver_type(tcx, expr) {
        best = merge_body_inference(best, candidate);
    }

    // Last resort: search for constructor-style expressions, even without
    // .into_lua() (e.g. methods returning Result<Option<Self>> directly).
    if let Some(class_name) = extract_type_from_closure_expr(tcx, expr) {
        best = merge_body_inference(best, LuaType::Class(class_name));
    }

    best.filter(is_informative)
}

/// Infer multiple return types from a closure body that uses `.into_lua_multi()`.
/// When the receiver is a tuple `(A, B)`, returns `vec![A_lua, B_lua]`.
/// Falls back to `infer_concrete_type_from_body` for single-return cases.
pub(in crate::extract) fn infer_multi_returns_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<Vec<LuaReturn>> {
    if let Some(returns) = infer_returns_from_expr(tcx, expr) {
        return Some(returns);
    }
    // Search for .into_lua_multi() on a tuple receiver
    if let Some(returns) = find_into_lua_multi_tuple(tcx, expr) {
        return Some(returns);
    }
    // Fall back to single-type inference
    infer_concrete_type_from_body(tcx, expr).map(|ty| vec![ty.into()])
}

pub(in crate::extract) fn infer_forwarded_multivalue_returns<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    segment: &hir::PathSegment<'tcx>,
    args: &'tcx [hir::Expr<'tcx>],
) -> Option<Vec<LuaReturn>> {
    let runtime_name = args
        .first()
        .and_then(|arg| extract_runtime_method_name(tcx, arg));
    let state = args
        .get(1)
        .and_then(|arg| infer_forwarded_userdata_state_type(tcx, arg));

    if !matches!(
        segment.ident.name.as_str(),
        "call_function" | "call_async_function"
    ) {
        return None;
    }

    if !matches!(infer_expr_lua_type(tcx, expr), LuaType::Variadic(_)) {
        return None;
    }

    let method_name = runtime_name?;
    if method_name != "__pairs" {
        return None;
    }

    let state = infer_pairs_state_type(tcx, expr, &state?);
    Some(infer_pairs_forward_returns(&state))
}

pub(in crate::extract) fn extract_runtime_method_name(
    tcx: TyCtxt<'_>,
    expr: &hir::Expr<'_>,
) -> Option<String> {
    let expr = peel_try_expr(expr);

    if let Some(name) = extract_string_literal(expr) {
        return Some(name);
    }

    let hir::ExprKind::MethodCall(segment, receiver, _, _) = &expr.kind else {
        return None;
    };
    (segment.ident.name.as_str() == "name")
        .then(|| extract_meta_method_name(tcx, receiver))
        .flatten()
}

pub(in crate::extract) fn infer_forwarded_userdata_state_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Call(callee, args) => infer_informative_expr_type(tcx, expr)
            .or_else(|| {
                args.iter()
                    .find_map(|arg| infer_forwarded_userdata_state_type(tcx, arg))
            })
            .or_else(|| infer_forwarded_userdata_state_type(tcx, callee))
            .or_else(|| resolve_any_user_data_class(tcx, expr)),
        hir::ExprKind::MethodCall(segment, receiver, _, _)
            if is_passthrough_method(segment.ident.name.as_str()) =>
        {
            infer_forwarded_userdata_state_type(tcx, receiver)
        }
        hir::ExprKind::Field(base, ident) => {
            infer_userdata_field_init_type(tcx, base, ident.name.as_str())
                .or_else(|| infer_informative_expr_type(tcx, expr))
                .or_else(|| resolve_any_user_data_class(tcx, expr))
        }
        _ => infer_informative_expr_type(tcx, expr)
            .or_else(|| resolve_any_user_data_class(tcx, expr)),
    }
}

pub(in crate::extract) fn infer_informative_expr_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    infer_from_expr(tcx, typeck, expr).filter(is_informative)
}

pub(in crate::extract) fn infer_userdata_field_init_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    base: &'tcx hir::Expr<'tcx>,
    field_name: &str,
) -> Option<LuaType> {
    let class_name = expr_userdata_container_name(tcx, base)
        .or_else(|| enclosing_impl_self_type_name(tcx, base.hir_id.owner.def_id))?;

    let mut best = None;
    for item_id in tcx.hir_crate_items(()).free_items() {
        let item = tcx.hir_item(item_id);
        if let Some(candidate) =
            infer_struct_field_init_type_from_item(tcx, item, &class_name, field_name)
        {
            best = merge_body_inference(best, candidate);
        }
    }

    best
}

pub(in crate::extract) fn infer_struct_field_init_type_from_item<'tcx>(
    tcx: TyCtxt<'tcx>,
    item: &'tcx hir::Item<'tcx>,
    class_name: &str,
    field_name: &str,
) -> Option<LuaType> {
    match &item.kind {
        hir::ItemKind::Mod(_, module) => module.item_ids.iter().fold(None, |best, item_id| {
            let candidate = infer_struct_field_init_type_from_item(
                tcx,
                tcx.hir_item(*item_id),
                class_name,
                field_name,
            );
            match candidate {
                Some(candidate) => merge_body_inference(best, candidate),
                None => best,
            }
        }),
        hir::ItemKind::Fn { body, .. } => {
            let body = tcx.hir_body(*body);
            infer_struct_field_init_type_from_expr(tcx, body.value, class_name, field_name)
        }
        hir::ItemKind::Impl(impl_block) => {
            impl_block.items.iter().fold(None, |best, impl_item_ref| {
                let impl_item_id = hir::ImplItemId {
                    owner_id: impl_item_ref.owner_id,
                };
                let impl_item = tcx.hir_impl_item(impl_item_id);
                let candidate = match impl_item.kind {
                    hir::ImplItemKind::Fn(_, body_id) => {
                        let body = tcx.hir_body(body_id);
                        infer_struct_field_init_type_from_expr(
                            tcx, body.value, class_name, field_name,
                        )
                    }
                    _ => None,
                };
                match candidate {
                    Some(candidate) => merge_body_inference(best, candidate),
                    None => best,
                }
            })
        }
        _ => None,
    }
}

pub(in crate::extract) fn expr_userdata_container_name<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<String> {
    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    let mut ty = typeck.expr_ty(expr);

    loop {
        match ty.kind() {
            ty::TyKind::Ref(_, inner, _) => ty = *inner,
            ty::TyKind::Adt(..) => return Some(type_display_name(tcx, ty)),
            _ => return None,
        }
    }
}

pub(in crate::extract) fn infer_struct_field_init_type_from_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    class_name: &str,
    field_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Struct(_, fields, tail) => fields
            .iter()
            .find(|field| {
                expr_userdata_container_name(tcx, expr).as_deref() == Some(class_name)
                    && field.ident.name.as_str() == field_name
            })
            .and_then(|field| infer_forwarded_userdata_state_type(tcx, field.expr))
            .or_else(|| match tail {
                hir::StructTailExpr::Base(base) => {
                    infer_struct_field_init_type_from_expr(tcx, base, class_name, field_name)
                }
                _ => None,
            }),
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .find_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local.init.and_then(|init| {
                    infer_struct_field_init_type_from_expr(tcx, init, class_name, field_name)
                }),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    infer_struct_field_init_type_from_expr(tcx, expr, class_name, field_name)
                }
                _ => None,
            })
            .or_else(|| {
                block.expr.and_then(|expr| {
                    infer_struct_field_init_type_from_expr(tcx, expr, class_name, field_name)
                })
            }),
        hir::ExprKind::Call(callee, args) => infer_struct_field_init_type_from_expr(
            tcx, callee, class_name, field_name,
        )
        .or_else(|| {
            args.iter().find_map(|arg| {
                infer_struct_field_init_type_from_expr(tcx, arg, class_name, field_name)
            })
        }),
        hir::ExprKind::MethodCall(_, receiver, args, _) => infer_struct_field_init_type_from_expr(
            tcx, receiver, class_name, field_name,
        )
        .or_else(|| {
            args.iter().find_map(|arg| {
                infer_struct_field_init_type_from_expr(tcx, arg, class_name, field_name)
            })
        }),
        hir::ExprKind::Match(scrutinee, arms, _) => infer_struct_field_init_type_from_expr(
            tcx, scrutinee, class_name, field_name,
        )
        .or_else(|| {
            arms.iter().find_map(|arm| {
                infer_struct_field_init_type_from_expr(tcx, arm.body, class_name, field_name)
            })
        }),
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            infer_struct_field_init_type_from_expr(tcx, cond, class_name, field_name)
                .or_else(|| {
                    infer_struct_field_init_type_from_expr(tcx, then_expr, class_name, field_name)
                })
                .or_else(|| {
                    else_expr.and_then(|expr| {
                        infer_struct_field_init_type_from_expr(tcx, expr, class_name, field_name)
                    })
                })
        }
        hir::ExprKind::Ret(Some(inner)) => {
            infer_struct_field_init_type_from_expr(tcx, inner, class_name, field_name)
        }
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            infer_struct_field_init_type_from_expr(tcx, body.value, class_name, field_name)
        }
        _ => None,
    }
}
