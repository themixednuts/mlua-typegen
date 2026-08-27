use mlua_typegen::{LuaFunction, LuaParam, LuaReturn, LuaType};
use rustc_hir as hir;
use rustc_hir::def_id::LocalDefId;
use rustc_middle::ty::{self, TyCtxt};

use crate::extract::{
    class::enclosing_impl_self_type_name,
    hir_util::{expr_def_id, peel_lua_conversion_expr},
    lower::{extract_params_from_tuple, map_return_ty, map_ty_to_lua},
    lua_type::{is_informative, replace_self_class_alias, replace_self_return_sentinel},
    returns::{enrich_return_names, infer_multi_returns_from_body},
    signature::{
        explicit_lua_params_from_hir, extract_names_from_pat, refine_params_from_body,
        refine_params_from_explicit_types,
    },
    source::expr_snippet,
    table::{terminal_return_is_opaque_any_userdata, unwrap_hir_result_type},
    trace::{trace, trace_global_function_snapshot},
    walk::find_in_recursive_expr_children,
};

use super::value_inference::merge_inferred_returns;

pub(in crate::extract) fn try_extract_function_value<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    name: &str,
) -> Option<LuaFunction> {
    if let Some(func) = try_extract_create_function(tcx, expr, name) {
        return Some(func);
    }

    let expr = peel_lua_conversion_expr(expr);
    let hir::ExprKind::Call(callee, _) = &expr.kind else {
        return None;
    };
    let def_id = expr_def_id(tcx, callee)?;
    let local = def_id.as_local()?;
    extract_function_from_local_fn(tcx, local, name)
}

pub(in crate::extract) fn extract_function_from_local_fn(
    tcx: TyCtxt<'_>,
    def_id: LocalDefId,
    name: &str,
) -> Option<LuaFunction> {
    match tcx.hir_node_by_def_id(def_id) {
        rustc_hir::Node::Item(item) => {
            let hir::ItemKind::Fn { body, .. } = item.kind else {
                return None;
            };
            let body = tcx.hir_body(body);
            find_create_function_in_expr(tcx, body.value, name)
        }
        rustc_hir::Node::ImplItem(impl_item) => {
            let hir::ImplItemKind::Fn(_, body_id) = impl_item.kind else {
                return None;
            };
            let body = tcx.hir_body(body_id);
            find_create_function_in_expr(tcx, body.value, name)
        }
        _ => None,
    }
}

pub(in crate::extract) fn find_create_function_in_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    name: &str,
) -> Option<LuaFunction> {
    if let Some(func) = try_extract_create_function(tcx, expr, name) {
        return Some(func);
    }

    match &expr.kind {
        hir::ExprKind::Closure(_) => None,
        _ => find_in_recursive_expr_children(expr, |child| {
            find_create_function_in_expr(tcx, child, name)
        }),
    }
}

/// Try to extract a LuaFunction from a `lua.create_function(|lua, args| ...)` expression.
pub(in crate::extract) fn try_extract_create_function<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    name: &str,
) -> Option<LuaFunction> {
    // Unwrap `?` operator: the expression might be `lua.create_function(...)?.into_function()?`
    let expr = peel_lua_conversion_expr(expr);

    match &expr.kind {
        // Direct: lua.create_function(closure)
        hir::ExprKind::MethodCall(segment, _receiver, args, _span) => {
            let method = segment.ident.name.as_str();
            if matches!(
                method,
                "create_function"
                    | "create_function_mut"
                    | "create_function_with"
                    | "create_async_function"
            ) && !args.is_empty()
            {
                let closure_expr = args.first()?;
                let is_async = method.starts_with("create_async_");
                // Try inline closure first, then named function reference
                let (params, returns) = extract_standalone_closure_signature(tcx, closure_expr)
                    .or_else(|| extract_named_fn_callback_signature(tcx, closure_expr))?;
                let func = LuaFunction {
                    name: name.to_string(),
                    is_async,
                    params,
                    returns,
                    doc: None,
                };
                trace_global_function_snapshot("try_extract_create_function", &func);
                return Some(func);
            }
            None
        }
        _ => None,
    }
}

