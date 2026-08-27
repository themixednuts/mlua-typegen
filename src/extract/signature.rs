use heck::ToSnakeCase;
use mlua_typegen::{LuaParam, LuaReturn, LuaType, MethodKind, make_union};
use rustc_hir as hir;
use rustc_hir::def_id::LocalDefId;
use rustc_middle::ty::{self, TyCtxt};

use super::{
    body_returns_named_userdata_self, contains_self_return_sentinel, enclosing_impl_self_type_name,
    enrich_return_names, enter_local_param_inference, expr_def_id, expr_snippet,
    extract_params_from_tuple, extract_string_literal, find_cross_module_call_in_hir,
    find_local_binding_init, infer_any_userdata_param_type_from_body,
    infer_callback_signature_from_body, infer_concrete_type_from_body, infer_from_expr,
    infer_from_lua_conversion_type, infer_json_like_param_type_from_body,
    infer_multi_returns_from_body, infer_param_type_from_body,
    infer_param_type_from_local_method_call, infer_table_param_type_from_body,
    infer_table_param_type_from_converter_calls, is_any_user_data, is_any_userdata_type_snippet,
    is_better_inferred_type, is_informative, is_passthrough_method, lua_type_from_extracted_name,
    map_return_ty, map_ty_to_lua, merge_body_inference, merge_inferred_returns,
    normalize_explicit_param_type, normalize_param_lua_type, owner_body, pat_to_name, peel_expr,
    peel_lua_conversion_expr, peel_try_expr, replace_self_class_alias,
    rewrap_erased_body_inference, self_return_sentinel, should_prefer_body_inference,
    should_prefer_explicit_param_type, should_trace_field_expr_snippet, should_trace_field_name,
    should_trace_method_name, split_top_level, trace, unwrap_result_ty,
};

pub(super) fn infer_local_fn_body_return<'tcx>(
    tcx: TyCtxt<'tcx>,
    def_id: LocalDefId,
) -> Option<LuaType> {
    let body = owner_body(tcx, def_id)?;
    let typeck = tcx.typeck(def_id);
    let inferred = infer_from_expr(tcx, typeck, body.value);
    let concrete = infer_concrete_type_from_body(tcx, body.value);

    match (inferred, concrete) {
        (Some(existing), Some(candidate))
            if should_prefer_body_inference(&existing, &candidate) =>
        {
            Some(rewrap_erased_body_inference(&existing, &candidate))
        }
        (Some(existing), _) => Some(existing),
        (None, some) => some,
    }
}

pub(super) fn infer_callable_return_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let expr = peel_expr(peel_try_expr(expr));
    let snippet = expr_snippet(tcx, expr);

    let inferred = match &expr.kind {
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            let typeck = tcx.typeck(closure.def_id);
            let inferred = infer_from_expr(tcx, typeck, body.value);
            let concrete = infer_concrete_type_from_body(tcx, body.value);
            match (inferred, concrete) {
                (Some(existing), Some(candidate))
                    if should_prefer_body_inference(&existing, &candidate) =>
                {
                    Some(rewrap_erased_body_inference(&existing, &candidate))
                }
                (Some(existing), _) => Some(existing),
                (None, some) => some,
            }
        }
        _ => expr_def_id(tcx, expr)
            .and_then(|def_id| def_id.as_local())
            .and_then(|local| infer_local_fn_body_return(tcx, local)),
    };

    if should_trace_field_expr_snippet(&snippet) {
        trace(format!(
            "infer_callable_return_type expr={} inferred={inferred:?}",
            snippet
        ));
    }

    inferred
}

