use std::collections::{HashMap, HashSet};

use heck::ToUpperCamelCase as _;
use mlua_typegen::{ExtractedApi, LuaClass, LuaModule, LuaType};
use rustc_hir as hir;
use rustc_middle::ty::TyCtxt;

use crate::extract::{
    RegistrationOwner, expr_snippet, peel_expr, trace, visit_recursive_expr_children,
    writable_field,
};

use super::events::record_event_emission;
use super::function_value::try_extract_create_function;
use super::module_export::{extract_named_set_call, module_for_function, record_module_export};
use super::table_module::{
    create_table_module_from_expr, try_extract_module_from_value_expr,
    try_extract_module_name_from_call,
};
use super::value_inference::{ExtractedModule, infer_value_expr_lua_type, upsert_extracted_module};

// ── Function registration extraction ───────────────────────────────────

/// Collect all supported facts from one function body in one traversal.
pub(in crate::extract) fn collect_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    body_id: hir::BodyId,
    module_item: Option<&hir::Item<'tcx>>,
    generated_module_name: Option<String>,
    registration_owner: Option<RegistrationOwner>,
    api: &mut ExtractedApi,
) {
    let body = tcx.hir_body(body_id);
    let annotated_module = module_item
        .and_then(|item| module_for_function(tcx, item))
        .or_else(|| {
            generated_module_name.map(|name| LuaModule {
                name,
                doc: module_item
                    .and_then(|item| crate::extract::extract_doc_comments(tcx, item.hir_id())),
                functions: Vec::new(),
                fields: Vec::new(),
            })
        });
    extract_body(tcx, body.value, api, annotated_module, registration_owner);
}

fn extract_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    api: &mut ExtractedApi,
    annotated_module: Option<LuaModule>,
    registration_owner: Option<RegistrationOwner>,
) {
    let mut ctx = BodyCollector {
        tcx,
        api,
        annotated_module,
        table_bindings: HashMap::new(),
        global_bindings: HashSet::new(),
        local_module_names: HashSet::new(),
        exported_module_names: HashSet::new(),
        module_children: HashMap::new(),
        registration_stack: registration_owner
            .map(|owner| RegistrationDraft::new(tcx, owner))
            .into_iter()
            .collect(),
    };
    visit_expr(&mut ctx, expr);
    ctx.finish();
}

struct BodyCollector<'a, 'tcx> {
    tcx: TyCtxt<'tcx>,
    api: &'a mut ExtractedApi,
    annotated_module: Option<LuaModule>,
    table_bindings: HashMap<String, usize>,
    global_bindings: HashSet<String>,
    local_module_names: HashSet<String>,
    exported_module_names: HashSet<String>,
    module_children: HashMap<String, Vec<String>>,
    registration_stack: Vec<RegistrationDraft>,
}

struct RegistrationDraft {
    owner: RegistrationOwner,
    class: LuaClass,
}

impl RegistrationDraft {
    fn new(tcx: TyCtxt<'_>, owner: RegistrationOwner) -> Self {
        let class = owner.empty_class(tcx);
        Self { owner, class }
    }
}

impl BodyCollector<'_, '_> {
    fn finish(mut self) {
        prune_unexported_registration_modules(
            self.api,
            &self.local_module_names,
            &self.exported_module_names,
        );
        if let Some(module) = self.annotated_module.take() {
            upsert_module(self.api, module);
        }
        if let Some(registration) = self.registration_stack.pop() {
            finish_registration(self.api, registration);
        }
        debug_assert!(self.registration_stack.is_empty());
    }
}

fn finish_registration(api: &mut ExtractedApi, registration: RegistrationDraft) {
    let class = registration.owner.finish_class(registration.class);
    crate::extract::trace_class_snapshot(
        "registration",
        &class.name,
        &class.fields,
        &class.methods,
    );
    crate::extract::upsert_class(api, class);
}

fn visit_block<'tcx>(ctx: &mut BodyCollector<'_, 'tcx>, block: &'tcx hir::Block<'tcx>) {
    for stmt in block.stmts {
        match &stmt.kind {
            hir::StmtKind::Semi(expr) | hir::StmtKind::Expr(expr) => {
                visit_expr(ctx, expr);
            }
            hir::StmtKind::Let(local) => {
                if let Some(init) = local.init {
                    if let Some(binding) = extract_binding_name(local.pat) {
                        if is_globals_call(init) {
                            ctx.global_bindings.insert(binding.clone());
                        }
                        if let Some(module) = create_table_module_from_expr(ctx.tcx, init, &binding)
                        {
                            upsert_extracted_module(ctx.api, module);
                            ctx.local_module_names.insert(binding.clone());
                            let idx = resolve_module_index(ctx, &binding)
                                .unwrap_or(ctx.api.modules.len());
                            ctx.table_bindings.insert(binding, idx);
                        } else if let Some(module_name) =
                            try_extract_module_name_from_call(ctx.tcx, init)
                        {
                            let extracted = ExtractedModule {
                                module: LuaModule {
                                    name: module_name.clone(),
                                    doc: None,
                                    functions: Vec::new(),
                                    fields: Vec::new(),
                                },
                                nested_modules: Vec::new(),
                            };
                            upsert_extracted_module(ctx.api, extracted);
                            ctx.local_module_names.insert(module_name.clone());
                            let idx = resolve_module_index(ctx, &module_name)
                                .unwrap_or(ctx.api.modules.len());
                            ctx.table_bindings.insert(binding, idx);
                            mark_module_exported(ctx, &module_name);
                        }
                    }
                    visit_expr(ctx, init);
                }
            }
            _ => {}
        }
    }

    if let Some(expr) = block.expr {
        visit_expr(ctx, expr);
    }
}