/// Extract params and return types from a standalone closure (not on UserData).
/// Closure signature: `|lua: &Lua, (p1, p2, ...): (T1, T2, ...)| -> Result<R, Error>`
pub(in crate::extract) fn extract_standalone_closure_signature(
    tcx: TyCtxt<'_>,
    closure_expr: &hir::Expr<'_>,
) -> Option<(Vec<LuaParam>, Vec<LuaReturn>)> {
    let closure_snippet = expr_snippet(tcx, closure_expr);
    let trace_target = ["UrlRef", "PathRef", "raw"]
        .iter()
        .any(|name| closure_snippet.contains(name));
    let hir::ExprKind::Closure(closure) = &closure_expr.kind else {
        return None;
    };

    let closure_def_id = closure.def_id;
    let closure_ty = tcx.type_of(closure_def_id).skip_binder();

    let ty::TyKind::Closure(_, closure_args) = closure_ty.kind() else {
        return None;
    };

    let sig = closure_args.as_closure().sig();
    let sig = tcx.liberate_late_bound_regions(closure_def_id.into(), sig);

    let inputs = sig.inputs();
    // create_function closures: |lua, (params)| → inputs are [&Lua, (P1, P2, ...)]
    let inner_inputs = if inputs.len() == 1 {
        if let ty::TyKind::Tuple(fields) = inputs[0].kind() {
            fields.as_slice()
        } else {
            inputs
        }
    } else {
        inputs
    };

    let body = tcx.hir_body(closure.body);
    let hir_param_names = if body.params.len() > 1 {
        extract_names_from_pat(body.params[1].pat)
    } else {
        Vec::new()
    };

    if trace_target {
        trace(format!(
            "extract_standalone_closure_signature start body_params={} inner_inputs={} hir_names={hir_param_names:?} closure={closure_snippet}",
            body.params.len(),
            inner_inputs.len(),
        ));
    }

    let params = if inner_inputs.len() > 1 {
        explicit_lua_params_from_hir(tcx, body, 1)
            .unwrap_or_else(|| extract_params_from_tuple(tcx, inner_inputs[1], &hir_param_names))
    } else {
        Vec::new()
    };

    let ret_ty = sig.output();
    let mut returns = map_return_ty(tcx, ret_ty);

    // When the signature returns Any (Value) or MultiValue, try to infer concrete types from body
    let body = tcx.hir_body(closure.body);
    merge_inferred_returns(&mut returns, infer_multi_returns_from_body(tcx, body.value));

    let mut params = params;
    refine_params_from_explicit_types(tcx, &closure_snippet, body, 1, 0, &mut params);
    refine_params_from_body(tcx, body.value, &mut params);

    if returns.len() == 1
        && matches!(returns[0].ty, LuaType::Class(_))
        && terminal_return_is_opaque_any_userdata(tcx, body.value)
    {
        returns = vec![LuaType::Any.into()];
    }

    // Enrich multi-returns with names from the body's tuple expression
    enrich_return_names(tcx, body.value, &mut returns);

    if let Some(self_name) = enclosing_impl_self_type_name(tcx, closure_def_id) {
        for param in &mut params {
            replace_self_class_alias(&mut param.ty, &self_name);
        }
        for ret in &mut returns {
            replace_self_return_sentinel(&mut ret.ty, &self_name);
            replace_self_class_alias(&mut ret.ty, &self_name);
        }
    }

    if trace_target {
        trace(format!(
            "extract_standalone_closure_signature final params={params:?} returns={returns:?} closure={closure_snippet}"
        ));
    }

    Some((params, returns))
}