pub(super) fn infer_mapped_method_result<'tcx>(
    tcx: TyCtxt<'tcx>,
    typeck: &'tcx ty::TypeckResults<'tcx>,
    receiver: &'tcx hir::Expr<'tcx>,
    args: &'tcx [hir::Expr<'tcx>],
    method_name: &str,
) -> Option<LuaType> {
    let mapper = args.first()?;
    let mapped = infer_callable_return_type(tcx, mapper)?;
    let receiver_ty = typeck.expr_ty(receiver);

    let ty::TyKind::Adt(adt_def, _) = receiver_ty.kind() else {
        return None;
    };

    let path = tcx.def_path_str(adt_def.did());
    let result = match method_name {
        "map" => {
            if path.ends_with("Option") {
                Some(LuaType::Optional(Box::new(mapped)))
            } else if path.ends_with("Result") {
                Some(mapped)
            } else {
                None
            }
        }
        "and_then" => {
            if path.ends_with("Option") {
                Some(match mapped {
                    LuaType::Optional(_) => mapped,
                    other => LuaType::Optional(Box::new(other)),
                })
            } else if path.ends_with("Result") {
                Some(mapped)
            } else {
                None
            }
        }
        _ => None,
    };

    let receiver_snippet = expr_snippet(tcx, receiver);
    let mapper_snippet = expr_snippet(tcx, mapper);
    if should_trace_field_expr_snippet(&receiver_snippet)
        || should_trace_field_expr_snippet(&mapper_snippet)
    {
        trace(format!(
            "infer_mapped_method_result method={method_name} receiver={} mapper={} result={result:?}",
            receiver_snippet, mapper_snippet
        ));
    }

    result
}

pub(super) fn infer_field_closure_result<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            let name = segment.ident.name.as_str();
            if matches!(name, "map" | "and_then") {
                let typeck = tcx.typeck(expr.hir_id.owner.def_id);
                if let Some(mapped) = infer_mapped_method_result(tcx, typeck, receiver, args, name)
                {
                    return Some(mapped);
                }
            }

            if name.contains("scoped") || name.contains("scope") {
                let mut best = None;
                for arg in args.iter() {
                    let hir::ExprKind::Closure(closure) = &arg.kind else {
                        continue;
                    };

                    let body = tcx.hir_body(closure.body);
                    let inferred = infer_field_closure_result(tcx, body.value)
                        .or_else(|| infer_concrete_type_from_body(tcx, body.value));
                    if let Some(candidate) = inferred {
                        best = merge_body_inference(best, candidate);
                    }
                }
                if best.is_some() {
                    return best;
                }
            }

            if is_passthrough_method(name) {
                infer_field_closure_result(tcx, receiver)
            } else {
                None
            }
        }
        hir::ExprKind::Call(callee, args) => {
            if let hir::ExprKind::Path(qpath) = &callee.kind {
                let name = match qpath {
                    hir::QPath::Resolved(_, path) => {
                        path.segments.last().map(|seg| seg.ident.name.as_str())
                    }
                    hir::QPath::TypeRelative(_, seg) => Some(seg.ident.name.as_str()),
                };
                if matches!(name, Some("Ok" | "Some")) && args.len() == 1 {
                    return infer_field_closure_result(tcx, &args[0]);
                }
            }
            infer_callable_return_type(tcx, callee)
        }
        hir::ExprKind::Block(block, _) => block
            .expr
            .and_then(|tail| infer_field_closure_result(tcx, tail)),
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            infer_field_closure_result(tcx, body.value)
        }
        hir::ExprKind::If(_, then_expr, else_expr) => {
            let mut best = infer_field_closure_result(tcx, then_expr);
            if let Some(else_expr) =
                else_expr.and_then(|expr| infer_field_closure_result(tcx, expr))
            {
                best = merge_body_inference(best, else_expr);
            }
            best
        }
        hir::ExprKind::Match(_, arms, _) => arms.iter().fold(None, |best, arm| {
            if let Some(candidate) = infer_field_closure_result(tcx, arm.body) {
                merge_body_inference(best, candidate)
            } else {
                best
            }
        }),
        hir::ExprKind::Path(_) => {
            if let Some(init) = find_local_binding_init(tcx, expr) {
                let init = peel_lua_conversion_expr(init);
                let inferred = infer_field_closure_result(tcx, init)
                    .or_else(|| infer_concrete_type_from_body(tcx, init));
                if should_trace_field_expr_snippet(&expr_snippet(tcx, init)) {
                    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
                    trace(format!(
                        "infer_field_closure_result path expr={} init={} existing={:?} inferred={inferred:?}",
                        expr_snippet(tcx, expr),
                        expr_snippet(tcx, init),
                        map_ty_to_lua(tcx, typeck.expr_ty(expr)),
                    ));
                }
                if let Some(candidate) = inferred {
                    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
                    let existing = map_ty_to_lua(tcx, typeck.expr_ty(expr));
                    if should_prefer_body_inference(&existing, &candidate)
                        || !is_informative(&existing)
                    {
                        return Some(rewrap_erased_body_inference(&existing, &candidate));
                    }
                }
            }

            let inferred = infer_callable_return_type(tcx, expr);
            if should_trace_field_expr_snippet(&expr_snippet(tcx, expr)) {
                trace(format!(
                    "infer_field_closure_result direct-path expr={} inferred={inferred:?}",
                    expr_snippet(tcx, expr)
                ));
            }
            inferred
        }
        _ => infer_callable_return_type(tcx, expr),
    }
}

