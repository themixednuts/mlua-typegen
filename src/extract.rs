use rustc_hir as hir;
use rustc_hir::attrs::AttributeKind;
use rustc_hir::def_id::LocalDefId;
use rustc_middle::ty::{self, TyCtxt};
use rustc_span::Symbol;

use heck::ToSnakeCase;
use mlua_typegen::{
    LuaApi, LuaClass, LuaEnum, LuaField, LuaFunction, LuaMethod, LuaModule, LuaParam, LuaType,
    MethodKind,
};
use mlua_typegen::typemap::map_rust_type;

/// Sentinel value used to mark "returns Self" during extraction.
/// `extract_class` replaces this with the actual class name.
fn self_return_sentinel() -> LuaType {
    LuaType::Class(String::new())
}

fn is_self_return_sentinel(ty: &LuaType) -> bool {
    matches!(ty, LuaType::Class(name) if name.is_empty())
}

/// Check whether a type is `mlua::AnyUserData` (possibly wrapped in `Result`).
fn is_any_user_data(tcx: TyCtxt<'_>, ty: ty::Ty<'_>) -> bool {
    if let ty::TyKind::Adt(adt_def, _) = ty.kind() {
        tcx.def_path_str(adt_def.did()).ends_with("AnyUserData")
    } else {
        false
    }
}

/// Walk statements in a loop body block, calling `$visitor` on each expression.
/// Uses a macro to avoid lifetime issues with `TyCtxt`'s invariant lifetime parameter.
macro_rules! walk_loop_body {
    ($block:expr, $visitor:expr) => {
        for stmt in $block.stmts {
            match &stmt.kind {
                hir::StmtKind::Semi(e) | hir::StmtKind::Expr(e) => {
                    $visitor(e);
                }
                _ => {}
            }
        }
        if let Some(e) = $block.expr {
            $visitor(e);
        }
    };
}