fn is_global_registration_target<'tcx>(
    ctx: &BodyCollector<'_, 'tcx>,
    receiver: &'tcx hir::Expr<'tcx>,
    receiver_name: Option<&str>,
) -> bool {
    is_globals_call(receiver)
        || receiver_name.is_some_and(|name| ctx.global_bindings.contains(name))
}

fn rename_registered_module_binding<'tcx>(
    ctx: &mut BodyCollector<'_, 'tcx>,
    binding_name: &str,
    exported_name: &str,
) -> bool {
    let Some(&idx) = ctx.table_bindings.get(binding_name) else {
        return false;
    };

    trace(format!(
        "renaming module binding {binding_name} -> {exported_name}"
    ));
    ctx.api.modules[idx].name = exported_name.to_string();
    ctx.table_bindings.insert(exported_name.to_string(), idx);
    ctx.local_module_names.insert(exported_name.to_string());
    mark_module_exported(ctx, exported_name);
    true
}

fn record_exported_registration_module<'tcx>(
    ctx: &mut BodyCollector<'_, 'tcx>,
    extracted: ExtractedModule,
) {
    trace(format!("extracted global module {}", extracted.module.name));
    let module_name = upsert_extracted_module(ctx.api, extracted);
    ctx.local_module_names.insert(module_name.clone());
    mark_module_exported(ctx, &module_name);
}

fn record_global_registration_field<'tcx>(
    ctx: &mut BodyCollector<'_, 'tcx>,
    name: &str,
    value: &'tcx hir::Expr<'tcx>,
) {
    trace(format!("recording global field {name}"));
    ctx.api.global_fields.push(writable_field(
        name.to_string(),
        infer_value_expr_lua_type(ctx.tcx, value),
    ));
}

fn handle_global_registration<'tcx>(
    ctx: &mut BodyCollector<'_, 'tcx>,
    name: &str,
    value: &'tcx hir::Expr<'tcx>,
) {
    if let Some(binding_name) = table_binding_name(value)
        && rename_registered_module_binding(ctx, &binding_name, name)
    {
        return;
    }

    if let Some(module) = try_extract_module_from_value_expr(ctx.tcx, value, name) {
        record_exported_registration_module(ctx, module);
        return;
    }

    record_global_registration_field(ctx, name, value);
}

fn handle_module_registration<'tcx>(
    ctx: &mut BodyCollector<'_, 'tcx>,
    module_name: &str,
    field_name: &str,
    value: &'tcx hir::Expr<'tcx>,
) {
    let Some(idx) = resolve_module_index(ctx, module_name) else {
        trace(format!(
            "skipping field {field_name} on non-exported table {module_name}"
        ));
        return;
    };

    let parent_name = ctx.api.modules[idx].name.clone();
    trace(format!(
        "recording field {field_name} on module {parent_name}"
    ));
    ctx.api.modules[idx].fields.push(writable_field(
        field_name.to_string(),
        infer_value_expr_lua_type(ctx.tcx, value),
    ));

    let child_module_name = nested_module_name(&parent_name, field_name);
    if let Some(module) = try_extract_module_from_value_expr(ctx.tcx, value, &child_module_name) {
        let child_name = module.module.name.clone();
        trace(format!("extracted child module {}", child_name));
        upsert_extracted_module(ctx.api, module);
        ctx.local_module_names.insert(child_name.clone());
        ctx.module_children
            .entry(parent_name.clone())
            .or_default()
            .push(child_name.clone());
        if ctx.exported_module_names.contains(&parent_name) {
            mark_module_exported(ctx, &child_name);
        }
        if let Some(field) = ctx.api.modules[idx]
            .fields
            .iter_mut()
            .find(|field| field.name == field_name)
        {
            field.ty = LuaType::Class(child_name);
        }
    }
}