pub(super) fn explicit_lua_type_from_body_param(
    tcx: TyCtxt<'_>,
    body: &hir::Body<'_>,
    param_index: usize,
) -> Option<LuaType> {
    let param = body.params.get(param_index)?;
    if param.ty_span == param.pat.span {
        return None;
    }

    let snippet = tcx.sess.source_map().span_to_snippet(param.ty_span).ok()?;
    let snippet = snippet.trim();
    (!snippet.is_empty())
        .then(|| normalize_explicit_param_type(lua_type_from_extracted_name(snippet)))
}

pub(super) fn infer_local_callable_param_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    def_id: LocalDefId,
    param_index: usize,
) -> Option<LuaType> {
    let _guard = enter_local_param_inference(def_id, param_index)?;
    let body = owner_body(tcx, def_id)?;
    let local_param = body.params.get(param_index)?;
    let local_param_name = pat_to_name(local_param.pat);

    let mut candidates = Vec::new();
    if let Some(explicit) = explicit_lua_type_from_body_param(tcx, body, param_index)
        && is_informative(&explicit)
    {
        candidates.push(explicit);
    }

    candidates.extend(
        [
            infer_param_type_from_body(tcx, body.value, &local_param_name),
            infer_param_type_from_local_method_call(tcx, body.value, &local_param_name),
            infer_any_userdata_param_type_from_body(tcx, body.value, &local_param_name),
            infer_table_param_type_from_body(tcx, body.value, &local_param_name),
            infer_table_param_type_from_converter_calls(tcx, body.value, &local_param_name),
            infer_json_like_param_type_from_body(tcx, body.value, &local_param_name),
        ]
        .into_iter()
        .flatten()
        .map(normalize_param_lua_type),
    );

    let mut best = candidates.into_iter().reduce(|best, candidate| {
        if is_better_inferred_type(&best, &candidate) || !is_informative(&best) {
            candidate
        } else {
            best
        }
    })?;

    if let Some(self_name) = enclosing_impl_self_type_name(tcx, def_id) {
        replace_self_class_alias(&mut best, &self_name);
    }

    let item_name = tcx.item_name(def_id.to_def_id());
    let name = item_name.as_str();
    if should_trace_method_name(name) || should_trace_field_name(&local_param_name) {
        trace(format!(
            "infer_local_callable_param_type def={def_id:?} name={name} param_index={param_index} local_param={local_param_name} ty={best:?} body={}",
            expr_snippet(tcx, body.value)
        ));
    }

    Some(best)
}

