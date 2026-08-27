use mlua_typegen::{EventEmission, ExtractedApi, LuaType};
use rustc_hir as hir;
use rustc_middle::ty::{self, TyCtxt};

use crate::extract::{map_ty_to_lua, peel_try_expr};

/// Record an event emission at one expression visited by the body collector.
///
/// Recognizes patterns like:
/// `emit_event(&lua, ("event-name".to_string(), args))` or
/// `emit_sync_callback(&lua, ("event-name", args))`
/// where args come from `lua.pack_multi(typed_value)`.
pub(in crate::extract) fn record_event_emission<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    api: &mut ExtractedApi,
) {
    let hir::ExprKind::Call(_, call_args) = &expr.kind else {
        return;
    };
    for arg in *call_args {
        if let hir::ExprKind::Tup(fields) = &arg.kind
            && fields.len() == 2
            && let Some(event_name) = extract_string_literal_from_expr(tcx, &fields[0])
            && !event_name.is_empty()
        {
            let arg_types = extract_event_arg_types(tcx, &fields[1]);
            api.event_emissions.push(EventEmission {
                event_name,
                arg_types,
            });
        }
    }
}

/// Extract a string literal from an expression, handling `.to_string()` and `format!` patterns.
pub(in crate::extract) fn extract_string_literal_from_expr(
    _tcx: TyCtxt<'_>,
    expr: &hir::Expr<'_>,
) -> Option<String> {
    match &expr.kind {
        hir::ExprKind::Lit(lit) => {
            if let rustc_ast::LitKind::Str(sym, _) = &lit.node {
                return Some(sym.as_str().to_string());
            }
            None
        }
        // "literal".to_string()
        hir::ExprKind::MethodCall(segment, receiver, _, _) => {
            if segment.ident.name.as_str() == "to_string" {
                return extract_string_literal_from_expr(_tcx, receiver);
            }
            None
        }
        _ => None,
    }
}

/// Extract event callback arg types from an expression.
/// If the expression is the result of `pack_multi(value)`, extract value's type.
/// Also resolves local variable bindings to find the pack_multi call.
pub(in crate::extract) fn extract_event_arg_types<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Vec<LuaType> {
    // Direct pack_multi(value) call
    if let Some(types) = extract_pack_multi_arg_types(tcx, expr) {
        return types;
    }

    // If expr is a local variable reference, resolve to its initializer
    if let hir::ExprKind::Path(hir::QPath::Resolved(_, path)) = &expr.kind
        && let rustc_hir::def::Res::Local(hir_id) = path.res
    {
        let node = tcx.hir_node(hir_id);
        if let rustc_hir::Node::Pat(pat) = node {
            // Walk up to find the enclosing LetStmt via the parent hierarchy
            let parent_node = tcx.hir_node(tcx.parent_hir_id(pat.hir_id));
            if let rustc_hir::Node::LetStmt(local) = parent_node
                && let Some(init) = local.init
            {
                // The initializer might be pack_multi(value)?
                let init = peel_try_expr(init);
                if let Some(types) = extract_pack_multi_arg_types(tcx, init) {
                    return types;
                }
            }
        }
    }

    // Inline tuple expression: (arg1, arg2, ...) — extract each element's type
    if let hir::ExprKind::Tup(fields) = &expr.kind {
        if fields.is_empty() {
            return vec![];
        }
        let typeck = tcx.typeck(expr.hir_id.owner.def_id);
        return fields
            .iter()
            .map(|f| {
                let ty = typeck.node_type(f.hir_id);
                map_ty_to_lua(tcx, ty)
            })
            .collect();
    }

    // Fallback: use typeck on the expression — decompose tuple types
    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    let ty = typeck.node_type(expr.hir_id);
    match ty.kind() {
        ty::TyKind::Tuple(fields) if fields.is_empty() => vec![],
        ty::TyKind::Tuple(fields) => fields.iter().map(|t| map_ty_to_lua(tcx, t)).collect(),
        _ => {
            let lua_ty = map_ty_to_lua(tcx, ty);
            if matches!(lua_ty, LuaType::Any | LuaType::Nil) {
                vec![]
            } else {
                vec![lua_ty]
            }
        }
    }
}

/// Check if an expression is a `pack_multi(value)` call and extract the value's type.
pub(in crate::extract) fn extract_pack_multi_arg_types<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<Vec<LuaType>> {
    let hir::ExprKind::MethodCall(segment, _receiver, args, _) = &expr.kind else {
        return None;
    };
    if segment.ident.name.as_str() != "pack_multi" {
        return None;
    }
    let value_expr = args.first()?;
    let typeck = tcx.typeck(value_expr.hir_id.owner.def_id);
    let ty = typeck.node_type(value_expr.hir_id);
    Some(match ty.kind() {
        ty::TyKind::Tuple(fields) if fields.is_empty() => vec![],
        ty::TyKind::Tuple(fields) => fields.iter().map(|t| map_ty_to_lua(tcx, t)).collect(),
        _ => {
            let lua_ty = map_ty_to_lua(tcx, ty);
            if lua_ty == LuaType::Nil {
                vec![]
            } else {
                vec![lua_ty]
            }
        }
    })
}
