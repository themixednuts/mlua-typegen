use mlua_typegen::{LuaField, LuaType, make_union};
use rustc_hir as hir;
use rustc_middle::ty::{self, TyCtxt};

use super::{
    expr_def_id, expr_snippet, extract_string_literal, find_local_binding_init,
    infer_concrete_type_from_body, infer_expr_lua_type, infer_field_closure_result,
    infer_local_fn_body_return, infer_local_table_binding_type, infer_mapped_method_result,
    infer_userdata_binding_type_from_body, infer_wrapper_method_lua_type, is_any_user_data,
    is_better_inferred_type, is_informative, is_passthrough_method, local_binding_info,
    map_ty_to_lua, peel_expr, peel_lua_conversion_expr, peel_try_expr, readonly_field,
    resolve_any_user_data_class, resolve_local_method_def_id, rewrap_erased_body_inference,
    should_prefer_body_inference, should_trace_field_expr_snippet, should_trace_field_name, trace,
    try_lua_value_constructor, unwrap_result_ty, writable_field,
};

pub(super) fn find_into_lua_receiver_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let expr = peel_expr(expr);

    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            let name = segment.ident.name.as_str();
            if name == "into_lua" || name == "into_lua_multi" {
                if let Some(ty) = infer_wrapper_method_lua_type(tcx, receiver)
                    && is_informative(&ty)
                {
                    return Some(ty);
                }
                let typeck = tcx.typeck(receiver.hir_id.owner.def_id);
                let receiver_ty = typeck.expr_ty(receiver);
                let lua_ty = map_ty_to_lua(tcx, receiver_ty);
                if should_trace_field_expr_snippet(&expr_snippet(tcx, receiver)) {
                    let adt_path = match receiver_ty.kind() {
                        ty::TyKind::Adt(adt_def, _) => Some(tcx.def_path_str(adt_def.did())),
                        _ => None,
                    };
                    trace(format!(
                        "find_into_lua_receiver_type receiver={} rust_ty={receiver_ty:?} adt_path={adt_path:?} lua_ty={lua_ty:?}",
                        expr_snippet(tcx, receiver),
                    ));
                }
                if is_informative(&lua_ty) {
                    return Some(lua_ty);
                }
                // If receiver is AnyUserData, try to resolve the class from the
                // expression that produced it (e.g. TypeName::make(...))
                if is_any_user_data(tcx, receiver_ty)
                    && let Some(class) = resolve_any_user_data_class(tcx, receiver)
                {
                    return Some(class);
                }
            }
            // Recurse into receiver and args
            if let Some(ty) = find_into_lua_receiver_type(tcx, receiver) {
                return Some(ty);
            }
            for arg in args.iter() {
                if let Some(ty) = find_into_lua_receiver_type(tcx, arg) {
                    return Some(ty);
                }
            }
            None
        }
        hir::ExprKind::Block(block, _) => {
            // Check tail and local bindings that can feed it. Ignore side-effect
            // statements so we don't infer return types from unrelated calls.
            if let Some(ty) = block.expr.and_then(|e| find_into_lua_receiver_type(tcx, e)) {
                return Some(ty);
            }

            for stmt in block.stmts {
                if let hir::StmtKind::Let(hir::LetStmt { init: Some(e), .. }) = &stmt.kind
                    && let Some(ty) = find_into_lua_receiver_type(tcx, e)
                {
                    return Some(ty);
                }
            }
            None
        }
        hir::ExprKind::Match(scrut, arms, _) => {
            // Check scrutinee first (for ? operator desugaring)
            if let Some(ty) = find_into_lua_receiver_type(tcx, scrut) {
                return Some(ty);
            }
            // Collect types from all arms to build union
            let mut branch_types: Vec<LuaType> = Vec::new();
            for arm in arms.iter() {
                if let Some(ty) = find_into_lua_receiver_type(tcx, arm.body) {
                    branch_types.push(ty);
                }
            }
            if branch_types.is_empty() {
                None
            } else if branch_types.len() == 1 {
                Some(branch_types.into_iter().next().unwrap())
            } else {
                Some(make_union(branch_types))
            }
        }
        hir::ExprKind::Call(callee, args) => {
            // Check for LuaValue::Variant(...) constructor
            let typeck = tcx.typeck(expr.hir_id.owner.def_id);
            if let Some(ty) = try_lua_value_constructor(tcx, typeck, expr) {
                return Some(ty);
            }
            if let Some(ty) = find_into_lua_receiver_type(tcx, callee) {
                return Some(ty);
            }
            for arg in args.iter() {
                if let Some(ty) = find_into_lua_receiver_type(tcx, arg) {
                    return Some(ty);
                }
            }
            None
        }
        // LuaValue::Nil (path, no args)
        hir::ExprKind::Path(_) => {
            if let Some(init) = find_local_binding_init(tcx, expr) {
                let init = peel_lua_conversion_expr(init);
                if let Some(ty) = find_into_lua_receiver_type(tcx, init) {
                    return Some(ty);
                }
            }
            let typeck = tcx.typeck(expr.hir_id.owner.def_id);
            try_lua_value_constructor(tcx, typeck, expr)
        }
        hir::ExprKind::Closure(inner_closure) => {
            // Cross closure boundary — the concrete type may be inside
            let inner_body = tcx.hir_body(inner_closure.body);
            find_into_lua_receiver_type(tcx, inner_body.value)
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            if let Some(ty) = find_into_lua_receiver_type(tcx, cond) {
                return Some(ty);
            }
            // Collect from both branches
            let mut branch_types: Vec<LuaType> = Vec::new();
            if let Some(ty) = find_into_lua_receiver_type(tcx, then_expr) {
                branch_types.push(ty);
            }
            if let Some(else_e) = else_expr
                && let Some(ty) = find_into_lua_receiver_type(tcx, else_e)
            {
                branch_types.push(ty);
            }
            if branch_types.is_empty() {
                None
            } else if branch_types.len() == 1 {
                Some(branch_types.into_iter().next().unwrap())
            } else {
                Some(make_union(branch_types))
            }
        }
        _ => None,
    }
}