pub(super) fn extract_meta_method_name(tcx: TyCtxt<'_>, expr: &hir::Expr<'_>) -> Option<String> {
    if let Some(name) = extract_string_literal(expr) {
        return Some(name);
    }
    if let hir::ExprKind::Path(qpath) = &expr.kind {
        let res = tcx
            .typeck(expr.hir_id.owner.def_id)
            .qpath_res(qpath, expr.hir_id);
        if let rustc_hir::def::Res::Def(_, def_id) = res {
            let variant = tcx.def_path_str(def_id);
            let name = variant.rsplit("::").next()?;
            return Some(meta_method_lua_name(name));
        }
    }
    None
}

pub(super) fn meta_method_lua_name(variant: &str) -> String {
    let known = match variant {
        "Add" => "__add",
        "Sub" => "__sub",
        "Mul" => "__mul",
        "Div" => "__div",
        "Mod" => "__mod",
        "Pow" => "__pow",
        "Unm" => "__unm",
        "IDiv" => "__idiv",
        "BAnd" => "__band",
        "BOr" => "__bor",
        "BXor" => "__bxor",
        "BNot" => "__bnot",
        "Shl" => "__shl",
        "Shr" => "__shr",
        "Concat" => "__concat",
        "Len" => "__len",
        "Eq" => "__eq",
        "Lt" => "__lt",
        "Le" => "__le",
        "Index" => "__index",
        "NewIndex" => "__newindex",
        "Call" => "__call",
        "ToString" => "__tostring",
        "Pairs" => "__pairs",
        "IPairs" => "__ipairs",
        "Gt" => "__gt",
        "Ge" => "__ge",
        "Iter" => "__iter",
        "Close" => "__close",
        _ => {
            let name = format!("__{}", variant.to_snake_case());
            eprintln!("warning: unknown MetaMethod variant `{variant}`, emitting `{name}`");
            return name;
        }
    };
    known.to_string()
}