fn handle_named_registration<'tcx>(
    ctx: &mut BodyCollector<'_, 'tcx>,
    receiver: &'tcx hir::Expr<'tcx>,
    name: &str,
    value: &'tcx hir::Expr<'tcx>,
) {
    let receiver_name = table_binding_name(receiver);
    let is_globals = is_global_registration_target(ctx, receiver, receiver_name.as_deref());

    trace(format!(
        "saw set({}, ...) receiver={} globals={} value={}",
        name,
        expr_snippet(ctx.tcx, receiver),
        is_globals,
        expr_snippet(ctx.tcx, value)
    ));

    if let Some(func) = try_extract_create_function(ctx.tcx, value, name) {
        trace(format!("extracted function {name}"));
        if is_globals {
            ctx.api.global_functions.push(func);
        } else if let Some(module_name) = receiver_name.as_deref()
            && let Some(idx) = resolve_module_index(ctx, module_name)
        {
            ctx.api.modules[idx].functions.push(func);
        }
        return;
    }

    if is_globals {
        handle_global_registration(ctx, name, value);
    } else if let Some(module_name) = receiver_name.as_deref() {
        handle_module_registration(ctx, module_name, name, value);
    }
}

fn visit_expr<'tcx>(ctx: &mut BodyCollector<'_, 'tcx>, expr: &'tcx hir::Expr<'tcx>) {
    if let Some(registration) = ctx.registration_stack.last_mut() {
        crate::extract::record_field_registration(ctx.tcx, expr, &mut registration.class.fields);
        crate::extract::record_method_registration(ctx.tcx, expr, &mut registration.class.methods);
    }
    record_event_emission(ctx.tcx, expr, ctx.api);
    if let Some(module) = &mut ctx.annotated_module {
        record_module_export(ctx.tcx, expr, module);
    }
    if let hir::ExprKind::MethodCall(_, receiver, _, _) = &expr.kind
        && let Some(named_call) = extract_named_set_call(expr)
    {
        handle_named_registration(ctx, receiver, &named_call.name, named_call.value);
    }

    match &expr.kind {
        hir::ExprKind::Block(block, _) => visit_block(ctx, block),
        hir::ExprKind::Closure(closure) => {
            let body = ctx.tcx.hir_body(closure.body);
            if let Some(owner) = crate::extract::userdata_registration::registration_owner_for_body(
                ctx.tcx,
                closure.body,
            ) {
                ctx.registration_stack
                    .push(RegistrationDraft::new(ctx.tcx, owner));
                visit_expr(ctx, body.value);
                let registration = ctx
                    .registration_stack
                    .pop()
                    .expect("nested registration was pushed");
                finish_registration(ctx.api, registration);
            } else {
                visit_expr(ctx, body.value);
            }
        }
        _ => visit_recursive_expr_children(expr, |child| {
            visit_expr(ctx, child);
        }),
    }
}

fn extract_binding_name(pat: &hir::Pat<'_>) -> Option<String> {
    match &pat.kind {
        hir::PatKind::Binding(_, _, ident, _) => Some(ident.name.as_str().to_string()),
        _ => None,
    }
}

fn resolve_module_index<'tcx>(ctx: &BodyCollector<'_, 'tcx>, module_name: &str) -> Option<usize> {
    ctx.table_bindings
        .get(module_name)
        .copied()
        .or_else(|| ctx.api.modules.iter().position(|m| m.name == module_name))
}

fn mark_module_exported<'tcx>(ctx: &mut BodyCollector<'_, 'tcx>, module_name: &str) {
    if !ctx.exported_module_names.insert(module_name.to_string()) {
        return;
    }

    if let Some(children) = ctx.module_children.get(module_name).cloned() {
        for child in children {
            mark_module_exported(ctx, &child);
        }
    }
}

fn prune_unexported_registration_modules(
    api: &mut ExtractedApi,
    local_module_names: &HashSet<String>,
    exported_module_names: &HashSet<String>,
) {
    let removed: HashSet<_> = local_module_names
        .difference(exported_module_names)
        .cloned()
        .collect();

    if removed.is_empty() {
        return;
    }

    api.modules.retain(|module| !removed.contains(&module.name));
}

pub(in crate::extract) fn upsert_module(api: &mut ExtractedApi, module: LuaModule) {
    if let Some(index) = api.modules.iter().position(|m| m.name == module.name) {
        let existing = &mut api.modules[index];
        existing.functions.extend(module.functions);
        existing.fields.extend(module.fields);
        if existing.doc.is_none() {
            existing.doc = module.doc;
        }
    } else {
        api.modules.push(module);
    }
}

/// Check if an expression is `lua.globals()` or `<something>.globals()`.
fn is_globals_call(expr: &hir::Expr<'_>) -> bool {
    let expr = peel_expr(expr);
    if let hir::ExprKind::MethodCall(segment, _, _, _) = &expr.kind {
        return segment.ident.name.as_str() == "globals";
    }
    false
}

/// Try to get a name for a table variable (best effort).
fn table_binding_name(expr: &hir::Expr<'_>) -> Option<String> {
    let expr = peel_expr(expr);
    match &expr.kind {
        hir::ExprKind::Path(hir::QPath::Resolved(_, path)) => path
            .segments
            .last()
            .map(|s| s.ident.name.as_str().to_string()),
        _ => None,
    }
}

pub(in crate::extract) fn nested_module_name(parent: &str, field_name: &str) -> String {
    format!("{parent}__{}", field_name.to_upper_camel_case())
}