pub(super) fn infer_from_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    typeck: &'tcx ty::TypeckResults<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);
    let expr_text = expr_snippet(tcx, expr);

    let inferred = match &expr.kind {
        // .into_lua(lua) — the receiver has the concrete type
        hir::ExprKind::MethodCall(segment, receiver, _, _)
            if segment.ident.name.as_str() == "into_lua"
                || segment.ident.name.as_str() == "into_lua_multi" =>
        {
            let receiver_ty = typeck.expr_ty(receiver);
            let lua_ty = map_ty_to_lua(tcx, receiver_ty);
            if is_informative(&lua_ty) {
                return Some(lua_ty);
            }
            // Recurse into receiver in case of chained calls
            infer_from_expr(tcx, typeck, receiver)
        }

        // Ok(expr) — unwrap and check the inner expression
        // LuaValue::Variant(...) — direct Value construction
        hir::ExprKind::Call(callee, args) => {
            if let hir::ExprKind::Path(hir::QPath::Resolved(_, path)) = &callee.kind
                && let Some(seg) = path.segments.last()
                && seg.ident.name.as_str() == "Ok"
                && args.len() == 1
            {
                return infer_from_expr(tcx, typeck, &args[0]);
            }
            if let Some(local) = expr_def_id(tcx, callee).and_then(|def_id| def_id.as_local())
                && let Some(inferred) = infer_local_fn_body_return(tcx, local)
            {
                let call_ty = map_ty_to_lua(tcx, typeck.expr_ty(expr));
                if should_prefer_body_inference(&call_ty, &inferred) {
                    return Some(inferred);
                }
            }
            let callee_inferred = match &callee.kind {
                hir::ExprKind::Cast(inner, _) => infer_from_expr(tcx, typeck, inner),
                hir::ExprKind::Closure(_) => infer_from_expr(tcx, typeck, callee),
                _ => None,
            };
            if let Some(inferred) = callee_inferred {
                let call_ty = map_ty_to_lua(tcx, typeck.expr_ty(expr));
                if should_prefer_body_inference(&call_ty, &inferred) {
                    return Some(inferred);
                }
            }
            // Check for LuaValue::Variant(...) constructor
            if let Some(ty) = try_lua_value_constructor(tcx, typeck, expr) {
                return Some(ty);
            }
            // For function calls, check the return type
            let call_ty = typeck.expr_ty(expr);
            let lua_ty = map_ty_to_lua(tcx, call_ty);
            if is_informative(&lua_ty) || matches!(lua_ty, LuaType::Variadic(_)) {
                Some(lua_ty)
            } else {
                None
            }
        }

        // Method call chains: expr.method().method2() — check the overall type
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            if let Some(ty) = infer_wrapper_method_lua_type(tcx, expr)
                && is_informative(&ty)
            {
                return Some(ty);
            }

            let method_name = segment.ident.name.as_str();
            let call_ty = typeck.expr_ty(expr);
            let lua_ty = map_ty_to_lua(tcx, call_ty);
            if matches!(method_name, "map" | "and_then")
                && let Some(mapped) =
                    infer_mapped_method_result(tcx, typeck, receiver, args, method_name)
                && should_prefer_body_inference(&lua_ty, &mapped)
            {
                return Some(rewrap_erased_body_inference(&lua_ty, &mapped));
            }
            if let Some(local) = resolve_local_method_def_id(tcx, typeck, expr, receiver)
                && let Some(inferred) = infer_local_fn_body_return(tcx, local)
                && should_prefer_body_inference(&lua_ty, &inferred)
            {
                return Some(inferred);
            }
            if is_informative(&lua_ty) {
                return Some(lua_ty);
            }
            if matches!(lua_ty, LuaType::Variadic(_)) {
                return Some(lua_ty);
            }
            // For scoped borrow methods (borrow_mut_scoped, borrow_scoped),
            // the concrete type is inside the closure argument's body.
            if method_name.contains("scoped") || method_name.contains("scope") {
                for arg in args.iter() {
                    if let hir::ExprKind::Closure(inner_closure) = &arg.kind {
                        let inner_body = tcx.hir_body(inner_closure.body);
                        // Inner closure has its own typeck owner
                        let inner_typeck = tcx.typeck(inner_closure.def_id);
                        if let Some(ty) = infer_from_expr(tcx, inner_typeck, inner_body.value) {
                            return Some(ty);
                        }
                    }
                }
            }
            if is_passthrough_method(method_name) {
                infer_from_expr(tcx, typeck, receiver)
            } else {
                None
            }
        }

        // Closure body is a block — check the tail expression
        hir::ExprKind::Block(block, _) => {
            if let Some(tail) = block.expr {
                return infer_from_expr(tcx, typeck, tail);
            }
            // Check last statement if it's an expression
            if let Some(stmt) = block.stmts.last()
                && let hir::StmtKind::Semi(e) | hir::StmtKind::Expr(e) = &stmt.kind
            {
                return infer_from_expr(tcx, typeck, e);
            }
            None
        }

        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            let inner_typeck = tcx.typeck(closure.def_id);
            let inferred = infer_from_expr(tcx, inner_typeck, body.value);
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

        // Match expressions — collect types from ALL arms and build union
        hir::ExprKind::Match(_, arms, _) => {
            let mut branch_types: Vec<LuaType> = Vec::new();
            for arm in arms.iter() {
                if let Some(ty) = infer_from_expr(tcx, typeck, arm.body) {
                    branch_types.push(ty);
                }
            }
            if branch_types.is_empty() {
                None
            } else if branch_types.len() == 1 {
                Some(branch_types.into_iter().next().unwrap())
            } else {
                Some(make_union(branch_types))
            }
        }

        // If/else — collect from both branches
        hir::ExprKind::If(_, then_expr, else_expr) => {
            let mut branch_types: Vec<LuaType> = Vec::new();
            if let Some(ty) = infer_from_expr(tcx, typeck, then_expr) {
                branch_types.push(ty);
            }
            if let Some(else_e) = else_expr
                && let Some(ty) = infer_from_expr(tcx, typeck, else_e)
            {
                branch_types.push(ty);
            }
            if branch_types.is_empty() {
                None
            } else if branch_types.len() == 1 {
                Some(branch_types.into_iter().next().unwrap())
            } else {
                Some(make_union(branch_types))
            }
        }

        // LuaValue::Variant path (no args, e.g. LuaValue::Nil)
        hir::ExprKind::Path(_) => {
            if let Some(inferred) =
                local_binding_info(tcx, expr).and_then(|(_target, body, binding)| {
                    infer_userdata_binding_type_from_body(tcx, body.value, &binding)
                })
            {
                let ty = typeck.expr_ty(expr);
                let lua_ty = map_ty_to_lua(tcx, ty);
                if is_better_inferred_type(&lua_ty, &inferred) || !is_informative(&lua_ty) {
                    return Some(inferred);
                }
            }
            if let Some(inferred) = infer_local_table_binding_type(tcx, expr) {
                let ty = typeck.expr_ty(expr);
                let lua_ty = map_ty_to_lua(tcx, ty);
                if is_better_inferred_type(&lua_ty, &inferred) || !is_informative(&lua_ty) {
                    return Some(inferred);
                }
            }
            if let Some(init) = find_local_binding_init(tcx, expr) {
                let init = peel_lua_conversion_expr(init);
                let init_typeck = tcx.typeck(init.hir_id.owner.def_id);
                if let Some(inferred) = infer_from_expr(tcx, init_typeck, init) {
                    let ty = typeck.expr_ty(expr);
                    let lua_ty = map_ty_to_lua(tcx, ty);
                    if is_better_inferred_type(&lua_ty, &inferred) || !is_informative(&lua_ty) {
                        return Some(inferred);
                    }
                }
            }
            if let Some(ty) = try_lua_value_constructor(tcx, typeck, expr) {
                return Some(ty);
            }
            let ty = typeck.expr_ty(expr);
            let lua_ty = map_ty_to_lua(tcx, ty);
            if is_informative(&lua_ty) {
                Some(lua_ty)
            } else {
                None
            }
        }

        // Direct struct/value construction — use typeck
        _ => {
            let ty = typeck.expr_ty(expr);
            let lua_ty = map_ty_to_lua(tcx, ty);
            if is_informative(&lua_ty) {
                Some(lua_ty)
            } else {
                None
            }
        }
    };

    if should_trace_field_expr_snippet(&expr_text) {
        let expr_ty = typeck.expr_ty(expr);
        trace(format!(
            "infer_from_expr expr={} rust_ty={:?} lua_ty={:?} inferred={inferred:?}",
            expr_text,
            expr_ty,
            map_ty_to_lua(tcx, expr_ty),
        ));
    }

    inferred
}