/// Extract closure parameter types and return types for a UserData method closure.
pub(super) fn extract_closure_signature(
    tcx: TyCtxt<'_>,
    closure_expr: &hir::Expr<'_>,
    kind: MethodKind,
) -> Option<(Vec<LuaParam>, Vec<LuaReturn>)> {
    let closure_snippet = expr_snippet(tcx, closure_expr);
    let trace_target = [
        "ends_with",
        "join",
        "starts_with",
        "strip_prefix",
        "raw",
        "UrlRef",
        "PathRef",
        "userdata_impl",
    ]
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

    // Closures pack all parameters into a single tuple type.
    // For add_method: the tuple is (&Lua, &Self, UserParams)
    // For add_function: the tuple is (&Lua, UserParams)
    let params_idx = match kind {
        MethodKind::Method => 2,
        MethodKind::Function => 1,
    };

    // Unwrap the outer tuple to get the individual elements
    let inner_inputs = if inputs.len() == 1 {
        if let ty::TyKind::Tuple(fields) = inputs[0].kind() {
            fields.as_slice()
        } else {
            inputs
        }
    } else {
        inputs
    };

    // Try to extract param names from HIR closure patterns
    let body = tcx.hir_body(closure.body);
    let hir_param_names = if params_idx < body.params.len() {
        extract_names_from_pat(body.params[params_idx].pat)
    } else {
        Vec::new()
    };

    if trace_target {
        trace(format!(
            "extract_closure_signature start kind={kind:?} params_idx={params_idx} body_params={} inner_inputs={} hir_names={hir_param_names:?} closure={closure_snippet}",
            body.params.len(),
            inner_inputs.len(),
        ));
    }

    let params = if params_idx < inner_inputs.len() {
        let user_params_ty = inner_inputs[params_idx];
        // For add_function/add_function_mut, the first param is AnyUserData (self).
        // Detect this and skip it from the Lua-visible params.
        if kind == MethodKind::Function {
            let first_is_any_user_data = match user_params_ty.kind() {
                // Single param: just AnyUserData (no extra args)
                ty::TyKind::Adt(adt_def, _) => {
                    tcx.def_path_str(adt_def.did()).ends_with("AnyUserData")
                }
                // Multiple params: (AnyUserData, actual_args...)
                ty::TyKind::Tuple(fields) if !fields.is_empty() => {
                    if let ty::TyKind::Adt(adt_def, _) = fields[0].kind() {
                        tcx.def_path_str(adt_def.did()).ends_with("AnyUserData")
                    } else {
                        false
                    }
                }
                _ => false,
            };

            if first_is_any_user_data {
                // Extract params after the AnyUserData self param
                let params = if let ty::TyKind::Tuple(fields) = user_params_ty.kind() {
                    explicit_lua_params_from_hir(tcx, body, params_idx)
                        .map(|params| params.into_iter().skip(1).collect())
                        .unwrap_or_else(|| {
                            let rest_names: Vec<String> =
                                hir_param_names.iter().skip(1).cloned().collect();
                            fields
                                .iter()
                                .skip(1)
                                .enumerate()
                                .map(|(i, field_ty)| LuaParam {
                                    name: rest_names
                                        .get(i)
                                        .cloned()
                                        .unwrap_or_else(|| format!("p{}", i + 1)),
                                    ty: map_ty_to_lua(tcx, field_ty),
                                })
                                .collect()
                        })
                } else {
                    // Single AnyUserData param → no Lua-visible params
                    Vec::new()
                };

                let ret_ty = unwrap_result_ty(tcx, sig.output());
                let mut returns = if is_any_user_data(tcx, ret_ty) {
                    // Builder pattern: returns AnyUserData (self) for chaining.
                    vec![self_return_sentinel().into()]
                } else {
                    map_return_ty(tcx, sig.output())
                };

                merge_inferred_returns(
                    &mut returns,
                    infer_multi_returns_from_body(tcx, body.value),
                );

                let mut params = params;
                refine_params_from_explicit_types(
                    tcx,
                    &closure_snippet,
                    body,
                    params_idx,
                    1,
                    &mut params,
                );
                refine_params_from_body(tcx, body.value, &mut params);

                if hir_param_names.first().is_some_and(|self_name| {
                    body_returns_named_userdata_self(body.value, self_name)
                }) && returns.len() == 1
                    && !contains_self_return_sentinel(&returns[0].ty)
                {
                    returns[0].ty = make_union(vec![self_return_sentinel(), returns[0].ty.clone()]);
                }

                enrich_return_names(tcx, body.value, &mut returns);
                if trace_target {
                    trace(format!(
                        "extract_closure_signature function-self final kind={kind:?} params={params:?} returns={returns:?} closure={closure_snippet}"
                    ));
                }
                return Some((params, returns));
            }
        }
        explicit_lua_params_from_hir(tcx, body, params_idx)
            .unwrap_or_else(|| extract_params_from_tuple(tcx, user_params_ty, &hir_param_names))
    } else {
        Vec::new()
    };

    let ret_ty = sig.output();
    let mut returns = map_return_ty(tcx, ret_ty);

    // When the signature returns Any (Value) or MultiValue, try to infer concrete types from body
    let body = tcx.hir_body(closure.body);
    merge_inferred_returns(&mut returns, infer_multi_returns_from_body(tcx, body.value));

    let mut params = params;
    refine_params_from_explicit_types(tcx, &closure_snippet, body, params_idx, 0, &mut params);
    refine_params_from_body(tcx, body.value, &mut params);

    // Enrich multi-returns with names from the body's tuple expression
    enrich_return_names(tcx, body.value, &mut returns);

    // Cross-crate lookup fallback: if returns are uninformative and the closure
    // calls a function retrieved from a module by name, emit a @lookup: marker.
    if returns.iter().all(|r| !is_informative(&r.ty))
        && let Some(func_name) = find_cross_module_call_in_hir(tcx, body.value)
    {
        returns = vec![LuaType::Class(format!("@lookup:{func_name}")).into()];
    }

    if trace_target {
        trace(format!(
            "extract_closure_signature final kind={kind:?} params={params:?} returns={returns:?} closure={closure_snippet}"
        ));
    }

    Some((params, returns))
}