/// Extract doc comments (`#[doc = "..."]`) from HIR attributes for a given HirId.
fn extract_doc_comments(tcx: TyCtxt<'_>, hir_id: hir::HirId) -> Option<String> {
    let attrs = tcx.hir_attrs(hir_id);
    let mut lines = Vec::new();

    for attr in attrs {
        let doc_comment = extract_doc_from_attr(attr);
        if let Some(comment) = doc_comment {
            let line = comment.strip_prefix(' ').unwrap_or(&comment);
            lines.push(line.to_string());
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Extract doc comment text from a single HIR attribute.
fn extract_doc_from_attr(attr: &hir::Attribute) -> Option<String> {
    if let hir::Attribute::Parsed(AttributeKind::DocComment { comment, .. }) = attr {
        Some(comment.as_str().to_string())
    } else {
        None
    }
}

/// Extract doc comments for a type definition (struct/enum) by DefId.
fn extract_type_doc(tcx: TyCtxt<'_>, ty: ty::Ty<'_>) -> Option<String> {
    if let ty::TyKind::Adt(adt_def, _) = ty.kind() {
        let def_id = adt_def.did();
        if let Some(local_id) = def_id.as_local() {
            let hir_id = tcx.local_def_id_to_hir_id(local_id);
            return extract_doc_comments(tcx, hir_id);
        }
    }
    None
}

/// Collect the full Lua API surface from the crate.
pub fn collect_lua_api(tcx: TyCtxt<'_>) -> LuaApi {
    let mut api = LuaApi::default();

    for item_id in tcx.hir_crate_items(()).free_items() {
        let item = tcx.hir_item(item_id);

        match &item.kind {
            hir::ItemKind::Impl(impl_block) => {
                if let Some(trait_ref) = &impl_block.of_trait {
                    let trait_path = trait_ref
                        .trait_ref
                        .path
                        .res
                        .opt_def_id()
                        .map(|did| tcx.def_path_str(did));

                    if let Some(path) = &trait_path {
                        if is_userdata_trait(path) {
                            if let Some(class) =
                                extract_class(tcx, item.owner_id.def_id, impl_block)
                            {
                                api.classes.push(class);
                            }
                        } else if is_into_lua_trait(path) || is_from_lua_trait(path) {
                            // Check if this is an enum — if so, extract variant names
                            if let Some(lua_enum) =
                                extract_enum_from_lua_impl(tcx, item.owner_id.def_id)
                            {
                                // Deduplicate: only add if not already present
                                if !api.enums.iter().any(|e| e.name == lua_enum.name) {
                                    api.enums.push(lua_enum);
                                }
                            }
                        }
                    }
                }
            }
            hir::ItemKind::Fn { .. } => {
                // Check for #[mlua::lua_module] attribute
                if let Some(module) = extract_lua_module_fn(tcx, item) {
                    api.modules.push(module);
                }
                // Also look for functions that register Lua globals/modules
                extract_registrations_from_fn(tcx, item, &mut api);
            }
            _ => {}
        }
    }

    api
}

fn is_userdata_trait(path: &str) -> bool {
    path == "mlua::UserData"
        || path == "mlua::prelude::LuaUserData"
        || path.ends_with("::UserData")
}

fn is_into_lua_trait(path: &str) -> bool {
    path == "mlua::IntoLua"
        || path == "mlua::prelude::LuaIntoLua"
        || path.ends_with("::IntoLua")
}

fn is_from_lua_trait(path: &str) -> bool {
    path == "mlua::FromLua"
        || path == "mlua::prelude::LuaFromLua"
        || path.ends_with("::FromLua")
}

// ── #[mlua::lua_module] detection ───────────────────────────────────────

/// Detect `#[mlua::lua_module]` functions and extract the module they build.
fn extract_lua_module_fn<'tcx>(
    tcx: TyCtxt<'tcx>,
    item: &hir::Item<'tcx>,
) -> Option<LuaModule> {
    let attrs = tcx.hir_attrs(item.hir_id());
    let mut module_attr = None;

    for attr in attrs {
        if let hir::Attribute::Unparsed(attr_item) = attr {
            let segments: Vec<_> = attr_item.path.segments.iter().map(|s| s.as_str()).collect();
            if matches!(segments.as_slice(), ["mlua", "lua_module"] | ["lua_module"]) {
                module_attr = Some(attr_item);
                break;
            }
        }
    }

    module_attr.as_ref()?;

    // Get the module name from the attribute arg or function name
    let fn_name = item
        .kind
        .ident()
        .map(|id| id.name.as_str().to_string())
        .unwrap_or_else(|| "unknown_module".to_string());

    let module_name = module_attr
        .and_then(|a| extract_name_from_attr_args(&a.args))
        .unwrap_or(fn_name);

    let hir::ItemKind::Fn { body, .. } = &item.kind else {
        return None;
    };
    let body = tcx.hir_body(*body);

    let doc = extract_doc_comments(tcx, item.hir_id());

    let mut module = LuaModule {
        name: module_name,
        doc,
        functions: Vec::new(),
        fields: Vec::new(),
    };

    visit_expr_for_module_exports(tcx, body.value, &mut module);

    Some(module)
}

/// Try to extract `name = "value"` from attribute args.
fn extract_name_from_attr_args(args: &hir::AttrArgs) -> Option<String> {
    if let hir::AttrArgs::Delimited(delim) = args {
        // The tokens are stored as a TokenStream; do a simple string search
        let s = format!("{:?}", delim.tokens);
        parse_name_attr(&s)
    } else {
        None
    }
}

/// Simple parser for `name = "value"` in attribute arg strings.
fn parse_name_attr(s: &str) -> Option<String> {
    let idx = s.find("name")?;
    let rest = &s[idx + 4..];
    let rest = rest.trim().strip_prefix('=')?;
    let rest = rest.trim();
    // Find quoted string
    let start = rest.find('"')? + 1;
    let rest = &rest[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Walk an expression to find `table.set("name", value)` patterns for module exports.
fn visit_expr_for_module_exports<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    module: &mut LuaModule,
) {
    match &expr.kind {
        hir::ExprKind::Block(block, _) => {
            for stmt in block.stmts {
                match &stmt.kind {
                    hir::StmtKind::Semi(e) | hir::StmtKind::Expr(e) => {
                        visit_expr_for_module_exports(tcx, e, module);
                    }
                    hir::StmtKind::Let(local) => {
                        if let Some(init) = local.init {
                            visit_expr_for_module_exports(tcx, init, module);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(e) = block.expr {
                visit_expr_for_module_exports(tcx, e, module);
            }
        }
        hir::ExprKind::If(_, then_block, else_block) => {
            visit_expr_for_module_exports(tcx, then_block, module);
            if let Some(else_expr) = else_block {
                visit_expr_for_module_exports(tcx, else_expr, module);
            }
        }
        hir::ExprKind::Match(_, arms, _) => {
            for arm in *arms {
                visit_expr_for_module_exports(tcx, arm.body, module);
            }
        }
        hir::ExprKind::Loop(block, _, _, _) => {
            walk_loop_body!(block, |e| visit_expr_for_module_exports(tcx, e, module));
        }
        hir::ExprKind::MethodCall(segment, _receiver, args, _span) => {
            let method_name = segment.ident.name.as_str();
            if (method_name == "set" || method_name == "raw_set") && args.len() >= 2 {
                if let Some(name) = extract_string_literal(&args[0]) {
                    if let Some(func) = try_extract_create_function(tcx, &args[1], &name) {
                        module.functions.push(func);
                    } else {
                        // Non-function value being set on the module table
                        let ty = infer_expr_lua_type(tcx, &args[1]);
                        module.fields.push(LuaField {
                            name,
                            ty,
                            writable: true,
                            doc: None,
                        });
                    }
                }
            }
        }
        _ => {}
    }
}

// ── Enum extraction from IntoLua/FromLua impls ─────────────────────────

/// If the Self type of an IntoLua/FromLua impl is an enum, extract its variants.
fn extract_enum_from_lua_impl(tcx: TyCtxt<'_>, impl_def_id: LocalDefId) -> Option<LuaEnum> {
    let self_ty = tcx.type_of(impl_def_id).skip_binder();
    let ty::TyKind::Adt(adt_def, _) = self_ty.kind() else {
        return None;
    };

    if !adt_def.is_enum() {
        return None;
    }

    let name = type_display_name(tcx, self_ty);
    let variants: Vec<String> = adt_def
        .variants()
        .iter()
        .map(|v| variant_to_lua_string(v.name.as_str()))
        .collect();

    if variants.is_empty() {
        return None;
    }

    let doc = extract_type_doc(tcx, self_ty);

    Some(LuaEnum { name, doc, variants })
}

/// Convert a Rust enum variant name to a Lua-friendly string.
/// PascalCase → snake_case by default, since that's the common Lua convention.
/// Uses `heck` for proper handling of acronyms (e.g. `HTTPResponse` → `http_response`).
fn variant_to_lua_string(name: &str) -> String {
    name.to_snake_case()
}

// ── Function registration extraction ───────────────────────────────────

/// Scan a function body for patterns like:
/// - `lua.globals().set("name", lua.create_function(...))`
/// - `table.set("name", lua.create_function(...))`
fn extract_registrations_from_fn<'tcx>(
    tcx: TyCtxt<'tcx>,
    item: &hir::Item<'tcx>,
    api: &mut LuaApi,
) {
    let hir::ItemKind::Fn { body, .. } = &item.kind else {
        return;
    };
    let body = tcx.hir_body(*body);
    let mut ctx = RegistrationCtx { tcx, api };
    visit_expr_for_registrations(&mut ctx, body.value);
}

struct RegistrationCtx<'a, 'tcx> {
    tcx: TyCtxt<'tcx>,
    api: &'a mut LuaApi,
}

fn visit_expr_for_registrations<'tcx>(
    ctx: &mut RegistrationCtx<'_, 'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) {
    match &expr.kind {
        hir::ExprKind::Block(block, _) => {
            for stmt in block.stmts {
                match &stmt.kind {
                    hir::StmtKind::Semi(e) | hir::StmtKind::Expr(e) => {
                        visit_expr_for_registrations(ctx, e);
                    }
                    hir::StmtKind::Let(local) => {
                        if let Some(init) = local.init {
                            visit_expr_for_registrations(ctx, init);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(e) = block.expr {
                visit_expr_for_registrations(ctx, e);
            }
        }

        hir::ExprKind::If(_, then_block, else_block) => {
            visit_expr_for_registrations(ctx, then_block);
            if let Some(else_expr) = else_block {
                visit_expr_for_registrations(ctx, else_expr);
            }
        }
        hir::ExprKind::Match(_, arms, _) => {
            for arm in *arms {
                visit_expr_for_registrations(ctx, arm.body);
            }
        }
        hir::ExprKind::Loop(block, _, _, _) => {
            walk_loop_body!(block, |e| visit_expr_for_registrations(ctx, e));
        }

        // Match: <receiver>.set("name", <value>) or raw_set
        hir::ExprKind::MethodCall(segment, receiver, args, _span) => {
            let method_name = segment.ident.name.as_str();

            if (method_name == "set" || method_name == "raw_set") && args.len() >= 2 {
                if let Some(name) = extract_string_literal(&args[0]) {
                    let is_globals = is_globals_call(receiver);

                    // Check if the value being set is a create_function call
                    if let Some(func) = try_extract_create_function(ctx.tcx, &args[1], &name) {
                        if is_globals {
                            ctx.api.global_functions.push(func);
                        } else {
                            // It's being set on a table — try to find/create a module
                            let module_name = get_table_name(receiver)
                                .unwrap_or_else(|| "unknown".to_string());
                            let module = ctx
                                .api
                                .modules
                                .iter_mut()
                                .find(|m| m.name == module_name);
                            if let Some(module) = module {
                                module.functions.push(func);
                            } else {
                                ctx.api.modules.push(LuaModule {
                                    name: module_name,
                                    doc: None,
                                    functions: vec![func],
                                    fields: Vec::new(),
                                });
                            }
                        }
                    }
                }
            }

            // Recurse into receiver and args
            visit_expr_for_registrations(ctx, receiver);
            for arg in *args {
                visit_expr_for_registrations(ctx, arg);
            }
        }

        hir::ExprKind::Call(_, args) => {
            for arg in *args {
                visit_expr_for_registrations(ctx, arg);
            }
        }

        _ => {}
    }
}

/// Check if an expression is `lua.globals()` or `<something>.globals()`.
fn is_globals_call(expr: &hir::Expr<'_>) -> bool {
    if let hir::ExprKind::MethodCall(segment, _, _, _) = &expr.kind {
        return segment.ident.name.as_str() == "globals";
    }
    false
}

/// Try to get a name for a table variable (best effort).
fn get_table_name(expr: &hir::Expr<'_>) -> Option<String> {
    match &expr.kind {
        hir::ExprKind::Path(hir::QPath::Resolved(_, path)) => {
            path.segments.last().map(|s| s.ident.name.as_str().to_string())
        }
        _ => None,
    }
}

/// Try to extract a LuaFunction from a `lua.create_function(|lua, args| ...)` expression.
fn try_extract_create_function<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    name: &str,
) -> Option<LuaFunction> {
    // Unwrap `?` operator: the expression might be `lua.create_function(...)?.into_function()?`
    let expr = unwrap_try_expr(expr);

    match &expr.kind {
        // Direct: lua.create_function(closure)
        hir::ExprKind::MethodCall(segment, _receiver, args, _span) => {
            let method = segment.ident.name.as_str();
            if matches!(method,
                    "create_function" | "create_function_mut"
                    | "create_function_with"
                    | "create_async_function")
                && !args.is_empty()
            {
                let closure_expr = &args[0];
                let (params, returns) =
                    extract_standalone_closure_signature(tcx, closure_expr)?;
                let is_async = method.starts_with("create_async_");
                return Some(LuaFunction {
                    name: name.to_string(),
                    is_async,
                    params,
                    returns,
                    doc: None,
                });
            }
            None
        }
        _ => None,
    }
}

/// Unwrap `expr?` → `expr` (strip the Try/Match desugaring).
fn unwrap_try_expr<'tcx>(expr: &'tcx hir::Expr<'tcx>) -> &'tcx hir::Expr<'tcx> {
    // In HIR, `expr?` desugars to a match. Just try to look through it.
    if let hir::ExprKind::Match(scrutinee, _, hir::MatchSource::TryDesugar(_)) = &expr.kind {
        return unwrap_try_expr(scrutinee);
    }
    if let hir::ExprKind::Call(_, args) = &expr.kind {
        // Could be Result::from(expr?) — try the first arg
        if let Some(first) = args.first() {
            if let hir::ExprKind::Match(scrutinee, _, hir::MatchSource::TryDesugar(_)) =
                &first.kind
            {
                return unwrap_try_expr(scrutinee);
            }
        }
    }
    expr
}

/// Extract params and return types from a standalone closure (not on UserData).
/// Closure signature: `|lua: &Lua, (p1, p2, ...): (T1, T2, ...)| -> Result<R, Error>`
fn extract_standalone_closure_signature(
    tcx: TyCtxt<'_>,
    closure_expr: &hir::Expr<'_>,
) -> Option<(Vec<LuaParam>, Vec<LuaType>)> {
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
    let body = tcx.hir_body(closure.body);
    let hir_param_names = if body.params.len() > 1 {
        extract_names_from_pat(body.params[1].pat)
    } else {
        Vec::new()
    };

    let params = if inputs.len() > 1 {
        extract_params_from_tuple(tcx, inputs[1], &hir_param_names)
    } else {
        Vec::new()
    };

    let ret_ty = sig.output();
    let returns = map_return_ty(tcx, ret_ty);

    Some((params, returns))
}

// ── UserData class extraction (existing) ───────────────────────────────

/// Extract a LuaClass from a `impl UserData for T` block.
fn extract_class<'tcx>(
    tcx: TyCtxt<'tcx>,
    impl_def_id: LocalDefId,
    impl_block: &hir::Impl<'tcx>,
) -> Option<LuaClass> {
    let self_ty = tcx.type_of(impl_def_id).skip_binder();
    let class_name = type_display_name(tcx, self_ty);

    let mut fields = Vec::new();
    let mut methods = Vec::new();

    for impl_item_ref in impl_block.items {
        let impl_item_id = hir::ImplItemId {
            owner_id: impl_item_ref.owner_id,
        };
        let impl_item = tcx.hir_impl_item(impl_item_id);
        let name = impl_item.ident.name;

        if name == Symbol::intern("add_methods") {
            extract_methods_from_body(tcx, impl_item, &mut methods);
        } else if name == Symbol::intern("add_fields") {
            extract_fields_from_body(tcx, impl_item, &mut fields);
        }
    }

    let doc = extract_type_doc(tcx, self_ty);

    // Replace Self return sentinels with the actual class name.
    // These are set by extract_closure_signature when it detects the builder
    // pattern: add_function taking AnyUserData and returning AnyUserData.
    for method in &mut methods {
        for ret in &mut method.returns {
            if is_self_return_sentinel(ret) {
                *ret = LuaType::Class(class_name.clone());
            }
        }
    }

    Some(LuaClass {
        name: class_name,
        doc,
        fields,
        methods,
    })
}

/// Get a display name for a type (last path segment).
fn type_display_name(tcx: TyCtxt<'_>, ty: ty::Ty<'_>) -> String {
    match ty.kind() {
        ty::TyKind::Adt(adt_def, _) => {
            let path = tcx.def_path_str(adt_def.did());
            path.rsplit("::").next().unwrap_or(&path).to_string()
        }
        _ => format!("{ty}"),
    }
}

fn extract_methods_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    impl_item: &hir::ImplItem<'tcx>,
    methods: &mut Vec<LuaMethod>,
) {
    let hir::ImplItemKind::Fn(_sig, body_id) = impl_item.kind else {
        return;
    };
    let body = tcx.hir_body(body_id);
    visit_expr_for_methods(tcx, body.value, methods);
}

fn visit_expr_for_methods<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    methods: &mut Vec<LuaMethod>,
) {
    match &expr.kind {
        hir::ExprKind::Block(block, _) => {
            for stmt in block.stmts {
                match &stmt.kind {
                    hir::StmtKind::Semi(e) | hir::StmtKind::Expr(e) => {
                        visit_expr_for_methods(tcx, e, methods);
                    }
                    hir::StmtKind::Let(local) => {
                        if let Some(init) = local.init {
                            visit_expr_for_methods(tcx, init, methods);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(e) = block.expr {
                visit_expr_for_methods(tcx, e, methods);
            }
        }
        // Walk into if/match/let blocks to find conditional method registrations
        hir::ExprKind::If(_, then_block, else_block) => {
            visit_expr_for_methods(tcx, then_block, methods);
            if let Some(else_expr) = else_block {
                visit_expr_for_methods(tcx, else_expr, methods);
            }
        }
        hir::ExprKind::Match(_, arms, _) => {
            for arm in *arms {
                visit_expr_for_methods(tcx, arm.body, methods);
            }
        }
        // Walk into loop bodies
        hir::ExprKind::Loop(block, _, _, _) => {
            walk_loop_body!(block, |e| visit_expr_for_methods(tcx, e, methods));
        }
        // Walk into closure bodies (e.g. immediately-invoked closures)
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            visit_expr_for_methods(tcx, body.value, methods);
        }
        // Walk into call args (handles IIFE patterns and chained calls)
        hir::ExprKind::Call(callee, args) => {
            visit_expr_for_methods(tcx, callee, methods);
            for arg in *args {
                visit_expr_for_methods(tcx, arg, methods);
            }
        }
        hir::ExprKind::MethodCall(segment, _receiver, args, _span) => {
            let method_name = segment.ident.name.as_str();
            let is_async = method_name.starts_with("add_async_");
            match method_name {
                // All method variants (immutable, mutable, once, async)
                "add_method" | "add_method_mut" | "add_method_once"
                | "add_async_method" | "add_async_method_mut" | "add_async_method_once" => {
                    if let Some(m) = extract_single_method(tcx, args, MethodKind::Method, is_async) {
                        methods.push(m);
                    }
                }
                // All function variants (immutable, mutable, async)
                "add_function" | "add_function_mut"
                | "add_async_function" => {

                    if let Some(m) = extract_single_method(tcx, args, MethodKind::Function, is_async) {
                        methods.push(m);
                    }
                }
                // All meta method variants
                "add_meta_method" | "add_meta_method_mut"
                | "add_async_meta_method" | "add_async_meta_method_mut" => {
                    if let Some(m) = extract_meta_method(tcx, args, MethodKind::Method, is_async) {
                        methods.push(m);
                    }
                }
                // All meta function variants
                "add_meta_function" | "add_meta_function_mut"
                | "add_async_meta_function" => {
                    if let Some(m) = extract_meta_method(tcx, args, MethodKind::Function, is_async) {
                        methods.push(m);
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn extract_single_method<'tcx>(
    tcx: TyCtxt<'tcx>,
    args: &'tcx [hir::Expr<'tcx>],
    kind: MethodKind,
    is_async: bool,
) -> Option<LuaMethod> {
    if args.len() < 2 {
        return None;
    }

    let name = extract_string_literal(&args[0])?;
    let closure_expr = &args[1];
    let (params, returns) = extract_closure_signature(tcx, closure_expr, kind)?;

    Some(LuaMethod {
        name,
        kind,
        is_async,
        params,
        returns,
        doc: None,
    })
}

fn extract_meta_method<'tcx>(
    tcx: TyCtxt<'tcx>,
    args: &'tcx [hir::Expr<'tcx>],
    kind: MethodKind,
    is_async: bool,
) -> Option<LuaMethod> {
    if args.len() < 2 {
        return None;
    }

    let name = extract_meta_method_name(tcx, &args[0])?;
    let closure_expr = &args[1];
    let (params, returns) = extract_closure_signature(tcx, closure_expr, kind)?;

    Some(LuaMethod {
        name,
        kind,
        is_async,
        params,
        returns,
        doc: None,
    })
}

fn extract_string_literal(expr: &hir::Expr<'_>) -> Option<String> {
    if let hir::ExprKind::Lit(lit) = &expr.kind {
        if let rustc_ast::ast::LitKind::Str(sym, _) = &lit.node {
            return Some(sym.as_str().to_string());
        }
    }
    None
}

fn extract_meta_method_name(tcx: TyCtxt<'_>, expr: &hir::Expr<'_>) -> Option<String> {
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

fn meta_method_lua_name(variant: &str) -> String {
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
fn extract_closure_signature(
    tcx: TyCtxt<'_>,
    closure_expr: &hir::Expr<'_>,
    kind: MethodKind,
) -> Option<(Vec<LuaParam>, Vec<LuaType>)> {
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
                    let rest_names: Vec<String> = hir_param_names.iter().skip(1).cloned().collect();
                    fields.iter().skip(1).enumerate().map(|(i, field_ty)| LuaParam {
                        name: rest_names.get(i).cloned().unwrap_or_else(|| format!("p{}", i + 1)),
                        ty: map_ty_to_lua(tcx, field_ty),
                    }).collect()
                } else {
                    // Single AnyUserData param → no Lua-visible params
                    Vec::new()
                };

                let ret_ty = unwrap_result_ty(tcx, sig.output());
                let returns = if is_any_user_data(tcx, ret_ty) {
                    // Builder pattern: returns AnyUserData (self) for chaining.
                    vec![self_return_sentinel()]
                } else {
                    map_return_ty(tcx, sig.output())
                };
                return Some((params, returns));
            }
        }
        extract_params_from_tuple(tcx, user_params_ty, &hir_param_names)
    } else {
        Vec::new()
    };

    let ret_ty = sig.output();
    let returns = map_return_ty(tcx, ret_ty);

    Some((params, returns))
}

/// Extract parameter names from a HIR pattern (e.g. a tuple destructuring pattern).
fn extract_names_from_pat(pat: &hir::Pat<'_>) -> Vec<String> {
    match &pat.kind {
        hir::PatKind::Tuple(pats, _) => {
            pats.iter().map(|p| pat_to_name(p)).collect()
        }
        hir::PatKind::Binding(_, _, ident, _) => {
            vec![ident.name.as_str().to_string()]
        }
        _ => Vec::new(),
    }
}

/// Get a name from a simple binding pattern, falling back to `_`.
fn pat_to_name(pat: &hir::Pat<'_>) -> String {
    match &pat.kind {
        hir::PatKind::Binding(_, _, ident, _) => {
            let name = ident.name.as_str();
            // Skip the underscore prefix that rustc sometimes adds
            name.strip_prefix('_').filter(|s| !s.is_empty()).unwrap_or(name).to_string()
        }
        _ => "_".to_string(),
    }
}

fn extract_params_from_tuple<'tcx>(tcx: TyCtxt<'tcx>, ty: ty::Ty<'tcx>, hir_names: &[String]) -> Vec<LuaParam> {
    match ty.kind() {
        ty::TyKind::Tuple(fields) => fields
            .iter()
            .enumerate()
            .map(|(i, field_ty)| LuaParam {
                name: hir_names.get(i).cloned().unwrap_or_else(|| format!("p{}", i + 1)),
                ty: map_ty_to_lua(tcx, field_ty),
            })
            .collect(),
        _ => {
            if ty.is_unit() {
                Vec::new()
            } else {
                let name = hir_names.first().cloned().unwrap_or_else(|| "p1".to_string());
                vec![LuaParam {
                    name,
                    ty: map_ty_to_lua(tcx, ty),
                }]
            }
        }
    }
}

/// Check if a type is `u8`.
fn is_u8(ty: &ty::Ty<'_>) -> bool {
    matches!(ty.kind(), ty::TyKind::Uint(ty::UintTy::U8))
}

/// Check if a fully-qualified type path is a container that should become `string`
/// when parameterized with `u8` (byte buffers).
fn is_byte_container(path: &str) -> bool {
    matches!(
        path,
        "std::vec::Vec" | "alloc::vec::Vec"
        | "std::collections::VecDeque" | "std::collections::vec_deque::VecDeque"
        | "arrayvec::ArrayVec" | "smallvec::SmallVec"
        | "tinyvec::TinyVec" | "tinyvec::ArrayVec"
        | "thin_vec::ThinVec"
    )
}

/// Check if a trait path is a Fn-family trait (Fn, FnMut, FnOnce, async variants).
fn is_fn_trait(path: &str) -> bool {
    matches!(
        path,
        "std::ops::Fn" | "core::ops::Fn"
        | "std::ops::FnMut" | "core::ops::FnMut"
        | "std::ops::FnOnce" | "core::ops::FnOnce"
        | "std::ops::AsyncFn" | "core::ops::AsyncFn"
        | "std::ops::AsyncFnMut" | "core::ops::AsyncFnMut"
        | "std::ops::AsyncFnOnce" | "core::ops::AsyncFnOnce"
    )
}

/// Check if a trait path is a string-like trait (Display, ToString, Error).
fn is_string_trait(path: &str) -> bool {
    matches!(
        path,
        "std::fmt::Display" | "core::fmt::Display"
        | "std::string::ToString" | "alloc::string::ToString"
        | "std::error::Error" | "core::error::Error"
    )
}

/// Check if a trait path is Iterator or IntoIterator.
fn is_iterator_trait(path: &str) -> bool {
    matches!(
        path,
        "std::iter::Iterator" | "core::iter::Iterator"
        | "std::iter::IntoIterator" | "core::iter::IntoIterator"
    )
}

/// Check if a trait path is AsRef or Borrow (generic over the target type).
fn is_asref_trait(path: &str) -> bool {
    matches!(
        path,
        "std::convert::AsRef" | "core::convert::AsRef"
        | "std::borrow::Borrow" | "core::borrow::Borrow"
    )
}

/// Check if a trait path is Into or From (generic conversion trait).
fn is_into_trait(path: &str) -> bool {
    matches!(
        path,
        "std::convert::Into" | "core::convert::Into"
    )
}

/// Check if a trait path is Deref.
fn is_deref_trait(path: &str) -> bool {
    matches!(
        path,
        "std::ops::Deref" | "core::ops::Deref"
    )
}

/// Find a projection for a given associated type name (e.g. "Item", "Target", "Output").
fn find_projection<'tcx>(
    tcx: TyCtxt<'tcx>,
    predicates: &'tcx ty::List<ty::PolyExistentialPredicate<'tcx>>,
    assoc_name: &str,
) -> Option<ty::Ty<'tcx>> {
    for pred in predicates.iter() {
        if let ty::ExistentialPredicate::Projection(proj) = pred.skip_binder() {
            if tcx.item_name(proj.def_id).as_str() == assoc_name {
                return proj.term.as_type();
            }
        }
    }
    None
}

/// Map a `dyn Trait` type by inspecting the principal trait and projections.
fn map_dyn_trait<'tcx>(
    tcx: TyCtxt<'tcx>,
    predicates: &'tcx ty::List<ty::PolyExistentialPredicate<'tcx>>,
) -> LuaType {
    for pred in predicates.iter() {
        match pred.skip_binder() {
            ty::ExistentialPredicate::Trait(trait_ref) => {
                let trait_path = tcx.def_path_str(trait_ref.def_id);

                // dyn Fn(A, B) -> C → fun(p1: A, p2: B): C
                if is_fn_trait(&trait_path) {
                    // Fn trait args are encoded as the trait's generic args (tuple of params)
                    let params: Vec<LuaType> = trait_ref
                        .args
                        .iter()
                        .filter_map(|arg| arg.as_type())
                        // The first arg is the tuple of parameters
                        .next()
                        .map(|tuple_ty| match tuple_ty.kind() {
                            ty::TyKind::Tuple(fields) => {
                                fields.iter().map(|t| map_ty_to_lua(tcx, t)).collect()
                            }
                            _ => vec![map_ty_to_lua(tcx, tuple_ty)],
                        })
                        .unwrap_or_default();

                    // Return type is in the Output projection
                    let returns = find_projection(tcx, predicates, "Output")
                        .map(|ret_ty| {
                            let lua_ty = map_ty_to_lua(tcx, ret_ty);
                            if lua_ty == LuaType::Nil { vec![] } else { vec![lua_ty] }
                        })
                        .unwrap_or_default();

                    if params.is_empty() && returns.is_empty() {
                        return LuaType::Function;
                    }
                    return LuaType::FunctionSig { params, returns };
                }

                if is_string_trait(&trait_path) {
                    return LuaType::String;
                }

                // dyn AsRef<T> / dyn Borrow<T> → map T
                if is_asref_trait(&trait_path) {
                    if let Some(target) = trait_ref.args.iter().filter_map(|a| a.as_type()).next() {
                        return map_ty_to_lua(tcx, target);
                    }
                }

                // dyn Into<T> → map T
                if is_into_trait(&trait_path) {
                    if let Some(target) = trait_ref.args.iter().filter_map(|a| a.as_type()).next() {
                        return map_ty_to_lua(tcx, target);
                    }
                }

                // dyn Deref<Target = T> → map T
                if is_deref_trait(&trait_path) {
                    if let Some(target_ty) = find_projection(tcx, predicates, "Target") {
                        return map_ty_to_lua(tcx, target_ty);
                    }
                }

                // dyn Iterator<Item = T> / dyn IntoIterator<Item = T> → T[]
                if is_iterator_trait(&trait_path) {
                    if let Some(item_ty) = find_projection(tcx, predicates, "Item") {
                        return LuaType::Array(Box::new(map_ty_to_lua(tcx, item_ty)));
                    }
                    return LuaType::Array(Box::new(LuaType::Any));
                }
            }
            _ => {}
        }
    }
    LuaType::Any
}

/// Convert a rustc `ty::Ty` to a `LuaType`.
fn map_ty_to_lua<'tcx>(tcx: TyCtxt<'tcx>, ty: ty::Ty<'tcx>) -> LuaType {
    match ty.kind() {
        ty::TyKind::Bool => LuaType::Boolean,
        ty::TyKind::Int(_) | ty::TyKind::Uint(_) => LuaType::Integer,
        ty::TyKind::Float(_) => LuaType::Number,
        ty::TyKind::Str => LuaType::String,
        ty::TyKind::Char => LuaType::String,
        ty::TyKind::Tuple(fields) if fields.is_empty() => LuaType::Nil,
        ty::TyKind::Never => LuaType::Nil,

        ty::TyKind::Ref(_, inner, _) => map_ty_to_lua(tcx, *inner),
        ty::TyKind::RawPtr(inner, _) => map_ty_to_lua(tcx, *inner),

        // &[u8] → string (byte buffer), &[T] → T[]
        ty::TyKind::Slice(inner) => {
            if is_u8(inner) {
                LuaType::String
            } else {
                LuaType::Array(Box::new(map_ty_to_lua(tcx, *inner)))
            }
        }

        // [u8; N] → string (byte literal / buffer), [T; N] → T[]
        ty::TyKind::Array(inner, _len) => {
            if is_u8(inner) {
                LuaType::String
            } else {
                LuaType::Array(Box::new(map_ty_to_lua(tcx, *inner)))
            }
        }

        // Function pointers → extract typed signature
        ty::TyKind::FnPtr(sig_tys, _hdr) => {
            let sig = sig_tys.skip_binder();
            let params: Vec<LuaType> = sig.inputs().iter().map(|t| map_ty_to_lua(tcx, *t)).collect();
            let ret = map_ty_to_lua(tcx, sig.output());
            let returns = if ret == LuaType::Nil { vec![] } else { vec![ret] };
            if params.is_empty() && returns.is_empty() {
                LuaType::Function
            } else {
                LuaType::FunctionSig { params, returns }
            }
        }

        // FnDef and closures → function (signature not easily extractable inline)
        ty::TyKind::FnDef(..) | ty::TyKind::Closure(..) => LuaType::Function,

        // dyn Trait → inspect the trait to pick a better type
        ty::TyKind::Dynamic(predicates, ..) => {
            map_dyn_trait(tcx, predicates)
        }

        ty::TyKind::Adt(adt_def, args) => {
            let path = tcx.def_path_str(adt_def.did());

            // Vec<u8> / VecDeque<u8> etc. → string (byte buffer)
            if is_byte_container(&path) {
                if let Some(inner) = args.types().next() {
                    if is_u8(&inner) {
                        return LuaType::String;
                    }
                }
            }

            let type_args: Vec<LuaType> = args.types().map(|t| map_ty_to_lua(tcx, t)).collect();
            map_rust_type(&path, &type_args)
        }

        _ => LuaType::Any,
    }
}

/// Map a return type to a list of Lua types.
/// Handles Result unwrapping and tuple decomposition for multiple returns.
fn map_return_ty<'tcx>(tcx: TyCtxt<'tcx>, ty: ty::Ty<'tcx>) -> Vec<LuaType> {
    // Unwrap Coroutine (async closures) to get the actual return type
    let ty = unwrap_coroutine_ty(tcx, ty);
    // Then unwrap Result<T, _> → T
    let ty = unwrap_result_ty(tcx, ty);

    match ty.kind() {
        // Empty tuple = no return values
        ty::TyKind::Tuple(fields) if fields.is_empty() => Vec::new(),

        // Non-empty tuple = multiple return values
        ty::TyKind::Tuple(fields) => fields
            .iter()
            .map(|t| map_ty_to_lua(tcx, t))
            .collect(),

        // Single return value
        _ => {
            let lua_ty = map_ty_to_lua(tcx, ty);
            if lua_ty == LuaType::Nil {
                Vec::new()
            } else {
                vec![lua_ty]
            }
        }
    }
}

/// Unwrap Result<T, _> to get T. If not a Result, returns the type unchanged.
fn unwrap_result_ty<'tcx>(tcx: TyCtxt<'tcx>, ty: ty::Ty<'tcx>) -> ty::Ty<'tcx> {
    if let ty::TyKind::Adt(adt_def, args) = ty.kind() {
        let path = tcx.def_path_str(adt_def.did());
        if path == "std::result::Result" || path == "core::result::Result" {
            if let Some(inner) = args.types().next() {
                return inner;
            }
        }
    }
    ty
}

/// Unwrap a Coroutine type (from async closures) to find the Result<T, E> return type.
/// Async closures produce: Coroutine(DefId, [move_id, (), ResumeTy, (), Result<T, E>, ...])
fn unwrap_coroutine_ty<'tcx>(tcx: TyCtxt<'tcx>, ty: ty::Ty<'tcx>) -> ty::Ty<'tcx> {
    if let ty::TyKind::Coroutine(_, args) = ty.kind() {
        // The coroutine args contain the yield and return types.
        // Look for the first Result<_, mlua::Error> type in the generic args.
        for arg in args.iter() {
            if let Some(inner_ty) = arg.as_type() {
                if let ty::TyKind::Adt(adt_def, _) = inner_ty.kind() {
                    let path = tcx.def_path_str(adt_def.did());
                    if path == "std::result::Result" || path == "core::result::Result" {
                        return inner_ty;
                    }
                }
            }
        }
    }
    ty
}

/// Infer the Lua type of a value expression using typeck.
fn infer_expr_lua_type(tcx: TyCtxt<'_>, expr: &hir::Expr<'_>) -> LuaType {
    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    let ty = typeck.expr_ty(expr);
    map_ty_to_lua(tcx, ty)
}

/// Returns true if a LuaType carries useful type information (not Any, Nil, or Function bare).
fn is_informative(ty: &LuaType) -> bool {
    !matches!(ty, LuaType::Any | LuaType::Nil | LuaType::Function)
}

/// Walk a closure body to infer the concrete type when the signature says `Value` (any).
/// Recursively searches the entire expression tree for `.into_lua()` calls and
/// returns the type of the receiver — the concrete type before Lua conversion.
fn infer_concrete_type_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    // First try structural inference (Ok(...), blocks, etc.)
    if let Some(ty) = infer_from_expr(tcx, typeck, expr) {
        return Some(ty);
    }
    // Fall back to a deep search for .into_lua() calls in the entire tree
    find_into_lua_receiver_type(tcx, expr)
}