// ── Field extraction ───────────────────────────────────────────────────

pub(super) fn record_field_registration<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    fields: &mut Vec<LuaField>,
) {
    let hir::ExprKind::MethodCall(segment, _receiver, args, _span) = &expr.kind else {
        return;
    };
    match segment.ident.name.as_str() {
        "add_field_method_get" | "add_field_function_get" => {
            if let Some(field) = extract_field_getter(tcx, args) {
                if let Some(existing) = fields.iter_mut().find(|item| item.name == field.name) {
                    existing.ty = field.ty;
                } else {
                    fields.push(field);
                }
            }
        }
        "add_field_method_set" | "add_field_function_set" => {
            if let Some(name) = args.first().and_then(extract_string_literal) {
                if let Some(existing) = fields.iter_mut().find(|field| field.name == name) {
                    existing.writable = true;
                } else {
                    fields.push(writable_field(
                        name,
                        LuaType::UnsupportedStatic {
                            rust_type: "write-only userdata field".to_owned(),
                            reason: "the setter registration provides no readable Lua value type"
                                .to_owned(),
                        },
                    ));
                }
            }
        }
        "add_field" | "add_meta_field" => {
            if let Some(name) = args.first().and_then(extract_string_literal) {
                let ty = args.get(1).map_or_else(
                    || LuaType::UnsupportedStatic {
                        rust_type: "missing field value".to_owned(),
                        reason: "the field registration has no value expression".to_owned(),
                    },
                    |value| infer_expr_lua_type(tcx, value),
                );
                fields.push(readonly_field(name, ty));
            }
        }
        "add_meta_field_with" => {
            fields.extend(extract_field_getter(tcx, args));
        }
        _ => {}
    }
}