/// Extract parameter names from a HIR pattern (e.g. a tuple destructuring pattern).
pub(super) fn extract_names_from_pat(pat: &hir::Pat<'_>) -> Vec<String> {
    match &pat.kind {
        hir::PatKind::Tuple(pats, _) => pats.iter().map(|p| pat_to_name(p)).collect(),
        hir::PatKind::Binding(_, _, ident, _) => {
            vec![ident.name.as_str().to_string()]
        }
        _ => Vec::new(),
    }
}

pub(super) fn refine_params_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    params: &mut [LuaParam],
) {
    for param in params.iter_mut() {
        // For Function-typed params, try to infer the full callback signature
        // by finding .call() / .call_async() invocations on this param in the body.
        if matches!(param.ty, LuaType::Function) {
            if let Some(sig) = infer_callback_signature_from_body(tcx, expr, &param.name) {
                param.ty = sig;
            }
            continue;
        }

        if !matches!(param.ty, LuaType::Any | LuaType::Table | LuaType::String) {
            continue;
        }

        let candidates: Vec<_> = [
            infer_param_type_from_body(tcx, expr, &param.name),
            infer_param_type_from_local_method_call(tcx, expr, &param.name),
            infer_any_userdata_param_type_from_body(tcx, expr, &param.name),
            infer_table_param_type_from_body(tcx, expr, &param.name),
            infer_table_param_type_from_converter_calls(tcx, expr, &param.name),
            infer_json_like_param_type_from_body(tcx, expr, &param.name),
            infer_from_lua_conversion_type(tcx, expr, &param.name),
        ]
        .into_iter()
        .flatten()
        .map(normalize_param_lua_type)
        .collect();

        if matches!(param.ty, LuaType::Any) && !candidates.is_empty() {
            param.ty = make_union(candidates);
            continue;
        }

        let mut best = param.ty.clone();
        for candidate in candidates {
            if is_better_inferred_type(&best, &candidate) || !is_informative(&best) {
                best = candidate;
            }
        }

        if is_better_inferred_type(&param.ty, &best) || !is_informative(&param.ty) {
            param.ty = best;
        }
    }
}

pub(super) fn refine_params_from_explicit_types<'tcx>(
    tcx: TyCtxt<'tcx>,
    closure_snippet: &str,
    body: &'tcx hir::Body<'tcx>,
    params_idx: usize,
    skip_leading: usize,
    params: &mut [LuaParam],
) {
    if params.is_empty() {
        return;
    }

    let explicit_types = explicit_param_types_from_body_param(tcx, body, params_idx, params.len())
        .or_else(|| {
            explicit_param_types_from_closure_snippet(closure_snippet, params_idx, params.len())
        });
    let Some(explicit_types) = explicit_types else {
        return;
    };

    for (param, explicit) in params
        .iter_mut()
        .zip(explicit_types.into_iter().skip(skip_leading))
    {
        if should_prefer_explicit_param_type(&param.ty, &explicit) {
            param.ty = explicit;
        }
    }
}

pub(super) fn explicit_lua_params_from_hir<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &'tcx hir::Body<'tcx>,
    params_idx: usize,
) -> Option<Vec<LuaParam>> {
    let param = body.params.get(params_idx)?;
    if param.ty_span == param.pat.span {
        return None;
    }

    let names = extract_names_from_pat(param.pat);
    let expected_len = names.len().max(1);

    // Prefer typeck-resolved types over snippet parsing
    let types =
        explicit_param_types_from_typeck(tcx, body, params_idx, expected_len).or_else(|| {
            let snippet = tcx.sess.source_map().span_to_snippet(param.ty_span).ok()?;
            explicit_param_types_from_type_snippet(&snippet, expected_len)
        })?;

    Some(
        types
            .into_iter()
            .enumerate()
            .map(|(i, ty)| LuaParam {
                name: names
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("p{}", i + 1)),
                ty,
            })
            .collect(),
    )
}