/// Extract params and returns from a named function reference used as an mlua
/// callback, e.g. `lua.create_function(hostname)?` where `hostname` is:
/// ```ignore
/// fn hostname(_lua: &Lua, _: ()) -> mlua::Result<String> { ... }
/// ```
pub(in crate::extract) fn extract_named_fn_callback_signature<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<(Vec<LuaParam>, Vec<LuaReturn>)> {
    // Resolve the expression to a function DefId
    let def_id = match &expr.kind {
        hir::ExprKind::Path(qpath) => {
            let typeck = tcx.typeck(expr.hir_id.owner.def_id);
            let res = typeck.qpath_res(qpath, expr.hir_id);
            match res {
                rustc_hir::def::Res::Def(rustc_hir::def::DefKind::Fn, def_id) => def_id,
                rustc_hir::def::Res::Def(rustc_hir::def::DefKind::AssocFn, def_id) => def_id,
                _ => return None,
            }
        }
        _ => return None,
    };

    // Get the function signature from the type system
    let sig = tcx.fn_sig(def_id).skip_binder();
    let sig = tcx.liberate_late_bound_regions(def_id, sig);

    let inputs = sig.inputs();
    // mlua callback pattern: fn(_lua: &Lua, params) -> Result<Return>
    // Skip the first arg (&Lua), extract from the second
    if inputs.len() < 2 {
        return None;
    }

    let param_ty = inputs[1];
    let params = match param_ty.kind() {
        // () → no params
        ty::TyKind::Tuple(fields) if fields.is_empty() => Vec::new(),
        // (P1, P2, ...) → multiple params
        ty::TyKind::Tuple(fields) => {
            // Try to get param names from the HIR body if local
            let hir_names = def_id
                .as_local()
                .and_then(|local| {
                    let node = tcx.hir_node_by_def_id(local);
                    match node {
                        rustc_hir::Node::Item(item) => {
                            if let hir::ItemKind::Fn { body, .. } = item.kind {
                                let body = tcx.hir_body(body);
                                // Second param is the args tuple
                                body.params.get(1).map(|p| extract_names_from_pat(p.pat))
                            } else {
                                None
                            }
                        }
                        _ => None,
                    }
                })
                .unwrap_or_default();

            fields
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let ty = map_ty_to_lua(tcx, t);
                    let name = hir_names
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| format!("p{}", i + 1));
                    LuaParam { name, ty }
                })
                .collect()
        }
        // Single non-tuple param
        _ => {
            let ty = map_ty_to_lua(tcx, param_ty);
            let name = def_id
                .as_local()
                .and_then(|local| {
                    let node = tcx.hir_node_by_def_id(local);
                    match node {
                        rustc_hir::Node::Item(item) => {
                            if let hir::ItemKind::Fn { body, .. } = item.kind {
                                let body = tcx.hir_body(body);
                                body.params
                                    .get(1)
                                    .and_then(|p| extract_names_from_pat(p.pat).into_iter().next())
                            } else {
                                None
                            }
                        }
                        _ => None,
                    }
                })
                .unwrap_or_else(|| "p1".to_string());
            vec![LuaParam { name, ty }]
        }
    };

    let ret_ty = sig.output();
    let mut returns = map_return_ty(tcx, ret_ty);

    // For async fns, fn_sig returns impl Future which may not resolve.
    // If returns are empty/uninformative, try the HIR return type annotation.
    if returns.iter().all(|r| !is_informative(&r.ty))
        && let Some(local) = def_id.as_local()
        && let rustc_hir::Node::Item(item) = tcx.hir_node_by_def_id(local)
        && let hir::ItemKind::Fn { sig: fn_sig, .. } = &item.kind
        && let hir::FnRetTy::Return(ret_hir_ty) = &fn_sig.decl.output
    {
        // Try to resolve via HIR type structure (unwrapping Result<T>)
        if let Some(lua_ty) = unwrap_hir_result_type(tcx, ret_hir_ty) {
            if lua_ty == LuaType::Nil {
                returns = vec![];
            } else if is_informative(&lua_ty) {
                returns = vec![lua_ty.into()];
            }
        }
    }

    Some((params, returns))
}