/// Recursively search an expression tree for `.into_lua()` calls and return
/// the concrete type of the receiver. Crosses closure boundaries to handle
/// patterns like `borrow_mut_scoped(|me| { ... value.into_lua(lua) })`.
fn find_into_lua_receiver_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            let name = segment.ident.name.as_str();
            if name == "into_lua" {
                let typeck = tcx.typeck(receiver.hir_id.owner.def_id);
                let receiver_ty = typeck.expr_ty(receiver);
                let lua_ty = map_ty_to_lua(tcx, receiver_ty);
                if is_informative(&lua_ty) {
                    return Some(lua_ty);
                }
            }
            // Recurse into receiver and args
            if let Some(ty) = find_into_lua_receiver_type(tcx, receiver) {
                return Some(ty);
            }
            for arg in *args {
                if let Some(ty) = find_into_lua_receiver_type(tcx, arg) {
                    return Some(ty);
                }
            }
            None
        }
        hir::ExprKind::Block(block, _) => {
            // Check all statements and tail
            for stmt in block.stmts {
                if let hir::StmtKind::Semi(e) | hir::StmtKind::Expr(e) | hir::StmtKind::Let(hir::LetStmt { init: Some(e), .. }) = &stmt.kind {
                    if let Some(ty) = find_into_lua_receiver_type(tcx, e) {
                        return Some(ty);
                    }
                }
            }
            block.expr.and_then(|e| find_into_lua_receiver_type(tcx, e))
        }
        hir::ExprKind::Match(scrut, arms, _) => {
            if let Some(ty) = find_into_lua_receiver_type(tcx, scrut) {
                return Some(ty);
            }
            for arm in *arms {
                if let Some(ty) = find_into_lua_receiver_type(tcx, arm.body) {
                    return Some(ty);
                }
            }
            None
        }
        hir::ExprKind::Call(callee, args) => {
            if let Some(ty) = find_into_lua_receiver_type(tcx, callee) {
                return Some(ty);
            }
            for arg in *args {
                if let Some(ty) = find_into_lua_receiver_type(tcx, arg) {
                    return Some(ty);
                }
            }
            None
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
            if let Some(ty) = find_into_lua_receiver_type(tcx, then_expr) {
                return Some(ty);
            }
            if let Some(e) = else_expr {
                find_into_lua_receiver_type(tcx, e)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn infer_from_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    typeck: &'tcx ty::TypeckResults<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    match &expr.kind {
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
        hir::ExprKind::Call(callee, args) => {
            if let hir::ExprKind::Path(hir::QPath::Resolved(_, path)) = &callee.kind {
                if let Some(seg) = path.segments.last() {
                    if seg.ident.name.as_str() == "Ok" && args.len() == 1 {
                        return infer_from_expr(tcx, typeck, &args[0]);
                    }
                }
            }
            // For function calls, check the return type
            let call_ty = typeck.expr_ty(expr);
            let lua_ty = map_ty_to_lua(tcx, call_ty);
            if is_informative(&lua_ty) {
                Some(lua_ty)
            } else {
                None
            }
        }

        // Method call chains: expr.method().method2() — check the overall type
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            let call_ty = typeck.expr_ty(expr);
            let lua_ty = map_ty_to_lua(tcx, call_ty);
            if is_informative(&lua_ty) {
                return Some(lua_ty);
            }
            // For scoped borrow methods (borrow_mut_scoped, borrow_scoped),
            // the concrete type is inside the closure argument's body.
            let method_name = segment.ident.name.as_str();
            if method_name.contains("scoped") || method_name.contains("scope") {
                for arg in *args {
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
            infer_from_expr(tcx, typeck, receiver)
        }

        // Closure body is a block — check the tail expression
        hir::ExprKind::Block(block, _) => {
            if let Some(tail) = block.expr {
                return infer_from_expr(tcx, typeck, tail);
            }
            // Check last statement if it's an expression
            if let Some(stmt) = block.stmts.last() {
                if let hir::StmtKind::Semi(e) | hir::StmtKind::Expr(e) = &stmt.kind {
                    return infer_from_expr(tcx, typeck, e);
                }
            }
            None
        }

        // Match/if expressions — try the first arm
        hir::ExprKind::Match(_, arms, _) => {
            for arm in *arms {
                if let Some(ty) = infer_from_expr(tcx, typeck, arm.body) {
                    return Some(ty);
                }
            }
            None
        }

        hir::ExprKind::If(_, then_expr, _) => {
            infer_from_expr(tcx, typeck, then_expr)
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
    }
}

// ── Field extraction ───────────────────────────────────────────────────

fn extract_fields_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    impl_item: &hir::ImplItem<'tcx>,
    fields: &mut Vec<LuaField>,
) {
    let hir::ImplItemKind::Fn(_sig, body_id) = impl_item.kind else {
        return;
    };
    let body = tcx.hir_body(body_id);
    visit_expr_for_fields(tcx, body.value, fields);
}

fn visit_expr_for_fields<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    fields: &mut Vec<LuaField>,
) {
    match &expr.kind {
        hir::ExprKind::Block(block, _) => {
            for stmt in block.stmts {
                match &stmt.kind {
                    hir::StmtKind::Semi(e) | hir::StmtKind::Expr(e) => {
                        visit_expr_for_fields(tcx, e, fields);
                    }
                    hir::StmtKind::Let(local) => {
                        if let Some(init) = local.init {
                            visit_expr_for_fields(tcx, init, fields);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(e) = block.expr {
                visit_expr_for_fields(tcx, e, fields);
            }
        }
        hir::ExprKind::If(_, then_block, else_block) => {
            visit_expr_for_fields(tcx, then_block, fields);
            if let Some(else_expr) = else_block {
                visit_expr_for_fields(tcx, else_expr, fields);
            }
        }
        hir::ExprKind::Match(_, arms, _) => {
            for arm in *arms {
                visit_expr_for_fields(tcx, arm.body, fields);
            }
        }
        hir::ExprKind::Loop(block, _, _, _) => {
            walk_loop_body!(block, |e| visit_expr_for_fields(tcx, e, fields));
        }
        hir::ExprKind::MethodCall(segment, _receiver, args, _span) => {
            let method_name = segment.ident.name.as_str();
            match method_name {
                // Getter variants (method_get and function_get have same Lua effect)
                "add_field_method_get" | "add_field_function_get" => {
                    if let Some(f) = extract_field_getter(tcx, args) {
                        if let Some(existing) = fields.iter_mut().find(|ef| ef.name == f.name) {
                            existing.ty = f.ty;
                        } else {
                            fields.push(f);
                        }
                    }
                }
                // Setter variants
                "add_field_method_set" | "add_field_function_set" => {
                    if !args.is_empty() {
                        if let Some(name) = extract_string_literal(&args[0]) {
                            if let Some(existing) = fields.iter_mut().find(|f| f.name == name) {
                                existing.writable = true;
                            } else {
                                fields.push(LuaField {
                                    name,
                                    ty: LuaType::Any,
                                    writable: true,
                                    doc: None,
                                });
                            }
                        }
                    }
                }
                // Static field: add_field("name", value)
                // We can't easily infer the type from a value expression,
                // but we can try to get it from typeck
                "add_field" | "add_meta_field" => {
                    if !args.is_empty() {
                        if let Some(name) = extract_string_literal(&args[0]) {
                            let ty = if args.len() >= 2 {
                                infer_expr_lua_type(tcx, &args[1])
                            } else {
                                LuaType::Any
                            };
                            fields.push(LuaField {
                                name,
                                ty,
                                writable: false,
                                doc: None,
                            });
                        }
                    }
                }
                // Computed meta field: add_meta_field_with("name", |lua| -> value)
                "add_meta_field_with" => {
                    if let Some(f) = extract_field_getter(tcx, args) {
                        fields.push(f);
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn extract_field_getter<'tcx>(
    tcx: TyCtxt<'tcx>,
    args: &'tcx [hir::Expr<'tcx>],
) -> Option<LuaField> {
    if args.len() < 2 {
        return None;
    }

    let name = extract_string_literal(&args[0])?;
    let closure_expr = &args[1];

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

    let ret_ty = sig.output();
    let unwrapped = unwrap_result_ty(tcx, ret_ty);
    let mut ty = map_ty_to_lua(tcx, ret_ty);

    // When the closure returns Result<Value> (→ Any), try to infer the concrete
    // type by walking the closure body for constructor expressions. This handles
    // cached_field! macros where the type is erased to Value by .into_lua().
    if ty == LuaType::Any {
        let body = tcx.hir_body(closure.body);
        eprintln!("DEBUG BODY: field={name}, body_expr_is_block={}", matches!(&body.value.kind, hir::ExprKind::Block(..)));
        if let hir::ExprKind::Block(block, _) = &body.value.kind {
            if let Some(tail) = block.expr {
                eprintln!("DEBUG BODY: field={name}, block_tail_kind={}", match &tail.kind {
                    hir::ExprKind::MethodCall(seg, _, _, _) => format!("MethodCall({})", seg.ident.name),
                    hir::ExprKind::Call(_, _) => "Call".to_string(),
                    hir::ExprKind::Block(_, _) => "Block".to_string(),
                    hir::ExprKind::Closure(_) => "Closure".to_string(),
                    hir::ExprKind::Match(_, _, _) => "Match".to_string(),
                    hir::ExprKind::If(_, _, _) => "If".to_string(),
                    hir::ExprKind::Ret(_) => "Ret".to_string(),
                    hir::ExprKind::Path(_) => "Path".to_string(),
                    hir::ExprKind::Struct(_, _, _) => "Struct".to_string(),
                    other => format!("Other({:?})", std::mem::discriminant(other)),
                });
            } else {
                eprintln!("DEBUG BODY: field={name}, no block tail, stmts={}", block.stmts.len());
            }
        }
        if let Some(inferred) = infer_concrete_type_from_body(tcx, body.value) {
            eprintln!("DEBUG FIELD: name={name}, inferred={inferred:?} from body (was {unwrapped:?})");
            ty = inferred;
        } else {
            eprintln!("DEBUG FIELD: name={name}, ret_ty={ret_ty:?}, unwrapped={unwrapped:?} (no inference)");
        }
    }

    Some(LuaField {
        name,
        ty,
        writable: false,
        doc: None,
    })
}