/// Extract param types from typeck instead of source text snippets.
pub(super) fn explicit_param_types_from_typeck<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &'tcx hir::Body<'tcx>,
    params_idx: usize,
    expected_len: usize,
) -> Option<Vec<LuaType>> {
    let param = body.params.get(params_idx)?;
    let typeck = tcx.typeck(param.pat.hir_id.owner.def_id);
    let ty = typeck.node_type(param.pat.hir_id);

    let types = match ty.kind() {
        ty::TyKind::Tuple(fields) if fields.len() == expected_len => {
            fields.iter().map(|t| map_ty_to_lua(tcx, t)).collect()
        }
        // Tuple has more fields than expected — take the trailing elements
        // (the leading fields were likely AnyUserData self params already stripped)
        ty::TyKind::Tuple(fields) if fields.len() > expected_len => fields
            .iter()
            .skip(fields.len() - expected_len)
            .map(|t| map_ty_to_lua(tcx, t))
            .collect(),
        _ if expected_len == 1 => vec![map_ty_to_lua(tcx, ty)],
        _ => return None,
    };

    // Only use typeck types if they're informative (not all Any)
    types.iter().any(is_informative).then_some(types)
}

pub(super) fn explicit_param_types_from_body_param<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &'tcx hir::Body<'tcx>,
    params_idx: usize,
    expected_len: usize,
) -> Option<Vec<LuaType>> {
    // Prefer typeck over snippet parsing
    explicit_param_types_from_typeck(tcx, body, params_idx, expected_len).or_else(|| {
        let param = body.params.get(params_idx)?;
        if param.ty_span == param.pat.span {
            return None;
        }
        let snippet = tcx.sess.source_map().span_to_snippet(param.ty_span).ok()?;
        explicit_param_types_from_type_snippet(&snippet, expected_len)
    })
}

pub(super) fn explicit_param_types_from_closure_snippet(
    closure_snippet: &str,
    params_idx: usize,
    expected_len: usize,
) -> Option<Vec<LuaType>> {
    let params = closure_params_snippet(closure_snippet)?;
    let param = split_top_level(params, ',').get(params_idx)?.trim();
    let ty = top_level_param_type_annotation(param)?;
    explicit_param_types_from_type_snippet(ty, expected_len)
}

pub(super) fn explicit_param_types_from_type_snippet(
    snippet: &str,
    expected_len: usize,
) -> Option<Vec<LuaType>> {
    let snippet = snippet.trim();
    if snippet.is_empty() || snippet == "()" {
        return None;
    }

    if expected_len == 1 && is_any_userdata_type_snippet(snippet) {
        return None;
    }

    if expected_len == 1 {
        return Some(vec![normalize_explicit_param_type(
            lua_type_from_extracted_name(snippet),
        )]);
    }

    let inner = snippet
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))?;
    let parts = split_top_level(inner, ',');
    (parts.len() == expected_len).then(|| {
        parts
            .into_iter()
            .map(|part| normalize_explicit_param_type(lua_type_from_extracted_name(part)))
            .collect()
    })
}

pub(super) fn closure_params_snippet(snippet: &str) -> Option<&str> {
    let start = snippet.find('|')?;
    let rest = &snippet[start + 1..];
    let end = rest.find('|')?;
    Some(rest[..end].trim())
}

pub(super) fn top_level_param_type_annotation(param: &str) -> Option<&str> {
    let mut angle = 0usize;
    let mut paren = 0usize;
    let mut bracket = 0usize;

    for (idx, ch) in param.char_indices() {
        match ch {
            '<' => angle += 1,
            '>' => angle = angle.saturating_sub(1),
            '(' => paren += 1,
            ')' => paren = paren.saturating_sub(1),
            '[' => bracket += 1,
            ']' => bracket = bracket.saturating_sub(1),
            ':' if angle == 0 && paren == 0 && bracket == 0 => {
                return Some(param[idx + ch.len_utf8()..].trim());
            }
            _ => {}
        }
    }

    None
}
