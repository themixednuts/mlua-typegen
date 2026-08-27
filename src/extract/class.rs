use mlua_typegen::{LuaMethod, LuaReturn, MethodKind};
use rustc_hir as hir;
use rustc_hir::def_id::LocalDefId;
use rustc_middle::ty::{self, TyCtxt};

use super::{
    expr_snippet, extract_closure_signature, extract_meta_method_name, extract_string_literal,
    infer_forwarded_multivalue_returns, is_method_registration_name, peel_expr, peel_try_expr,
    should_replace_returns, should_trace_method_name, trace,
};

/// Get a display name for a type (last path segment).
pub(super) fn type_display_name(tcx: TyCtxt<'_>, ty: ty::Ty<'_>) -> String {
    match ty.kind() {
        ty::TyKind::Adt(adt_def, _) => {
            let path = tcx.def_path_str(adt_def.did());
            path.rsplit("::").next().unwrap_or(&path).to_string()
        }
        _ => format!("{ty}"),
    }
}

pub(super) fn enclosing_impl_self_type_name(
    tcx: TyCtxt<'_>,
    mut local: LocalDefId,
) -> Option<String> {
    loop {
        // Stop at the crate root — it has no parent.
        if local == rustc_hir::def_id::CRATE_DEF_ID {
            return None;
        }
        let parent = tcx.local_parent(local);
        if parent == local {
            return None;
        }

        // Only call type_of on defs that have types; modules, functions, etc.
        // will ICE if queried for type_of.
        let def_kind = tcx.def_kind(parent);
        let has_self_ty = matches!(
            def_kind,
            rustc_hir::def::DefKind::Impl { .. }
                | rustc_hir::def::DefKind::Struct
                | rustc_hir::def::DefKind::Enum
                | rustc_hir::def::DefKind::Union
        );

        if has_self_ty {
            let self_ty = tcx.type_of(parent).skip_binder();
            if let ty::TyKind::Adt(..) = self_ty.kind() {
                return Some(type_display_name(tcx, self_ty));
            }
        }

        local = parent;
    }
}

pub(super) fn record_method_registration<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    methods: &mut Vec<LuaMethod>,
) {
    let hir::ExprKind::MethodCall(segment, _receiver, args, _span) = &expr.kind else {
        return;
    };
    let method_name = segment.ident.name.as_str();
    let is_async = method_name.starts_with("add_async_");
    let registration_name = args.first().and_then(extract_string_literal);
    if let Some(name) = registration_name.as_deref()
        && should_trace_method_name(name)
        && is_method_registration_name(method_name)
    {
        trace(format!(
            "record_method_registration saw registrar={method_name} name={registration_name:?} call={}",
            expr_snippet(tcx, expr)
        ));
    }
    let method = match method_name {
        "add_method"
        | "add_method_mut"
        | "add_method_once"
        | "add_async_method"
        | "add_async_method_mut"
        | "add_async_method_once" => {
            extract_registered_method(tcx, args, MethodKind::Method, is_async)
        }
        "add_function" | "add_function_mut" | "add_async_function" => {
            extract_registered_method(tcx, args, MethodKind::Function, is_async)
        }
        "add_meta_method"
        | "add_meta_method_mut"
        | "add_async_meta_method"
        | "add_async_meta_method_mut" => {
            extract_meta_method(tcx, args, MethodKind::Method, is_async)
        }
        "add_meta_function" | "add_meta_function_mut" | "add_async_meta_function" => {
            extract_meta_method(tcx, args, MethodKind::Function, is_async)
        }
        _ => None,
    };
    if let Some(method) = method {
        methods.push(method);
    }
}