pub(super) fn extract_field_getter<'tcx>(
    tcx: TyCtxt<'tcx>,
    args: &'tcx [hir::Expr<'tcx>],
) -> Option<LuaField> {
    if args.len() < 2 {
        return None;
    }

    let name = extract_string_literal(args.first()?)?;
    let closure_expr = args.get(1)?;

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

    let ret_ty = unwrap_result_ty(tcx, sig.output());
    let mut ty = map_ty_to_lua(tcx, ret_ty);
    let body = tcx.hir_body(closure.body);
    let inferred_body_ty = infer_field_closure_result(tcx, body.value)
        .or_else(|| infer_concrete_type_from_body(tcx, body.value));
    let inferred_body_trace =
        should_trace_field_name(&name).then(|| format!("{inferred_body_ty:?}"));

    // When the closure returns Result<Value> (→ Any), try to infer the concrete
    // type by walking the closure body for constructor expressions. This handles
    // cached_field! macros where the type is erased to Value by .into_lua().
    if let Some(inferred) = inferred_body_ty
        && should_prefer_body_inference(&ty, &inferred)
    {
        ty = inferred;
    }

    if should_trace_field_name(&name) {
        trace(format!(
            "extract_field_getter name={name} sig_ty={:?} inferred={} final={ty:?} body={}",
            map_ty_to_lua(tcx, ret_ty),
            inferred_body_trace.as_deref().unwrap_or("None"),
            expr_snippet(tcx, body.value)
        ));
    }

    Some(readonly_field(name, ty))
}