pub(super) fn extract_registered_method<'tcx>(
    tcx: TyCtxt<'tcx>,
    args: &'tcx [hir::Expr<'tcx>],
    kind: MethodKind,
    is_async: bool,
) -> Option<LuaMethod> {
    if args.len() < 2 {
        return None;
    }

    let name = extract_string_literal(args.first()?)?;
    let closure_expr = args.get(1)?;
    let (params, returns) = extract_closure_signature(tcx, closure_expr, kind)?;

    if should_trace_method_name(&name) {
        trace(format!(
            "extract_registered_method name={name} kind={kind:?} params={params:?} returns={returns:?} closure={}",
            expr_snippet(tcx, closure_expr)
        ));
    }

    Some(LuaMethod {
        name,
        kind,
        is_async,
        params,
        returns,
        doc: None,
    })
}

pub(super) fn extract_meta_method<'tcx>(
    tcx: TyCtxt<'tcx>,
    args: &'tcx [hir::Expr<'tcx>],
    kind: MethodKind,
    is_async: bool,
) -> Option<LuaMethod> {
    if args.len() < 2 {
        return None;
    }

    let name = extract_meta_method_name(tcx, args.first()?)?;
    let closure_expr = args.get(1)?;
    let (params, mut returns) = extract_closure_signature(tcx, closure_expr, kind)?;

    if name == "__pairs"
        && let Some(inferred) = infer_forwarded_pairs_returns_from_closure(tcx, closure_expr)
        && should_replace_returns(&returns, &inferred)
    {
        returns = inferred;
    }

    if should_trace_method_name(&name) {
        trace(format!(
            "extract_meta_method name={name} kind={kind:?} params={params:?} returns={returns:?} closure={}",
            expr_snippet(tcx, closure_expr)
        ));
    }

    Some(LuaMethod {
        name,
        kind,
        is_async,
        params,
        returns,
        doc: None,
    })
}

pub(super) fn infer_forwarded_pairs_returns_from_closure<'tcx>(
    tcx: TyCtxt<'tcx>,
    closure_expr: &'tcx hir::Expr<'tcx>,
) -> Option<Vec<LuaReturn>> {
    let hir::ExprKind::Closure(closure) = &closure_expr.kind else {
        return None;
    };

    let body = tcx.hir_body(closure.body);
    find_forwarded_pairs_returns_in_expr(tcx, body.value)
}

pub(super) fn find_forwarded_pairs_returns_in_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<Vec<LuaReturn>> {
    let expr = peel_expr(peel_try_expr(expr));

    match &expr.kind {
        hir::ExprKind::MethodCall(segment, _receiver, args, _) => {
            infer_forwarded_multivalue_returns(tcx, expr, segment, args)
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .find_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local
                    .init
                    .and_then(|init| find_forwarded_pairs_returns_in_expr(tcx, init)),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    find_forwarded_pairs_returns_in_expr(tcx, expr)
                }
                _ => None,
            })
            .or_else(|| {
                block
                    .expr
                    .and_then(|expr| find_forwarded_pairs_returns_in_expr(tcx, expr))
            }),
        hir::ExprKind::Match(scrutinee, arms, _) => {
            find_forwarded_pairs_returns_in_expr(tcx, scrutinee).or_else(|| {
                arms.iter()
                    .find_map(|arm| find_forwarded_pairs_returns_in_expr(tcx, arm.body))
            })
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            find_forwarded_pairs_returns_in_expr(tcx, cond)
                .or_else(|| find_forwarded_pairs_returns_in_expr(tcx, then_expr))
                .or_else(|| {
                    else_expr.and_then(|expr| find_forwarded_pairs_returns_in_expr(tcx, expr))
                })
        }
        hir::ExprKind::Ret(Some(inner)) => find_forwarded_pairs_returns_in_expr(tcx, inner),
        hir::ExprKind::Call(callee, args) => find_forwarded_pairs_returns_in_expr(tcx, callee)
            .or_else(|| {
                args.iter()
                    .find_map(|arg| find_forwarded_pairs_returns_in_expr(tcx, arg))
            }),
        _ => None,
    }
}
