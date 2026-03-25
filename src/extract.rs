use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use rustc_hir as hir;
use rustc_hir::attrs::AttributeKind;
use rustc_hir::def_id::LocalDefId;
use rustc_middle::ty::{self, TyCtxt};
use rustc_span::Symbol;

use heck::{ToSnakeCase, ToUpperCamelCase};
use mlua_typegen::typemap::map_rust_type;
use mlua_typegen::{
    EventEmission, LuaApi, LuaClass, LuaEnum, LuaField, LuaFunction, LuaMethod, LuaModule,
    LuaParam, LuaReturn, LuaType, MethodKind, make_union,
};

fn trace_enabled() -> bool {
    std::env::var_os("MLUA_TYPEGEN_TRACE").is_some()
}

fn trace(msg: impl AsRef<str>) {
    if !trace_enabled() {
        return;
    }

    let msg = format!("[mlua-typegen] {}\n", msg.as_ref());
    eprint!("{msg}");

    if let Some(path) = std::env::var_os("MLUA_TYPEGEN_TRACE_FILE")
        && let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        use std::io::Write;
        let _ = file.write_all(msg.as_bytes());
    }
}

fn should_trace_method_name(name: &str) -> bool {
    matches!(
        name,
        "open"
            | "history"
            | "ends_with"
            | "join"
            | "starts_with"
            | "strip_prefix"
            | "raw"
            | "__eq"
            | "__pairs"
    )
}

fn should_trace_field_name(name: &str) -> bool {
    matches!(name, "base" | "parent" | "raw" | "url" | "path")
}

fn should_trace_field_expr_snippet(snippet: &str) -> bool {
    snippet.contains("me.base()")
        || snippet.contains("me.parent()")
        || snippet.contains("map(Self::new)")
        || snippet.contains("($value as fn")
}

fn expr_snippet(tcx: TyCtxt<'_>, expr: &hir::Expr<'_>) -> String {
    tcx.sess
        .source_map()
        .span_to_snippet(expr.span)
        .unwrap_or_else(|_| format!("<{:?}>", expr.kind))
}

fn qualified_def_path_str(tcx: TyCtxt<'_>, def_id: rustc_hir::def_id::DefId) -> String {
    let path = tcx.def_path_str(def_id);
    if !def_id.is_local() {
        return path;
    }

    let crate_name = tcx
        .crate_name(rustc_hir::def_id::LOCAL_CRATE)
        .as_str()
        .to_string();
    if path == crate_name || path.starts_with(&format!("{crate_name}::")) {
        path
    } else {
        format!("{crate_name}::{path}")
    }
}

fn def_snippet(tcx: TyCtxt<'_>, def_id: rustc_hir::def_id::DefId) -> Option<String> {
    let sm = tcx.sess.source_map();
    let span = def_id.as_local().map_or_else(
        || tcx.def_span(def_id),
        |local| match tcx.hir_node_by_def_id(local) {
            rustc_hir::Node::Item(item) => item.span,
            rustc_hir::Node::ImplItem(item) => item.span,
            rustc_hir::Node::TraitItem(item) => item.span,
            rustc_hir::Node::Expr(expr) => expr.span,
            _ => tcx.def_span(def_id),
        },
    );

    sm.span_to_snippet(span)
        .ok()
        .filter(|snippet| snippet.contains('\n') || snippet.len() > 64)
        .or_else(|| def_file_context(tcx, span))
}

fn def_file_context(tcx: TyCtxt<'_>, span: rustc_span::Span) -> Option<String> {
    let sm = tcx.sess.source_map();
    let filename = sm
        .span_to_filename(span)
        .prefer_local_unconditionally()
        .to_string();
    if filename.starts_with('<') {
        return None;
    }

    let start = sm.lookup_char_pos(span.lo()).line.saturating_sub(1);
    let end = sm.lookup_char_pos(span.hi()).line.saturating_add(80);
    let text = std::fs::read_to_string(filename).ok()?;
    let lines: Vec<_> = text.lines().collect();
    if start >= lines.len() {
        return None;
    }

    Some(lines[start..end.min(lines.len())].join("\n"))
}

/// Sentinel value used to mark "returns Self" during extraction.
/// `extract_class` replaces this with the actual class name.
fn self_return_sentinel() -> LuaType {
    LuaType::Class(String::new())
}

fn is_self_return_sentinel(ty: &LuaType) -> bool {
    matches!(ty, LuaType::Class(name) if name.is_empty())
}

fn contains_self_return_sentinel(ty: &LuaType) -> bool {
    match ty {
        LuaType::Class(_) => is_self_return_sentinel(ty),
        LuaType::Array(inner) | LuaType::Optional(inner) | LuaType::Variadic(inner) => {
            contains_self_return_sentinel(inner)
        }
        LuaType::Map(key, value) => {
            contains_self_return_sentinel(key) || contains_self_return_sentinel(value)
        }
        LuaType::Union(items) => items.iter().any(contains_self_return_sentinel),
        LuaType::FunctionSig { params, returns } => {
            params.iter().any(contains_self_return_sentinel)
                || returns.iter().any(contains_self_return_sentinel)
        }
        _ => false,
    }
}

fn walk_lua_type_mut<F>(ty: &mut LuaType, visit: &mut F)
where
    F: FnMut(&mut LuaType),
{
    visit(ty);

    match ty {
        LuaType::Array(inner) | LuaType::Optional(inner) | LuaType::Variadic(inner) => {
            walk_lua_type_mut(inner, visit)
        }
        LuaType::Map(key, value) => {
            walk_lua_type_mut(key, visit);
            walk_lua_type_mut(value, visit);
        }
        LuaType::Union(items) => {
            for item in items {
                walk_lua_type_mut(item, visit);
            }
        }
        LuaType::FunctionSig { params, returns } => {
            for param in params {
                walk_lua_type_mut(param, visit);
            }
            for ret in returns {
                walk_lua_type_mut(ret, visit);
            }
        }
        _ => {}
    }
}

fn replace_self_return_sentinel(ty: &mut LuaType, class_name: &str) {
    walk_lua_type_mut(ty, &mut |ty| {
        if let LuaType::Class(name) = ty
            && name.is_empty()
        {
            *name = class_name.to_string();
        }
    });
}

fn replace_self_class_alias(ty: &mut LuaType, class_name: &str) {
    walk_lua_type_mut(ty, &mut |ty| {
        if let LuaType::Class(name) = ty
            && name == "Self"
        {
            *name = class_name.to_string();
        }
    });
}

fn lua_runtime_type() -> LuaType {
    LuaType::Class("Lua".to_string())
}

thread_local! {
    static LOCAL_PARAM_INFERENCE_STACK: RefCell<Vec<(LocalDefId, usize)>> = const { RefCell::new(Vec::new()) };
}

struct LocalParamInferenceGuard(LocalDefId, usize);

impl Drop for LocalParamInferenceGuard {
    fn drop(&mut self) {
        LOCAL_PARAM_INFERENCE_STACK.with(|stack| {
            let popped = stack.borrow_mut().pop();
            debug_assert_eq!(popped, Some((self.0, self.1)));
        });
    }
}

fn enter_local_param_inference(
    def_id: LocalDefId,
    param_index: usize,
) -> Option<LocalParamInferenceGuard> {
    let already_in_progress = LOCAL_PARAM_INFERENCE_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        if stack.contains(&(def_id, param_index)) {
            true
        } else {
            stack.push((def_id, param_index));
            false
        }
    });

    (!already_in_progress).then_some(LocalParamInferenceGuard(def_id, param_index))
}

/// Check whether a type is or contains `mlua::AnyUserData`.
/// Matches: AnyUserData, Option<AnyUserData>, Result<AnyUserData, _>.
fn is_any_user_data(tcx: TyCtxt<'_>, ty: ty::Ty<'_>) -> bool {
    if let ty::TyKind::Adt(adt_def, substs) = ty.kind() {
        let path = tcx.def_path_str(adt_def.did());
        if path.ends_with("AnyUserData") {
            return true;
        }
        // Check Option<AnyUserData> or Result<AnyUserData, _>
        if (path.ends_with("Option") || path.ends_with("Result"))
            && !substs.is_empty()
            && let Some(inner) = substs[0].as_type()
        {
            return is_any_user_data(tcx, inner);
        }
    }
    false
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

fn visit_structural_expr_children<'tcx>(
    expr: &'tcx hir::Expr<'tcx>,
    mut visit: impl FnMut(&'tcx hir::Expr<'tcx>),
) {
    match &expr.kind {
        hir::ExprKind::Block(block, _) => {
            for stmt in block.stmts {
                match &stmt.kind {
                    hir::StmtKind::Semi(expr) | hir::StmtKind::Expr(expr) => visit(expr),
                    hir::StmtKind::Let(local) => {
                        if let Some(init) = local.init {
                            visit(init);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(expr) = block.expr {
                visit(expr);
            }
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            visit(cond);
            visit(then_expr);
            if let Some(else_expr) = else_expr {
                visit(else_expr);
            }
        }
        hir::ExprKind::Match(scrutinee, arms, _) => {
            visit(scrutinee);
            for arm in *arms {
                visit(arm.body);
            }
        }
        hir::ExprKind::Loop(block, _, _, _) => {
            walk_loop_body!(block, &mut visit);
        }
        hir::ExprKind::DropTemps(inner)
        | hir::ExprKind::Use(inner, _)
        | hir::ExprKind::Unary(_, inner)
        | hir::ExprKind::Cast(inner, _)
        | hir::ExprKind::Type(inner, _)
        | hir::ExprKind::Field(inner, _)
        | hir::ExprKind::AddrOf(_, _, inner)
        | hir::ExprKind::Become(inner)
        | hir::ExprKind::Yield(inner, _)
        | hir::ExprKind::UnsafeBinderCast(_, inner, _) => visit(inner),
        hir::ExprKind::Assign(lhs, rhs, _) | hir::ExprKind::AssignOp(_, lhs, rhs) => {
            visit(lhs);
            visit(rhs);
        }
        hir::ExprKind::Index(lhs, rhs, _) | hir::ExprKind::Binary(_, lhs, rhs) => {
            visit(lhs);
            visit(rhs);
        }
        hir::ExprKind::Array(exprs) | hir::ExprKind::Tup(exprs) => {
            for expr in *exprs {
                visit(expr);
            }
        }
        hir::ExprKind::Repeat(value, _) => visit(value),
        hir::ExprKind::Ret(value) | hir::ExprKind::Break(_, value) => {
            if let Some(value) = value {
                visit(value);
            }
        }
        hir::ExprKind::Struct(_, fields, tail) => {
            for field in *fields {
                visit(field.expr);
            }
            if let hir::StructTailExpr::Base(base) = tail {
                visit(base);
            }
        }
        hir::ExprKind::Let(let_expr) => visit(let_expr.init),
        _ => {}
    }
}

fn visit_recursive_expr_children<'tcx>(
    expr: &'tcx hir::Expr<'tcx>,
    mut visit: impl FnMut(&'tcx hir::Expr<'tcx>),
) {
    match &expr.kind {
        hir::ExprKind::Call(callee, args) => {
            visit(callee);
            for arg in *args {
                visit(arg);
            }
        }
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            visit(receiver);
            for arg in *args {
                visit(arg);
            }
        }
        _ => visit_structural_expr_children(expr, visit),
    }
}

fn find_in_recursive_expr_children<'tcx, T>(
    expr: &'tcx hir::Expr<'tcx>,
    mut visit: impl FnMut(&'tcx hir::Expr<'tcx>) -> Option<T>,
) -> Option<T> {
    let mut found = None;
    visit_recursive_expr_children(expr, |child| {
        if found.is_none() {
            found = visit(child);
        }
    });
    found
}

/// Extract doc comments (`#[doc = "..."]`) from HIR attributes for a given HirId.
fn extract_doc_comments(tcx: TyCtxt<'_>, hir_id: hir::HirId) -> Option<String> {
    let lines: Vec<_> = tcx
        .hir_attrs(hir_id)
        .iter()
        .filter_map(extract_doc_from_attr)
        .map(|comment| comment.strip_prefix(' ').unwrap_or(comment))
        .collect();

    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Extract doc comment text from a single HIR attribute.
fn extract_doc_from_attr(attr: &hir::Attribute) -> Option<&str> {
    match attr {
        hir::Attribute::Parsed(AttributeKind::DocComment { comment, .. }) => Some(comment.as_str()),
        _ => None,
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
        collect_lua_api_from_item(tcx, item, &mut api);
    }

    dedupe_api(&mut api);
    normalize_api_types(&mut api);
    api
}

fn collect_lua_api_from_item<'tcx>(tcx: TyCtxt<'tcx>, item: &hir::Item<'tcx>, api: &mut LuaApi) {
    if let hir::ItemKind::Mod(_, module) = &item.kind {
        for item_id in module.item_ids {
            let child = tcx.hir_item(*item_id);
            collect_lua_api_from_item(tcx, child, api);
        }
    }

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
                        if let Some(class) = extract_class(tcx, item.owner_id.def_id, impl_block) {
                            api.classes.push(class);
                        }
                    } else if (is_into_lua_trait(path) || is_from_lua_trait(path))
                        && let Some(lua_enum) =
                            extract_enum_from_lua_impl(tcx, item.owner_id.def_id)
                        && !api.enums.iter().any(|e| e.name == lua_enum.name)
                    {
                        api.enums.push(lua_enum);
                    }
                }
            }

            for impl_item_ref in impl_block.items {
                let impl_item_id = hir::ImplItemId {
                    owner_id: impl_item_ref.owner_id,
                };
                let impl_item = tcx.hir_impl_item(impl_item_id);
                extract_registrations_from_impl_item(tcx, impl_item, api);
            }
        }
        hir::ItemKind::Fn { .. } => {
            if let Some(module) = extract_lua_module_from_fn(tcx, item) {
                api.modules.push(module);
            }
            extract_registrations_from_fn(tcx, item, api);
            extract_event_emissions_from_fn(tcx, item, api);
        }
        _ => {}
    }
}

fn dedupe_api(api: &mut LuaApi) {
    api.classes.sort_by(|a, b| a.name.cmp(&b.name));
    api.classes.dedup_by(|a, b| a.name == b.name);
    for class in &mut api.classes {
        dedupe_fields(&mut class.fields);
        dedupe_methods(&mut class.methods);
    }

    api.enums.sort_by(|a, b| a.name.cmp(&b.name));
    api.enums.dedup_by(|a, b| a.name == b.name);

    api.modules.sort_by(|a, b| a.name.cmp(&b.name));
    api.modules.dedup_by(|a, b| a.name == b.name);
    for module in &mut api.modules {
        dedupe_fields(&mut module.fields);
        dedupe_functions(&mut module.functions);
    }
    api.modules
        .retain(|module| !is_synthetic_helper_module(module));

    api.global_fields.sort_by(|a, b| a.name.cmp(&b.name));
    api.global_fields.dedup_by(|a, b| a.name == b.name);

    api.global_functions.sort_by(|a, b| a.name.cmp(&b.name));
    api.global_functions.dedup_by(|a, b| a.name == b.name);
}

fn normalize_api_types(api: &mut LuaApi) {
    let known_names: HashSet<String> = api
        .classes
        .iter()
        .map(|class| class.name.clone())
        .chain(api.enums.iter().map(|item| item.name.clone()))
        .chain(api.modules.iter().map(|module| module.name.clone()))
        .collect();

    for class in &mut api.classes {
        for field in &mut class.fields {
            normalize_api_lua_type(&mut field.ty, &known_names);
        }
        for method in &mut class.methods {
            for param in &mut method.params {
                normalize_api_lua_type(&mut param.ty, &known_names);
            }
            for ret in &mut method.returns {
                normalize_api_lua_type(&mut ret.ty, &known_names);
            }
        }
    }

    for module in &mut api.modules {
        for field in &mut module.fields {
            normalize_api_lua_type(&mut field.ty, &known_names);
        }
        for func in &mut module.functions {
            for param in &mut func.params {
                normalize_api_lua_type(&mut param.ty, &known_names);
            }
            for ret in &mut func.returns {
                normalize_api_lua_type(&mut ret.ty, &known_names);
            }
        }
    }

    for field in &mut api.global_fields {
        normalize_api_lua_type(&mut field.ty, &known_names);
    }

    for func in &mut api.global_functions {
        for param in &mut func.params {
            normalize_api_lua_type(&mut param.ty, &known_names);
        }
        for ret in &mut func.returns {
            normalize_api_lua_type(&mut ret.ty, &known_names);
        }
    }
}

fn normalize_api_lua_type(ty: &mut LuaType, known_names: &HashSet<String>) {
    if let LuaType::Class(name) = ty {
        if let Some(parsed) = parse_embedded_lua_type_text(name) {
            *ty = parsed;
        } else if let Some(base) = ref_alias_base_name(name, known_names) {
            *name = base;
        }
    }

    walk_lua_type_mut(ty, &mut |nested| {
        if let LuaType::Union(items) = nested {
            *nested = make_union(std::mem::take(items));
        }
    });

    if let LuaType::Class(name) = ty
        && let Some(parsed) = parse_embedded_lua_type_text(name)
    {
        *ty = parsed;
        normalize_api_lua_type(ty, known_names);
    }
}

fn parse_embedded_lua_type_text(text: &str) -> Option<LuaType> {
    let text = text.trim();
    let union_parts = split_top_level(text, '|');
    if union_parts.len() > 1 {
        return Some(make_union(
            union_parts
                .into_iter()
                .map(parse_embedded_lua_type_atom)
                .collect(),
        ));
    }

    None
}

fn parse_embedded_lua_type_atom(text: &str) -> LuaType {
    match text.trim() {
        "nil" => LuaType::Nil,
        "boolean" => LuaType::Boolean,
        "integer" => LuaType::Integer,
        "number" => LuaType::Number,
        "string" => LuaType::String,
        "table" => LuaType::Table,
        "function" => LuaType::Function,
        "any" => LuaType::Any,
        "thread" => LuaType::Thread,
        other => LuaType::Class(other.to_string()),
    }
}

fn writable_field(name: String, ty: LuaType) -> LuaField {
    LuaField {
        name,
        ty,
        writable: true,
        doc: None,
    }
}

fn readonly_field(name: String, ty: LuaType) -> LuaField {
    LuaField {
        name,
        ty,
        writable: false,
        doc: None,
    }
}

fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut angle = 0usize;
    let mut paren = 0usize;
    let mut bracket = 0usize;

    for (idx, ch) in text.char_indices() {
        match ch {
            '<' => angle += 1,
            '>' => angle = angle.saturating_sub(1),
            '(' => paren += 1,
            ')' => paren = paren.saturating_sub(1),
            '[' => bracket += 1,
            ']' => bracket = bracket.saturating_sub(1),
            _ => {}
        }

        if ch == separator && angle == 0 && paren == 0 && bracket == 0 {
            parts.push(text[start..idx].trim());
            start = idx + ch.len_utf8();
        }
    }

    if start == 0 {
        return vec![text.trim()];
    }

    parts.push(text[start..].trim());
    parts
}

fn ref_alias_base_name(name: &str, known_names: &HashSet<String>) -> Option<String> {
    if known_names.contains(name) || matches!(name, "UserDataRef" | "UserDataRefMut") {
        return None;
    }

    for suffix in ["RefMut", "Ref"] {
        if let Some(base) = name.strip_suffix(suffix)
            && !base.is_empty()
            && base
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_uppercase())
        {
            return Some(base.to_string());
        }
    }

    None
}

fn dedupe_fields(fields: &mut Vec<LuaField>) {
    let mut seen = HashSet::new();
    fields.retain(|field| {
        let inserted = seen.insert(field.name.clone());
        if !inserted && should_trace_field_name(&field.name) {
            trace(format!(
                "dedupe_fields dropped field={}",
                field_trace_summary(field)
            ));
        }
        inserted
    });
}

fn dedupe_methods(methods: &mut Vec<LuaMethod>) {
    let mut seen = HashSet::new();
    methods.retain(|method| {
        let inserted = seen.insert(method_key(method));
        if !inserted && should_trace_method_name(&method.name) {
            trace(format!(
                "dedupe_methods dropped method={}",
                method_trace_summary(method)
            ));
        }
        inserted
    });
}

fn dedupe_functions(functions: &mut Vec<LuaFunction>) {
    let mut seen = HashSet::new();
    functions.retain(|function| seen.insert(function_key(function)));
}

fn is_synthetic_helper_module(module: &LuaModule) -> bool {
    let short_lowercase_name = !module.name.is_empty()
        && module.name.len() <= 2
        && module.name.chars().all(|c| c.is_ascii_lowercase());

    short_lowercase_name
        && module.functions.is_empty()
        && module
            .fields
            .iter()
            .all(|field| field.name.starts_with('_'))
}

#[test]
fn synthetic_helper_module_detection_handles_internal_wrapper_tables() {
    let internal = LuaModule {
        name: "ts".to_string(),
        doc: None,
        functions: Vec::new(),
        fields: vec![LuaField {
            name: "__mod".to_string(),
            ty: LuaType::Table,
            writable: true,
            doc: None,
        }],
    };

    let exported = LuaModule {
        name: "fs".to_string(),
        doc: None,
        functions: vec![LuaFunction {
            name: "cwd".to_string(),
            is_async: false,
            params: Vec::new(),
            returns: Vec::new(),
            doc: None,
        }],
        fields: Vec::new(),
    };

    assert!(is_synthetic_helper_module(&internal));
    assert!(!is_synthetic_helper_module(&exported));
}

#[test]
fn extracted_name_unwraps_userdata_refs() {
    assert_eq!(
        lua_type_from_extracted_name("UserDataRef<File>"),
        LuaType::Class("File".to_string())
    );
    assert_eq!(
        lua_type_from_extracted_name("UserDataRefMut<Url>"),
        LuaType::Class("Url".to_string())
    );
}

#[test]
fn extracted_name_handles_qualified_option_userdata_ref() {
    assert_eq!(
        lua_type_from_extracted_name("Option<mlua::UserDataRef<File>>"),
        LuaType::Optional(Box::new(LuaType::Class("File".to_string())))
    );
}

#[test]
fn table_value_normalization_prefers_specific_userdata_class() {
    let normalized = normalize_table_value_types(vec![
        LuaType::Class("UserDataRef".to_string()),
        LuaType::Class("FileRef".to_string()),
        LuaType::Integer,
    ]);

    assert_eq!(
        normalized,
        vec![LuaType::Class("File".to_string()), LuaType::Integer]
    );
}

#[test]
fn inferred_union_normalization_prefers_specific_userdata_class() {
    let normalized = normalize_inferred_lua_type(LuaType::Union(vec![
        LuaType::Class("UserDataRef".to_string()),
        LuaType::Class("FileRef".to_string()),
        LuaType::Integer,
    ]));

    assert_eq!(
        normalized,
        make_union(vec![LuaType::Class("File".to_string()), LuaType::Integer])
    );
}

#[test]
fn api_type_normalization_rewrites_known_ref_aliases() {
    let mut ty = LuaType::Union(vec![
        LuaType::Class("UserDataRef".to_string()),
        LuaType::Class("FileRef".to_string()),
        LuaType::Integer,
    ]);
    let known = HashSet::from(["File".to_string()]);

    normalize_api_lua_type(&mut ty, &known);

    assert_eq!(
        ty,
        make_union(vec![LuaType::Class("File".to_string()), LuaType::Integer])
    );
}

#[test]
fn api_type_normalization_parses_embedded_union_text() {
    let mut ty = LuaType::Class("UserDataRef | FileRef | integer".to_string());
    let known = HashSet::new();

    normalize_api_lua_type(&mut ty, &known);

    assert_eq!(
        ty,
        make_union(vec![LuaType::Class("File".to_string()), LuaType::Integer])
    );
}

fn method_key(method: &LuaMethod) -> String {
    format!(
        "{}|{:?}|{}|{:?}|{:?}",
        method.name, method.kind, method.is_async, method.params, method.returns
    )
}

fn function_key(function: &LuaFunction) -> String {
    format!(
        "{}|{}|{:?}|{:?}",
        function.name, function.is_async, function.params, function.returns
    )
}

fn is_userdata_trait(path: &str) -> bool {
    path == "mlua::UserData" || path == "mlua::prelude::LuaUserData" || path.ends_with("::UserData")
}

fn is_into_lua_trait(path: &str) -> bool {
    path == "mlua::IntoLua" || path == "mlua::prelude::LuaIntoLua" || path.ends_with("::IntoLua")
}

fn is_from_lua_trait(path: &str) -> bool {
    path == "mlua::FromLua" || path == "mlua::prelude::LuaFromLua" || path.ends_with("::FromLua")
}

// ── #[mlua::lua_module] detection ───────────────────────────────────────

/// Detect `#[mlua::lua_module]` functions and extract the module they build.
fn extract_lua_module_from_fn<'tcx>(
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
    if let Some(export_name) = extract_named_set_call(expr) {
        let NamedSetCall { name, value } = export_name;
        let exported = classify_exported_value(tcx, value, &name, None);
        push_exported_value(module, name, exported, None);
    }

    visit_structural_expr_children(expr, |child| {
        visit_expr_for_module_exports(tcx, child, module);
    });
}

struct NamedSetCall<'tcx> {
    name: String,
    value: &'tcx hir::Expr<'tcx>,
}

fn extract_named_set_call<'tcx>(expr: &'tcx hir::Expr<'tcx>) -> Option<NamedSetCall<'tcx>> {
    match &expr.kind {
        hir::ExprKind::MethodCall(segment, _receiver, args, _span) => {
            let method_name = segment.ident.name.as_str();
            let export_name = ((method_name == "set" || method_name == "raw_set")
                && args.len() >= 2)
                .then(|| extract_string_literal(&args[0]))
                .flatten();

            export_name.map(|name| NamedSetCall {
                name,
                value: &args[1],
            })
        }
        _ => None,
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
    let pascal_variants: Vec<String> = adt_def
        .variants()
        .iter()
        .map(|v| v.name.as_str().to_string())
        .collect();
    let variants: Vec<String> = pascal_variants
        .iter()
        .map(|v| variant_to_lua_string(v))
        .collect();

    if variants.is_empty() {
        return None;
    }

    let doc = extract_type_doc(tcx, self_ty);

    Some(LuaEnum {
        name,
        doc,
        variants,
        pascal_variants,
    })
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
    extract_registrations_from_body(tcx, body.value, api);
}

fn extract_registrations_from_impl_item<'tcx>(
    tcx: TyCtxt<'tcx>,
    impl_item: &hir::ImplItem<'tcx>,
    api: &mut LuaApi,
) {
    let hir::ImplItemKind::Fn(_, body_id) = impl_item.kind else {
        return;
    };
    let body = tcx.hir_body(body_id);
    extract_registrations_from_body(tcx, body.value, api);
}

/// Scan a function body for event emission patterns like:
/// `emit_event(&lua, ("event-name".to_string(), args))` or
/// `emit_sync_callback(&lua, ("event-name", args))`
/// where args come from `lua.pack_multi(typed_value)`.
fn extract_event_emissions_from_fn<'tcx>(
    tcx: TyCtxt<'tcx>,
    item: &hir::Item<'tcx>,
    api: &mut LuaApi,
) {
    let hir::ItemKind::Fn { body, .. } = &item.kind else {
        return;
    };
    let body = tcx.hir_body(*body);
    extract_event_emissions_from_expr(tcx, body.value, api);
}

fn extract_event_emissions_from_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    api: &mut LuaApi,
) {
    match &expr.kind {
        // Look for: some_fn(lua, (event_name, args)) where event_name is a string literal
        hir::ExprKind::Call(callee, call_args) => {
            // Check if any argument is a tuple containing (string_literal, pack_multi_result)
            for arg in call_args.iter() {
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
            // Recurse
            extract_event_emissions_from_expr(tcx, callee, api);
            for arg in call_args.iter() {
                extract_event_emissions_from_expr(tcx, arg, api);
            }
        }
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            extract_event_emissions_from_expr(tcx, receiver, api);
            for arg in args.iter() {
                extract_event_emissions_from_expr(tcx, arg, api);
            }
        }
        hir::ExprKind::Block(block, _) => {
            for stmt in block.stmts {
                match &stmt.kind {
                    hir::StmtKind::Expr(e) | hir::StmtKind::Semi(e) => {
                        extract_event_emissions_from_expr(tcx, e, api);
                    }
                    hir::StmtKind::Let(local) => {
                        if let Some(init) = local.init {
                            extract_event_emissions_from_expr(tcx, init, api);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(tail) = block.expr {
                extract_event_emissions_from_expr(tcx, tail, api);
            }
        }
        hir::ExprKind::Match(scrutinee, arms, _) => {
            extract_event_emissions_from_expr(tcx, scrutinee, api);
            for arm in *arms {
                extract_event_emissions_from_expr(tcx, arm.body, api);
            }
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            extract_event_emissions_from_expr(tcx, cond, api);
            extract_event_emissions_from_expr(tcx, then_expr, api);
            if let Some(e) = else_expr {
                extract_event_emissions_from_expr(tcx, e, api);
            }
        }
        // Async fn bodies are closures wrapping coroutines
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            extract_event_emissions_from_expr(tcx, body.value, api);
        }
        // Recurse through other common wrappers
        hir::ExprKind::DropTemps(inner) | hir::ExprKind::AddrOf(_, _, inner) => {
            extract_event_emissions_from_expr(tcx, inner, api);
        }
        _ => {}
    }
}

/// Extract a string literal from an expression, handling `.to_string()` and `format!` patterns.
fn extract_string_literal_from_expr(_tcx: TyCtxt<'_>, expr: &hir::Expr<'_>) -> Option<String> {
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
fn extract_event_arg_types<'tcx>(tcx: TyCtxt<'tcx>, expr: &'tcx hir::Expr<'tcx>) -> Vec<LuaType> {
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
fn extract_pack_multi_arg_types<'tcx>(
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

fn extract_registrations_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    api: &mut LuaApi,
) {
    let mut ctx = RegistrationCtx {
        tcx,
        api,
        table_bindings: HashMap::new(),
        global_bindings: HashSet::new(),
        local_module_names: HashSet::new(),
        exported_module_names: HashSet::new(),
        module_children: HashMap::new(),
    };
    visit_expr_for_registrations(&mut ctx, expr);
    prune_unexported_registration_modules(
        ctx.api,
        &ctx.local_module_names,
        &ctx.exported_module_names,
    );
}

struct RegistrationCtx<'a, 'tcx> {
    tcx: TyCtxt<'tcx>,
    api: &'a mut LuaApi,
    table_bindings: HashMap<String, usize>,
    global_bindings: HashSet<String>,
    local_module_names: HashSet<String>,
    exported_module_names: HashSet<String>,
    module_children: HashMap<String, Vec<String>>,
}

fn visit_registration_block<'tcx>(
    ctx: &mut RegistrationCtx<'_, 'tcx>,
    block: &'tcx hir::Block<'tcx>,
) {
    for stmt in block.stmts {
        match &stmt.kind {
            hir::StmtKind::Semi(expr) | hir::StmtKind::Expr(expr) => {
                visit_expr_for_registrations(ctx, expr);
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
                    visit_expr_for_registrations(ctx, init);
                }
            }
            _ => {}
        }
    }

    if let Some(expr) = block.expr {
        visit_expr_for_registrations(ctx, expr);
    }
}

fn is_global_registration_target<'tcx>(
    ctx: &RegistrationCtx<'_, 'tcx>,
    receiver: &'tcx hir::Expr<'tcx>,
    receiver_name: Option<&str>,
) -> bool {
    is_globals_call(receiver)
        || receiver_name.is_some_and(|name| ctx.global_bindings.contains(name))
}

fn rename_registered_module_binding<'tcx>(
    ctx: &mut RegistrationCtx<'_, 'tcx>,
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
    ctx: &mut RegistrationCtx<'_, 'tcx>,
    extracted: ExtractedModule,
) {
    trace(format!("extracted global module {}", extracted.module.name));
    let module_name = upsert_extracted_module(ctx.api, extracted);
    ctx.local_module_names.insert(module_name.clone());
    mark_module_exported(ctx, &module_name);
}

fn record_global_registration_field<'tcx>(
    ctx: &mut RegistrationCtx<'_, 'tcx>,
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
    ctx: &mut RegistrationCtx<'_, 'tcx>,
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
    ctx: &mut RegistrationCtx<'_, 'tcx>,
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
    ctx: &mut RegistrationCtx<'_, 'tcx>,
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

fn visit_expr_for_registrations<'tcx>(
    ctx: &mut RegistrationCtx<'_, 'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) {
    match &expr.kind {
        hir::ExprKind::Block(block, _) => {
            visit_registration_block(ctx, block);
        }
        hir::ExprKind::MethodCall(_, receiver, _, _) => {
            if let Some(named_call) = extract_named_set_call(expr) {
                handle_named_registration(ctx, receiver, &named_call.name, named_call.value);
            }

            visit_recursive_expr_children(expr, |child| {
                visit_expr_for_registrations(ctx, child);
            });
        }
        _ => visit_recursive_expr_children(expr, |child| {
            visit_expr_for_registrations(ctx, child);
        }),
    }
}

fn extract_binding_name(pat: &hir::Pat<'_>) -> Option<String> {
    match &pat.kind {
        hir::PatKind::Binding(_, _, ident, _) => Some(ident.name.as_str().to_string()),
        _ => None,
    }
}

fn resolve_module_index<'tcx>(ctx: &RegistrationCtx<'_, 'tcx>, module_name: &str) -> Option<usize> {
    ctx.table_bindings
        .get(module_name)
        .copied()
        .or_else(|| ctx.api.modules.iter().position(|m| m.name == module_name))
}

fn mark_module_exported<'tcx>(ctx: &mut RegistrationCtx<'_, 'tcx>, module_name: &str) {
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
    api: &mut LuaApi,
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

fn upsert_module(api: &mut LuaApi, module: LuaModule) {
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

fn nested_module_name(parent: &str, field_name: &str) -> String {
    format!("{parent}__{}", field_name.to_upper_camel_case())
}

enum ExportedValue {
    Function(LuaFunction),
    NestedModule(ExtractedModule),
    Field(LuaType),
}

fn classify_exported_value<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    name: &str,
    nested_module_name: Option<&str>,
) -> ExportedValue {
    if let Some(func) = try_extract_create_function(tcx, expr, name) {
        return ExportedValue::Function(func);
    }

    if let Some(module_name) = nested_module_name
        && let Some(module) = try_extract_module_from_value_expr(tcx, expr, module_name)
    {
        return ExportedValue::NestedModule(module);
    }

    ExportedValue::Field(infer_value_expr_lua_type(tcx, expr))
}

fn push_exported_value(
    module: &mut LuaModule,
    name: String,
    value: ExportedValue,
    nested_modules: Option<&mut Vec<LuaModule>>,
) {
    match value {
        ExportedValue::Function(func) => module.functions.push(func),
        ExportedValue::NestedModule(extracted) => {
            let nested_name = extracted.module.name.clone();
            module
                .fields
                .push(writable_field(name, LuaType::Class(nested_name)));
            if let Some(nested_modules) = nested_modules {
                nested_modules.extend(extracted.nested_modules);
                nested_modules.push(extracted.module);
            }
        }
        ExportedValue::Field(ty) => module.fields.push(writable_field(name, ty)),
    }
}

struct ExtractedModule {
    module: LuaModule,
    nested_modules: Vec<LuaModule>,
}

fn upsert_extracted_module(api: &mut LuaApi, extracted: ExtractedModule) -> String {
    let root_name = extracted.module.name.clone();
    for module in extracted.nested_modules {
        upsert_module(api, module);
    }
    upsert_module(api, extracted.module);
    root_name
}

fn infer_value_expr_lua_type<'tcx>(tcx: TyCtxt<'tcx>, expr: &'tcx hir::Expr<'tcx>) -> LuaType {
    let expr = peel_lua_conversion_expr(expr);

    if let Some(ty) = infer_wrapper_method_lua_type(tcx, expr)
        && (is_informative(&ty) || matches!(ty, LuaType::Class(_) | LuaType::Optional(_)))
    {
        return ty;
    }

    if let hir::ExprKind::MethodCall(segment, _receiver, args, _) = &expr.kind {
        match segment.ident.name.as_str() {
            "create_sequence_from" => {
                if let Some(values_expr) = args.first()
                    && let Some(item_ty) = infer_sequence_item_lua_type(tcx, values_expr)
                {
                    return LuaType::Array(Box::new(item_ty));
                }
            }
            "to_value" | "to_value_with" => {
                if let Some(value_expr) = args.first() {
                    return infer_value_expr_lua_type(tcx, value_expr);
                }
            }
            _ => {}
        }
    }

    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    if let Some(ty) = try_lua_value_constructor(tcx, typeck, expr) {
        return ty;
    }

    let ty = infer_expr_lua_type(tcx, expr);
    if is_informative(&ty) {
        ty
    } else {
        infer_concrete_type_from_body(tcx, expr).unwrap_or(ty)
    }
}

fn infer_sequence_item_lua_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Array(items) => items
            .first()
            .map(|item| infer_value_expr_lua_type(tcx, item)),
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            match segment.ident.name.as_str() {
                "map" => args
                    .first()
                    .and_then(|mapper| {
                        extract_type_from_closure_expr(tcx, mapper).map(LuaType::Class)
                    })
                    .or_else(|| infer_sequence_item_lua_type(tcx, receiver)),
                "iter" | "into_iter" | "copied" | "cloned" => {
                    sequence_item_type_from_expr(tcx, receiver)
                        .or_else(|| infer_sequence_item_lua_type(tcx, receiver))
                }
                "get_args" => Some(LuaType::String),
                _ => sequence_item_type_from_expr(tcx, expr),
            }
        }
        _ => sequence_item_type_from_expr(tcx, expr),
    }
}

fn sequence_item_type_from_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    sequence_item_type_from_ty(tcx, typeck.expr_ty(expr))
}

fn sequence_item_type_from_ty<'tcx>(tcx: TyCtxt<'tcx>, ty: ty::Ty<'tcx>) -> Option<LuaType> {
    match ty.kind() {
        ty::TyKind::Ref(_, inner, _) => sequence_item_type_from_ty(tcx, *inner),
        ty::TyKind::Slice(inner) | ty::TyKind::Array(inner, _) => Some(map_ty_to_lua(tcx, *inner)),
        ty::TyKind::Adt(adt, args) => {
            let path = tcx.def_path_str(adt.did());
            let name = path.rsplit("::").next().unwrap_or_default();
            match name {
                "Vec" | "VecDeque" | "LinkedList" | "HashSet" | "BTreeSet" | "IndexSet" => {
                    args.types().next().map(|inner| map_ty_to_lua(tcx, inner))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn infer_wrapper_method_lua_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);
    let hir::ExprKind::MethodCall(segment, _receiver, args, _) = &expr.kind else {
        return None;
    };

    match segment.ident.name.as_str() {
        "create_userdata" | "create_any_userdata" | "create_any_userdata_ref" => args
            .first()
            .map(|value_expr| infer_expr_lua_type(tcx, value_expr)),
        "named_registry_value" => infer_named_registry_value_lua_type(tcx, expr),
        "create_function" | "create_async_function" => args
            .first()
            .and_then(|closure| infer_created_function_signature(tcx, closure))
            .or(Some(LuaType::Function)),
        "create_table" => Some(LuaType::Table),
        "create_table_from" => args
            .first()
            .and_then(|entries| infer_table_from_entries_lua_type(tcx, entries)),
        "create_sequence_from" => args
            .first()
            .and_then(|values_expr| infer_sequence_item_lua_type(tcx, values_expr))
            .map(|item_ty| LuaType::Array(Box::new(item_ty))),
        "to_value" | "to_value_with" => Some(lua_runtime_type()),
        _ => None,
    }
}

fn infer_named_registry_value_lua_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let hir::ExprKind::MethodCall(segment, _receiver, args, _) = &expr.kind else {
        return None;
    };
    let key = extract_string_literal(args.first()?)?;
    let generic_ty = method_generic_lua_type(tcx, expr, segment)?;

    if !matches!(generic_ty, LuaType::Any) {
        return Some(generic_ty);
    }

    let body = owner_body(tcx, expr.hir_id.owner.def_id)?;
    let mut value_types = collect_named_registry_value_types(tcx, body.value, &key);
    if value_types.is_empty() {
        Some(LuaType::Any)
    } else {
        Some(make_union(std::mem::take(&mut value_types)))
    }
}

fn collect_named_registry_value_types<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    key: &str,
) -> Vec<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            let mut tys = Vec::new();
            if segment.ident.name.as_str() == "set_named_registry_value"
                && args.len() >= 2
                && extract_string_literal(&args[0]).as_deref() == Some(key)
            {
                tys.push(infer_value_expr_lua_type(tcx, &args[1]));
            }

            tys.extend(collect_named_registry_value_types(tcx, receiver, key));
            for arg in *args {
                tys.extend(collect_named_registry_value_types(tcx, arg, key));
            }
            tys
        }
        hir::ExprKind::Call(callee, args) => {
            let mut tys = collect_named_registry_value_types(tcx, callee, key);
            for arg in *args {
                tys.extend(collect_named_registry_value_types(tcx, arg, key));
            }
            tys
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .flat_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local
                    .init
                    .map(|init| collect_named_registry_value_types(tcx, init, key))
                    .unwrap_or_default(),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    collect_named_registry_value_types(tcx, expr, key)
                }
                _ => Vec::new(),
            })
            .chain(
                block
                    .expr
                    .into_iter()
                    .flat_map(|expr| collect_named_registry_value_types(tcx, expr, key)),
            )
            .collect(),
        hir::ExprKind::Match(scrutinee, arms, _) => {
            let mut tys = collect_named_registry_value_types(tcx, scrutinee, key);
            for arm in *arms {
                tys.extend(collect_named_registry_value_types(tcx, arm.body, key));
            }
            tys
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            let mut tys = collect_named_registry_value_types(tcx, cond, key);
            tys.extend(collect_named_registry_value_types(tcx, then_expr, key));
            if let Some(else_expr) = else_expr {
                tys.extend(collect_named_registry_value_types(tcx, else_expr, key));
            }
            tys
        }
        hir::ExprKind::Assign(lhs, rhs, _) | hir::ExprKind::AssignOp(_, lhs, rhs) => {
            let mut tys = collect_named_registry_value_types(tcx, lhs, key);
            tys.extend(collect_named_registry_value_types(tcx, rhs, key));
            tys
        }
        hir::ExprKind::Binary(_, lhs, rhs) => {
            let mut tys = collect_named_registry_value_types(tcx, lhs, key);
            tys.extend(collect_named_registry_value_types(tcx, rhs, key));
            tys
        }
        hir::ExprKind::Let(let_expr) => collect_named_registry_value_types(tcx, let_expr.init, key),
        hir::ExprKind::Array(items) | hir::ExprKind::Tup(items) => items
            .iter()
            .flat_map(|item| collect_named_registry_value_types(tcx, item, key))
            .collect(),
        hir::ExprKind::Struct(_, fields, tail) => fields
            .iter()
            .flat_map(|field| collect_named_registry_value_types(tcx, field.expr, key))
            .chain(match tail {
                hir::StructTailExpr::Base(base) => {
                    collect_named_registry_value_types(tcx, base, key)
                }
                _ => Vec::new(),
            })
            .collect(),
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            collect_named_registry_value_types(tcx, body.value, key)
        }
        hir::ExprKind::Ret(Some(inner)) => collect_named_registry_value_types(tcx, inner, key),
        _ => Vec::new(),
    }
}

fn infer_created_function_signature<'tcx>(
    tcx: TyCtxt<'tcx>,
    closure_expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let (params, returns) = extract_standalone_closure_signature(tcx, closure_expr)?;
    Some(LuaType::FunctionSig {
        params: params.into_iter().map(|param| param.ty).collect(),
        returns: returns.into_iter().map(|ret| ret.ty).collect(),
    })
}

fn is_passthrough_method(name: &str) -> bool {
    matches!(
        name,
        "and_then"
            | "map"
            | "map_err"
            | "ok"
            | "err"
            | "into"
            | "into_lua_err"
            | "with_context"
            | "context"
            | "clone"
            | "to_owned"
            | "borrow"
            | "as_ref"
    )
}

fn infer_table_from_entries_lua_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);
    let hir::ExprKind::Array(entries) = &expr.kind else {
        return Some(LuaType::Table);
    };

    let mut value_types = Vec::new();
    for entry in *entries {
        let hir::ExprKind::Tup(parts) = &entry.kind else {
            return Some(LuaType::Table);
        };
        if parts.len() != 2 || extract_string_literal(&parts[0]).is_none() {
            return Some(LuaType::Table);
        }
        value_types.push(infer_value_expr_lua_type(tcx, &parts[1]));
    }

    if value_types.is_empty() {
        return Some(LuaType::Table);
    }

    let value_ty = if value_types.len() == 1 {
        value_types.pop().unwrap_or(LuaType::Any)
    } else {
        make_union(value_types)
    };

    Some(LuaType::Map(Box::new(LuaType::String), Box::new(value_ty)))
}

fn should_replace_returns(existing: &[LuaReturn], inferred: &[LuaReturn]) -> bool {
    if inferred.is_empty() {
        return !existing.is_empty();
    }

    if inferred.iter().all(|ret| !is_informative(&ret.ty)) {
        return false;
    }

    if existing.is_empty() {
        return true;
    }

    if existing.len() != inferred.len() {
        return existing.iter().all(|ret| !is_informative(&ret.ty));
    }

    existing
        .iter()
        .zip(inferred)
        .all(|(existing, inferred)| should_prefer_body_inference(&existing.ty, &inferred.ty))
}

fn merge_inferred_returns(returns: &mut Vec<LuaReturn>, inferred: Option<Vec<LuaReturn>>) {
    if let Some(inferred) = inferred
        && should_replace_returns(returns, &inferred)
    {
        *returns = inferred;
    }
}

fn is_erased_body_type(ty: &LuaType) -> bool {
    match ty {
        LuaType::Any | LuaType::String => true,
        LuaType::Optional(inner) => is_erased_body_type(inner),
        _ => false,
    }
}

fn should_prefer_body_inference(existing: &LuaType, inferred: &LuaType) -> bool {
    if !is_informative(inferred) {
        return false;
    }

    is_better_inferred_type(existing, inferred)
        || !is_informative(existing)
        || (is_erased_body_type(existing) && !is_erased_body_type(inferred))
}

fn rewrap_erased_body_inference(existing: &LuaType, inferred: &LuaType) -> LuaType {
    match existing {
        LuaType::Optional(inner)
            if is_erased_body_type(inner) && !matches!(inferred, LuaType::Optional(_)) =>
        {
            LuaType::Optional(Box::new(inferred.clone()))
        }
        _ => inferred.clone(),
    }
}

fn merge_body_inference(best: Option<LuaType>, candidate: LuaType) -> Option<LuaType> {
    if !is_informative(&candidate) {
        return best;
    }

    match best {
        Some(current) if should_prefer_body_inference(&current, &candidate) => {
            Some(rewrap_erased_body_inference(&current, &candidate))
        }
        Some(current) => Some(current),
        None => Some(candidate),
    }
}

fn is_better_inferred_type(existing: &LuaType, inferred: &LuaType) -> bool {
    if let LuaType::Union(items) = inferred {
        return items
            .iter()
            .any(|item| is_better_inferred_type(existing, item));
    }

    match existing {
        LuaType::Any | LuaType::Nil => is_informative(inferred),
        LuaType::Function => matches!(inferred, LuaType::FunctionSig { .. }),
        LuaType::String => matches!(inferred, LuaType::StringLiteral(_)),
        LuaType::Variadic(inner) => match inferred {
            LuaType::Variadic(inferred_inner) => {
                is_better_inferred_type(inner, inferred_inner) || !is_informative(inner)
            }
            _ => false,
        },
        LuaType::Table => matches!(
            inferred,
            LuaType::Array(_) | LuaType::Map(_, _) | LuaType::Class(_) | LuaType::StringLiteral(_)
        ),
        LuaType::Optional(inner) => is_better_inferred_type(inner, inferred),
        _ => false,
    }
}

fn create_table_module_from_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    module_name: &str,
) -> Option<ExtractedModule> {
    let expr = peel_lua_conversion_expr(expr);
    let hir::ExprKind::MethodCall(segment, _receiver, args, _) = &expr.kind else {
        return None;
    };

    let method = segment.ident.name.as_str();
    if method != "create_table" && method != "create_table_from" {
        return None;
    }

    let mut module = LuaModule {
        name: module_name.to_string(),
        doc: None,
        functions: Vec::new(),
        fields: Vec::new(),
    };
    let mut nested_modules = Vec::new();

    if method == "create_table_from" && !args.is_empty() {
        populate_module_from_table_entries(
            tcx,
            &args[0],
            module_name,
            &mut module,
            &mut nested_modules,
        );
    }

    Some(ExtractedModule {
        module,
        nested_modules,
    })
}

/// Detect calls like `get_or_create_module(lua, "wezterm")` or
/// `get_or_create_sub_module(lua, "color")` and extract the module name
/// from the string literal argument.
fn try_extract_module_name_from_call<'tcx>(
    _tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<String> {
    let expr = peel_lua_conversion_expr(expr);

    match &expr.kind {
        hir::ExprKind::Call(callee, args) => {
            // Free function call: get_or_create_module(lua, "name")
            let callee_name = path_tail_name(callee)?;
            if !callee_name.contains("module") {
                return None;
            }
            // Extract the string literal argument (usually the last one).
            args.iter().rev().find_map(extract_string_literal)
        }
        hir::ExprKind::MethodCall(segment, _receiver, args, _) => {
            // Method call: lua.create_named_module("name") or similar
            let method = segment.ident.name.as_str();
            if !method.contains("module") {
                return None;
            }
            args.iter().rev().find_map(extract_string_literal)
        }
        _ => None,
    }
}

fn populate_module_from_table_entries<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    module_name: &str,
    module: &mut LuaModule,
    nested_modules: &mut Vec<LuaModule>,
) {
    let hir::ExprKind::Array(entries) = &expr.kind else {
        return;
    };

    for entry in *entries {
        let hir::ExprKind::Tup(parts) = &entry.kind else {
            continue;
        };
        if parts.len() != 2 {
            continue;
        }

        let Some(name) = extract_string_literal(&parts[0]) else {
            continue;
        };

        let exported = classify_exported_value(
            tcx,
            &parts[1],
            &name,
            Some(&nested_module_name(module_name, &name)),
        );
        push_exported_value(module, name, exported, Some(nested_modules));
    }
}

fn try_extract_module_from_value_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    module_name: &str,
) -> Option<ExtractedModule> {
    trace(format!(
        "try module {} from {}",
        module_name,
        expr_snippet(tcx, expr)
    ));
    if let Some(module) = create_table_module_from_expr(tcx, expr, module_name) {
        trace(format!("module {} from table expr", module.module.name));
        return Some(module);
    }

    let expr = peel_lua_conversion_expr(expr);
    let hir::ExprKind::Call(callee, _) = &expr.kind else {
        return None;
    };
    let def_id = expr_def_id(tcx, callee)?;
    let local = def_id.as_local()?;
    trace(format!("module {} from local fn {:?}", module_name, local));
    extract_module_from_local_fn(tcx, local, module_name)
}

fn extract_module_from_local_fn<'tcx>(
    tcx: TyCtxt<'tcx>,
    def_id: LocalDefId,
    module_name: &str,
) -> Option<ExtractedModule> {
    trace(format!(
        "extract module {} from def {:?}",
        module_name, def_id
    ));
    let rustc_hir::Node::Item(item) = tcx.hir_node_by_def_id(def_id) else {
        trace("module def is not an item");
        return None;
    };
    let hir::ItemKind::Fn { body, .. } = item.kind else {
        return None;
    };
    let body = tcx.hir_body(body);
    extract_composer_module_from_expr(tcx, body.value, module_name)
}

fn extract_composer_module_from_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    module_name: &str,
) -> Option<ExtractedModule> {
    match &expr.kind {
        hir::ExprKind::Block(block, _) => {
            for stmt in block.stmts {
                if let hir::StmtKind::Item(item_id) = stmt.kind {
                    let item = tcx.hir_item(item_id);
                    if item
                        .kind
                        .ident()
                        .is_some_and(|ident| ident.name.as_str() == "get")
                    {
                        return extract_composer_get_module(tcx, item, module_name);
                    }
                }
            }
            block
                .expr
                .and_then(|tail| extract_composer_module_from_expr(tcx, tail, module_name))
        }
        _ => None,
    }
}

fn extract_composer_get_module<'tcx>(
    tcx: TyCtxt<'tcx>,
    item: &hir::Item<'tcx>,
    module_name: &str,
) -> Option<ExtractedModule> {
    let hir::ItemKind::Fn { body, .. } = item.kind else {
        return None;
    };
    let body = tcx.hir_body(body);
    let match_expr = find_match_expr(body.value)?;

    let mut module = LuaModule {
        name: module_name.to_string(),
        doc: None,
        functions: Vec::new(),
        fields: Vec::new(),
    };
    let mut nested_modules = Vec::new();

    let hir::ExprKind::Match(_, arms, _) = &match_expr.kind else {
        return None;
    };

    for arm in *arms {
        let Some(key) = extract_bytes_key_from_pat(tcx, arm.pat) else {
            continue;
        };
        let value_expr = unwrap_try_expr(arm.body);

        if let Some(func) = try_extract_function_value(tcx, value_expr, &key) {
            module.functions.push(func);
            continue;
        }

        let exported = classify_exported_value(
            tcx,
            value_expr,
            &key,
            Some(&nested_module_name(module_name, &key)),
        );
        push_exported_value(&mut module, key, exported, Some(&mut nested_modules));
    }

    Some(ExtractedModule {
        module,
        nested_modules,
    })
}

fn find_match_expr<'tcx>(expr: &'tcx hir::Expr<'tcx>) -> Option<&'tcx hir::Expr<'tcx>> {
    match &expr.kind {
        hir::ExprKind::Match(_, _, _) => Some(expr),
        hir::ExprKind::Block(block, _) => block.expr.and_then(find_match_expr),
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            find_match_expr(receiver).or_else(|| args.iter().find_map(find_match_expr))
        }
        hir::ExprKind::Call(callee, args) => {
            find_match_expr(callee).or_else(|| args.iter().find_map(find_match_expr))
        }
        hir::ExprKind::DropTemps(inner)
        | hir::ExprKind::Use(inner, _)
        | hir::ExprKind::Cast(inner, _)
        | hir::ExprKind::Type(inner, _)
        | hir::ExprKind::AddrOf(_, _, inner)
        | hir::ExprKind::Unary(_, inner)
        | hir::ExprKind::Field(inner, _)
        | hir::ExprKind::Become(inner)
        | hir::ExprKind::Yield(inner, _)
        | hir::ExprKind::UnsafeBinderCast(_, inner, _) => find_match_expr(inner),
        _ => None,
    }
}

fn extract_bytes_key_from_pat(tcx: TyCtxt<'_>, pat: &hir::Pat<'_>) -> Option<String> {
    let snippet = tcx.sess.source_map().span_to_snippet(pat.span).ok()?;
    let snippet = snippet.trim();
    if snippet == "_" {
        return None;
    }
    let snippet = snippet.strip_prefix("b\"")?.strip_suffix('"')?;
    Some(snippet.to_string())
}

fn try_extract_function_value<'tcx>(
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

fn extract_function_from_local_fn(
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

fn find_create_function_in_expr<'tcx>(
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
fn try_extract_create_function<'tcx>(
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

/// Unwrap `expr?` → `expr` (strip the Try/Match desugaring).
/// Check if an expression is a `?` (try) desugaring — these are error paths, not success returns.
fn is_try_desugar_expr(expr: &hir::Expr<'_>) -> bool {
    let expr = peel_expr(expr);
    matches!(
        &expr.kind,
        hir::ExprKind::Match(_, _, hir::MatchSource::TryDesugar(_))
    ) || matches!(&expr.kind, hir::ExprKind::Call(_, args) if args.first().is_some_and(|a|
        matches!(&a.kind, hir::ExprKind::Match(_, _, hir::MatchSource::TryDesugar(_)))))
}

fn unwrap_try_expr<'tcx>(expr: &'tcx hir::Expr<'tcx>) -> &'tcx hir::Expr<'tcx> {
    // In HIR, `expr?` desugars to a match. Just try to look through it.
    if let hir::ExprKind::Match(scrutinee, _, hir::MatchSource::TryDesugar(_)) = &expr.kind {
        return unwrap_try_expr(scrutinee);
    }
    if let hir::ExprKind::Call(callee, args) = &expr.kind {
        if let Some(first) = args.first()
            && let hir::ExprKind::Match(scrutinee, _, hir::MatchSource::TryDesugar(_)) = &first.kind
        {
            return unwrap_try_expr(scrutinee);
        }
        if args.len() == 1
            && path_tail_name(callee).is_some_and(|name| {
                matches!(name.as_str(), "branch" | "from_residual" | "from_output")
            })
        {
            return unwrap_try_expr(&args[0]);
        }
    }
    expr
}

/// Extract params and return types from a standalone closure (not on UserData).
/// Closure signature: `|lua: &Lua, (p1, p2, ...): (T1, T2, ...)| -> Result<R, Error>`
fn extract_standalone_closure_signature(
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
fn extract_named_fn_callback_signature<'tcx>(
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
        && let Ok(snippet) = tcx.sess.source_map().span_to_snippet(ret_hir_ty.span)
    {
        let snippet = snippet.trim();
        // Unwrap Result<T, _> or mlua::Result<T>
        let inner = snippet
            .strip_prefix("mlua::Result<")
            .or_else(|| snippet.strip_prefix("Result<"))
            .and_then(|s| s.strip_suffix('>'))
            .unwrap_or(snippet);
        if inner == "()" || inner.is_empty() {
            returns = vec![];
        } else {
            let ty = lua_type_from_extracted_name(inner);
            if is_informative(&ty) {
                returns = vec![ty.into()];
            }
        }
    }

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
    for field in &mut fields {
        replace_self_return_sentinel(&mut field.ty, &class_name);
        replace_self_class_alias(&mut field.ty, &class_name);
    }
    for method in &mut methods {
        for param in &mut method.params {
            replace_self_class_alias(&mut param.ty, &class_name);
        }
        for ret in &mut method.returns {
            replace_self_return_sentinel(&mut ret.ty, &class_name);
            replace_self_class_alias(&mut ret.ty, &class_name);
        }
    }

    trace_class_snapshot("extract_class", &class_name, &fields, &methods);

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

fn enclosing_impl_self_type_name(tcx: TyCtxt<'_>, mut local: LocalDefId) -> Option<String> {
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
        // Walk into closure bodies (e.g. immediately-invoked closures)
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            visit_expr_for_methods(tcx, body.value, methods);
        }
        hir::ExprKind::MethodCall(segment, _receiver, args, _span) => {
            let method_name = segment.ident.name.as_str();
            let is_async = method_name.starts_with("add_async_");
            let registration_name = args.first().and_then(extract_string_literal);
            if let Some(name) = registration_name.as_deref()
                && should_trace_method_name(name)
                && is_method_registration_name(method_name)
            {
                trace(format!(
                    "visit_expr_for_methods saw registrar={method_name} name={registration_name:?} receiver=methods call={}",
                    expr_snippet(tcx, expr)
                ));
            }
            match method_name {
                // All method variants (immutable, mutable, once, async)
                "add_method"
                | "add_method_mut"
                | "add_method_once"
                | "add_async_method"
                | "add_async_method_mut"
                | "add_async_method_once" => {
                    if let Some(m) =
                        extract_registered_method(tcx, args, MethodKind::Method, is_async)
                    {
                        methods.push(m);
                    }
                }
                // All function variants (immutable, mutable, async)
                "add_function" | "add_function_mut" | "add_async_function" => {
                    if let Some(m) =
                        extract_registered_method(tcx, args, MethodKind::Function, is_async)
                    {
                        methods.push(m);
                    }
                }
                // All meta method variants
                "add_meta_method"
                | "add_meta_method_mut"
                | "add_async_meta_method"
                | "add_async_meta_method_mut" => {
                    if let Some(m) = extract_meta_method(tcx, args, MethodKind::Method, is_async) {
                        methods.push(m);
                    }
                }
                // All meta function variants
                "add_meta_function" | "add_meta_function_mut" | "add_async_meta_function" => {
                    if let Some(m) = extract_meta_method(tcx, args, MethodKind::Function, is_async)
                    {
                        methods.push(m);
                    }
                }
                _ => {}
            }
        }
        _ => {
            visit_recursive_expr_children(expr, |child| {
                visit_expr_for_methods(tcx, child, methods);
            });
        }
    }
}

fn extract_registered_method<'tcx>(
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

fn extract_meta_method<'tcx>(
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

fn infer_forwarded_pairs_returns_from_closure<'tcx>(
    tcx: TyCtxt<'tcx>,
    closure_expr: &'tcx hir::Expr<'tcx>,
) -> Option<Vec<LuaReturn>> {
    let hir::ExprKind::Closure(closure) = &closure_expr.kind else {
        return None;
    };

    let body = tcx.hir_body(closure.body);
    find_forwarded_pairs_returns_in_expr(tcx, body.value)
}

fn find_forwarded_pairs_returns_in_expr<'tcx>(
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

fn extract_string_literal(expr: &hir::Expr<'_>) -> Option<String> {
    if let hir::ExprKind::Lit(lit) = &expr.kind
        && let rustc_ast::ast::LitKind::Str(sym, _) = &lit.node
    {
        return Some(sym.as_str().to_string());
    }
    None
}

fn peel_expr<'tcx>(mut expr: &'tcx hir::Expr<'tcx>) -> &'tcx hir::Expr<'tcx> {
    loop {
        match &expr.kind {
            hir::ExprKind::DropTemps(inner)
            | hir::ExprKind::Use(inner, _)
            | hir::ExprKind::Cast(inner, _)
            | hir::ExprKind::Type(inner, _)
            | hir::ExprKind::AddrOf(_, _, inner)
            | hir::ExprKind::Unary(_, inner) => expr = inner,
            _ => return expr,
        }
    }
}

fn peel_try_expr<'tcx>(mut expr: &'tcx hir::Expr<'tcx>) -> &'tcx hir::Expr<'tcx> {
    loop {
        let peeled = peel_expr(expr);
        let unwrapped = unwrap_try_expr(peeled);
        if std::ptr::eq(unwrapped, expr) {
            return expr;
        }
        expr = unwrapped;
    }
}

/// Resolve a function receiver to a `table.get("name")` call, returning the full
/// `module.function_name` path for cross-crate lookup.
/// Traces through local variable bindings: `let func: Function = table.get("name")?`
fn resolve_table_get_function_name(tcx: TyCtxt<'_>, expr: &hir::Expr<'_>) -> Option<String> {
    // Direct: table.get("name")
    if let Some(name) = extract_table_get_name(tcx, expr) {
        return Some(name);
    }
    // Local variable: resolve to initializer
    if let hir::ExprKind::Path(hir::QPath::Resolved(_, path)) = &expr.kind
        && let rustc_hir::def::Res::Local(hir_id) = path.res
        && let rustc_hir::Node::Pat(pat) = tcx.hir_node(hir_id)
        && let rustc_hir::Node::LetStmt(local) = tcx.hir_node(tcx.parent_hir_id(pat.hir_id))
        && let Some(init) = local.init
    {
        let init = peel_try_expr(init);
        return extract_table_get_name(tcx, init);
    }
    None
}

/// Extract the function name from `table.get("name")` method calls.
/// Returns `module_name.function_name` if the receiver is itself a `get("module")` on a module.
fn extract_table_get_name(tcx: TyCtxt<'_>, expr: &hir::Expr<'_>) -> Option<String> {
    let hir::ExprKind::MethodCall(segment, receiver, args, _) = &expr.kind else {
        return None;
    };
    if segment.ident.name.as_str() != "get" || args.is_empty() {
        return None;
    }
    let func_name = extract_string_literal_from_expr(tcx, &args[0])?;

    // Try to get module name from receiver: module_table.get("func_name")
    // where module_table = parent.get("module_name")
    let module_name = resolve_table_get_module_name(tcx, receiver);
    if let Some(module) = module_name {
        Some(format!("{module}.{func_name}"))
    } else {
        Some(func_name)
    }
}

/// Resolve the module name from a table receiver: `wezterm_mod.get("gui")` → "gui"
fn resolve_table_get_module_name(tcx: TyCtxt<'_>, expr: &hir::Expr<'_>) -> Option<String> {
    // Direct table.get("module_name")
    if let hir::ExprKind::MethodCall(segment, _, args, _) = &expr.kind
        && segment.ident.name.as_str() == "get"
        && !args.is_empty()
    {
        return extract_string_literal_from_expr(tcx, &args[0]);
    }
    // Local variable
    if let hir::ExprKind::Path(hir::QPath::Resolved(_, path)) = &expr.kind
        && let rustc_hir::def::Res::Local(hir_id) = path.res
        && let rustc_hir::Node::Pat(pat) = tcx.hir_node(hir_id)
        && let rustc_hir::Node::LetStmt(local) = tcx.hir_node(tcx.parent_hir_id(pat.hir_id))
        && let Some(init) = local.init
    {
        let init = peel_try_expr(init);
        return resolve_table_get_module_name(tcx, init);
    }
    None
}

/// Extract a cross-module function call from a closure snippet.
/// Looks for patterns like `.get("func_name")` followed by `.call` or `.call_async`.
/// Returns `module.func_name` or just `func_name` for cross-crate lookup.
fn extract_cross_module_call_from_snippet(snippet: &str) -> Option<String> {
    // Find the last .get("name") before a .call / .call_async
    let call_pos = snippet
        .rfind(".call_async")
        .or_else(|| snippet.rfind(".call("))?;
    let before_call = &snippet[..call_pos];

    // Find the last .get("name") pattern
    let get_pos = before_call.rfind(".get(")?;
    let after_get = &before_call[get_pos + 5..]; // skip ".get("
    let func_name = after_get.split('"').nth(1)?;
    if func_name.is_empty() {
        return None;
    }

    // Try to find the module name from an earlier .get("module")
    let before_func_get = &before_call[..get_pos];
    if let Some(mod_get_pos) = before_func_get.rfind(".get(") {
        let after_mod_get = &before_func_get[mod_get_pos + 5..];
        if let Some(mod_name) = after_mod_get.split('"').nth(1)
            && !mod_name.is_empty()
        {
            return Some(format!("{mod_name}.{func_name}"));
        }
    }

    Some(func_name.to_string())
}

/// Peel `.to_dynamic()` method calls to get the original typed receiver.
fn peel_to_dynamic<'tcx>(expr: &'tcx hir::Expr<'tcx>) -> &'tcx hir::Expr<'tcx> {
    if let hir::ExprKind::MethodCall(segment, receiver, _, _) = &expr.kind
        && segment.ident.name.as_str() == "to_dynamic"
    {
        return receiver;
    }
    expr
}

fn peel_lua_conversion_expr<'tcx>(mut expr: &'tcx hir::Expr<'tcx>) -> &'tcx hir::Expr<'tcx> {
    loop {
        expr = peel_try_expr(expr);
        match &expr.kind {
            hir::ExprKind::MethodCall(segment, receiver, _, _)
                if matches!(segment.ident.name.as_str(), "into_lua" | "into_lua_multi") =>
            {
                expr = receiver;
            }
            _ => return expr,
        }
    }
}

fn path_tail_name(expr: &hir::Expr<'_>) -> Option<String> {
    let expr = peel_expr(expr);
    match &expr.kind {
        hir::ExprKind::Path(hir::QPath::Resolved(_, path)) => path
            .segments
            .last()
            .map(|seg| seg.ident.name.as_str().to_string()),
        hir::ExprKind::Path(hir::QPath::TypeRelative(_, seg)) => {
            Some(seg.ident.name.as_str().to_string())
        }
        _ => None,
    }
}

fn expr_def_id<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<rustc_hir::def_id::DefId> {
    let expr = peel_expr(expr);
    let hir::ExprKind::Path(qpath) = &expr.kind else {
        return None;
    };

    match qpath {
        hir::QPath::Resolved(_, path) => path.res.opt_def_id(),
        hir::QPath::TypeRelative(_, _) => {
            let typeck = tcx.typeck(expr.hir_id.owner.def_id);
            match typeck.qpath_res(qpath, expr.hir_id) {
                rustc_hir::def::Res::Def(_, def_id) => Some(def_id),
                _ => None,
            }
        }
    }
}

fn owner_body<'tcx>(tcx: TyCtxt<'tcx>, owner: LocalDefId) -> Option<&'tcx hir::Body<'tcx>> {
    match tcx.hir_node_by_def_id(owner) {
        rustc_hir::Node::Item(item) => {
            let hir::ItemKind::Fn { body, .. } = item.kind else {
                return None;
            };
            Some(tcx.hir_body(body))
        }
        rustc_hir::Node::ImplItem(item) => {
            let hir::ImplItemKind::Fn(_, body) = item.kind else {
                return None;
            };
            Some(tcx.hir_body(body))
        }
        rustc_hir::Node::Expr(expr) => {
            let hir::ExprKind::Closure(closure) = expr.kind else {
                return None;
            };
            Some(tcx.hir_body(closure.body))
        }
        _ => None,
    }
}

fn find_local_binding_init<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<&'tcx hir::Expr<'tcx>> {
    let expr = peel_expr(expr);
    let hir::ExprKind::Path(qpath) = &expr.kind else {
        return None;
    };

    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    let rustc_hir::def::Res::Local(target) = typeck.qpath_res(qpath, expr.hir_id) else {
        return None;
    };

    let body = owner_body(tcx, expr.hir_id.owner.def_id)?;
    find_binding_init_in_expr(body.value, target)
}

fn local_binding_info<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<(hir::HirId, &'tcx hir::Body<'tcx>, String)> {
    let expr = peel_expr(expr);
    let hir::ExprKind::Path(qpath) = &expr.kind else {
        return None;
    };

    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    let rustc_hir::def::Res::Local(target) = typeck.qpath_res(qpath, expr.hir_id) else {
        return None;
    };

    let body = owner_body(tcx, expr.hir_id.owner.def_id)?;
    Some((target, body, path_tail_name(expr)?))
}

fn infer_local_table_binding_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let (target, body, binding_name) = local_binding_info(tcx, expr)?;
    let init = find_binding_init_in_expr(body.value, target)?;
    let init = peel_lua_conversion_expr(init);
    let hir::ExprKind::MethodCall(segment, _receiver, _args, _) = &init.kind else {
        return None;
    };

    if !matches!(
        segment.ident.name.as_str(),
        "create_table" | "create_table_with_capacity"
    ) {
        return None;
    }

    let mut value_types = collect_table_binding_value_types(tcx, body.value, &binding_name);
    if value_types.is_empty() {
        return Some(LuaType::Table);
    }

    Some(LuaType::Map(
        Box::new(LuaType::String),
        Box::new(make_union(std::mem::take(&mut value_types))),
    ))
}

fn collect_table_binding_value_types<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    binding_name: &str,
) -> Vec<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            let mut tys = Vec::new();
            if matches!(segment.ident.name.as_str(), "set" | "raw_set")
                && args.len() >= 2
                && expr_refers_to_binding(receiver, binding_name)
                && extract_string_literal(&args[0]).is_some()
            {
                tys.push(infer_value_expr_lua_type(tcx, &args[1]));
            }
            tys.extend(collect_table_binding_value_types(
                tcx,
                receiver,
                binding_name,
            ));
            for arg in *args {
                tys.extend(collect_table_binding_value_types(tcx, arg, binding_name));
            }
            tys
        }
        hir::ExprKind::Call(callee, args) => {
            let mut tys = collect_table_binding_value_types(tcx, callee, binding_name);
            for arg in *args {
                tys.extend(collect_table_binding_value_types(tcx, arg, binding_name));
            }
            tys
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .flat_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local
                    .init
                    .map(|init| collect_table_binding_value_types(tcx, init, binding_name))
                    .unwrap_or_default(),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    collect_table_binding_value_types(tcx, expr, binding_name)
                }
                _ => Vec::new(),
            })
            .chain(
                block
                    .expr
                    .into_iter()
                    .flat_map(|expr| collect_table_binding_value_types(tcx, expr, binding_name)),
            )
            .collect(),
        hir::ExprKind::Match(scrutinee, arms, _) => {
            let mut tys = collect_table_binding_value_types(tcx, scrutinee, binding_name);
            for arm in *arms {
                tys.extend(collect_table_binding_value_types(
                    tcx,
                    arm.body,
                    binding_name,
                ));
            }
            tys
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            let mut tys = collect_table_binding_value_types(tcx, cond, binding_name);
            tys.extend(collect_table_binding_value_types(
                tcx,
                then_expr,
                binding_name,
            ));
            if let Some(else_expr) = else_expr {
                tys.extend(collect_table_binding_value_types(
                    tcx,
                    else_expr,
                    binding_name,
                ));
            }
            tys
        }
        hir::ExprKind::Assign(lhs, rhs, _) | hir::ExprKind::AssignOp(_, lhs, rhs) => {
            let mut tys = collect_table_binding_value_types(tcx, lhs, binding_name);
            tys.extend(collect_table_binding_value_types(tcx, rhs, binding_name));
            tys
        }
        hir::ExprKind::Let(let_expr) => {
            collect_table_binding_value_types(tcx, let_expr.init, binding_name)
        }
        hir::ExprKind::Array(items) | hir::ExprKind::Tup(items) => items
            .iter()
            .flat_map(|item| collect_table_binding_value_types(tcx, item, binding_name))
            .collect(),
        hir::ExprKind::Ret(Some(inner)) => {
            collect_table_binding_value_types(tcx, inner, binding_name)
        }
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            collect_table_binding_value_types(tcx, body.value, binding_name)
        }
        _ => Vec::new(),
    }
}

fn find_binding_init_in_expr<'tcx>(
    expr: &'tcx hir::Expr<'tcx>,
    target: hir::HirId,
) -> Option<&'tcx hir::Expr<'tcx>> {
    match &expr.kind {
        hir::ExprKind::Block(block, _) => {
            for stmt in block.stmts {
                if let Some(init) = find_binding_init_in_stmt(stmt, target) {
                    return Some(init);
                }
            }
            block
                .expr
                .and_then(|tail| find_binding_init_in_expr(tail, target))
        }
        hir::ExprKind::Closure(_) => None,
        _ => {
            find_in_recursive_expr_children(expr, |child| find_binding_init_in_expr(child, target))
        }
    }
}

fn find_binding_init_in_stmt<'tcx>(
    stmt: &'tcx hir::Stmt<'tcx>,
    target: hir::HirId,
) -> Option<&'tcx hir::Expr<'tcx>> {
    match &stmt.kind {
        hir::StmtKind::Let(hir::LetStmt {
            pat,
            init: Some(init),
            ..
        }) => {
            if pat_contains_hir_id(pat, target) {
                Some(init)
            } else {
                find_binding_init_in_expr(init, target)
            }
        }
        hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
            find_binding_init_in_expr(expr, target)
        }
        _ => None,
    }
}

fn pat_contains_hir_id(pat: &hir::Pat<'_>, target: hir::HirId) -> bool {
    match &pat.kind {
        hir::PatKind::Binding(_, hir_id, _, _) => *hir_id == target,
        hir::PatKind::Tuple(pats, _)
        | hir::PatKind::Or(pats)
        | hir::PatKind::TupleStruct(_, pats, _) => {
            pats.iter().any(|pat| pat_contains_hir_id(pat, target))
        }
        hir::PatKind::Slice(front, middle, back) => front
            .iter()
            .chain(middle.iter().copied())
            .chain(back.iter())
            .any(|pat| pat_contains_hir_id(pat, target)),
        hir::PatKind::Struct(_, fields, _) => fields
            .iter()
            .any(|field| pat_contains_hir_id(field.pat, target)),
        hir::PatKind::Box(inner)
        | hir::PatKind::Deref(inner)
        | hir::PatKind::Guard(inner, _)
        | hir::PatKind::Ref(inner, _, _) => pat_contains_hir_id(inner, target),
        _ => false,
    }
}

fn infer_local_fn_body_return<'tcx>(tcx: TyCtxt<'tcx>, def_id: LocalDefId) -> Option<LuaType> {
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

fn infer_callable_return_type<'tcx>(
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

fn infer_mapped_method_result<'tcx>(
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

fn infer_field_closure_result<'tcx>(
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
                for arg in *args {
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

fn explicit_lua_type_from_body_param(
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

fn infer_local_callable_param_type<'tcx>(
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
        && let Some(func_name) = extract_cross_module_call_from_snippet(&closure_snippet)
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
fn extract_names_from_pat(pat: &hir::Pat<'_>) -> Vec<String> {
    match &pat.kind {
        hir::PatKind::Tuple(pats, _) => pats.iter().map(|p| pat_to_name(p)).collect(),
        hir::PatKind::Binding(_, _, ident, _) => {
            vec![ident.name.as_str().to_string()]
        }
        _ => Vec::new(),
    }
}

fn refine_params_from_body<'tcx>(
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

fn refine_params_from_explicit_types<'tcx>(
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

fn explicit_lua_params_from_hir<'tcx>(
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
    let snippet = tcx.sess.source_map().span_to_snippet(param.ty_span).ok()?;
    let types = explicit_param_types_from_type_snippet(&snippet, expected_len)?;

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

fn explicit_param_types_from_body_param(
    tcx: TyCtxt<'_>,
    body: &hir::Body<'_>,
    params_idx: usize,
    expected_len: usize,
) -> Option<Vec<LuaType>> {
    let param = body.params.get(params_idx)?;
    if param.ty_span == param.pat.span {
        return None;
    }

    let snippet = tcx.sess.source_map().span_to_snippet(param.ty_span).ok()?;
    explicit_param_types_from_type_snippet(&snippet, expected_len)
}

fn explicit_param_types_from_closure_snippet(
    closure_snippet: &str,
    params_idx: usize,
    expected_len: usize,
) -> Option<Vec<LuaType>> {
    let params = closure_params_snippet(closure_snippet)?;
    let param = split_top_level(params, ',').get(params_idx)?.trim();
    let ty = top_level_param_type_annotation(param)?;
    explicit_param_types_from_type_snippet(ty, expected_len)
}

fn explicit_param_types_from_type_snippet(
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

fn closure_params_snippet(snippet: &str) -> Option<&str> {
    let start = snippet.find('|')?;
    let rest = &snippet[start + 1..];
    let end = rest.find('|')?;
    Some(rest[..end].trim())
}

fn top_level_param_type_annotation(param: &str) -> Option<&str> {
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

fn normalize_explicit_param_type(mut ty: LuaType) -> LuaType {
    normalize_userdata_ref_alias_type(&mut ty);
    ty
}

fn should_prefer_explicit_param_type(existing: &LuaType, explicit: &LuaType) -> bool {
    if existing == explicit {
        return false;
    }

    match existing {
        LuaType::Any | LuaType::Table | LuaType::Nil => is_informative(explicit),
        LuaType::String => matches!(
            explicit,
            LuaType::Class(_)
                | LuaType::Optional(_)
                | LuaType::Union(_)
                | LuaType::Array(_)
                | LuaType::Map(_, _)
        ),
        _ if is_generic_userdata_ref_type(existing) => !is_generic_userdata_ref_type(explicit),
        _ => is_better_inferred_type(existing, explicit) || !is_informative(existing),
    }
}

fn normalize_param_lua_type(ty: LuaType) -> LuaType {
    match ty {
        LuaType::Array(inner) => LuaType::Array(Box::new(normalize_param_lua_type(*inner))),
        LuaType::Optional(inner) => LuaType::Optional(Box::new(normalize_param_lua_type(*inner))),
        LuaType::Map(key, value) => LuaType::Map(
            Box::new(normalize_param_lua_type(*key)),
            Box::new(normalize_param_lua_type(*value)),
        ),
        LuaType::Union(items) => {
            make_union(items.into_iter().map(normalize_param_lua_type).collect())
        }
        LuaType::Variadic(inner) => LuaType::Variadic(Box::new(normalize_param_lua_type(*inner))),
        LuaType::FunctionSig { params, returns } => LuaType::FunctionSig {
            params: params.into_iter().map(normalize_param_lua_type).collect(),
            returns: returns.into_iter().map(normalize_param_lua_type).collect(),
        },
        other => other,
    }
}

fn infer_param_type_from_body<'tcx>(
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
                for arm in *arms {
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

                for arm in *arms {
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
            for arm in *arms {
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

fn string_bytes_binding_name(expr: &hir::Expr<'_>) -> Option<String> {
    let expr = peel_expr(expr);
    let hir::ExprKind::MethodCall(segment, receiver, _, _) = &expr.kind else {
        return None;
    };
    if segment.ident.name.as_str() != "as_bytes" {
        return None;
    }
    path_tail_name(receiver)
}

fn tuple_scrutinee_param_positions(expr: &hir::Expr<'_>, param_name: &str) -> Vec<usize> {
    let expr = peel_expr(expr);
    let hir::ExprKind::Tup(items) = &expr.kind else {
        return Vec::new();
    };

    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let name = path_tail_name(item).or_else(|| string_bytes_binding_name(item));
            (name.as_deref() == Some(param_name)).then_some(index)
        })
        .collect()
}

fn tuple_arm_param_type(
    tcx: TyCtxt<'_>,
    pat: &hir::Pat<'_>,
    positions: &[usize],
) -> Option<LuaType> {
    let hir::PatKind::Tuple(pats, _) = &pat.kind else {
        return None;
    };

    let mut tys = Vec::new();
    for position in positions {
        let pat = pats.get(*position)?;
        if let Some(literal) = string_literal_from_pat(tcx, pat) {
            tys.push(LuaType::StringLiteral(vec![literal]));
        }
    }

    if tys.is_empty() {
        None
    } else {
        Some(make_union(tys))
    }
}

fn string_literal_from_pat(tcx: TyCtxt<'_>, pat: &hir::Pat<'_>) -> Option<String> {
    let snippet = tcx.sess.source_map().span_to_snippet(pat.span).ok()?;
    let snippet = snippet.trim();

    if let Some(value) = snippet
        .strip_prefix("b\"")
        .and_then(|s| s.strip_suffix('"'))
    {
        Some(value.to_string())
    } else {
        snippet
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .map(|s| s.to_string())
    }
}

fn lua_type_from_value_arm<'tcx>(tcx: TyCtxt<'tcx>, arm: &'tcx hir::Arm<'tcx>) -> Option<LuaType> {
    match &arm.pat.kind {
        hir::PatKind::Or(pats) => {
            let tys: Option<Vec<_>> = pats
                .iter()
                .map(|pat| lua_type_from_value_pat_with_body(tcx, pat, arm.body))
                .collect();
            tys.map(make_union)
        }
        _ => lua_type_from_value_pat_with_body(tcx, arm.pat, arm.body),
    }
}

fn lua_type_from_value_pat_with_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    pat: &'tcx hir::Pat<'tcx>,
    body: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    match &pat.kind {
        hir::PatKind::TupleStruct(qpath, _, _)
        | hir::PatKind::Struct(qpath, _, _)
        | hir::PatKind::Expr(hir::PatExpr {
            kind: hir::PatExprKind::Path(qpath),
            ..
        }) => match qpath_last_name(qpath)?.as_str() {
            "Nil" => Some(LuaType::Nil),
            "Boolean" => Some(LuaType::Boolean),
            "Integer" => Some(LuaType::Integer),
            "Number" => Some(LuaType::Number),
            "String" => Some(LuaType::String),
            "Table" => extract_first_binding_name(pat)
                .and_then(|binding| infer_sequence_values_item_type(tcx, body, &binding))
                .map(|ty| LuaType::Array(Box::new(ty)))
                .or(Some(LuaType::Table)),
            "UserData" => extract_first_binding_name(pat)
                .and_then(|binding| infer_userdata_binding_type_from_body(tcx, body, &binding))
                .or_else(|| {
                    extract_first_binding_name(pat).and_then(|binding| {
                        infer_any_userdata_param_type_from_body(tcx, body, &binding)
                    })
                }),
            "Function" => Some(LuaType::Function),
            "Thread" => Some(LuaType::Thread),
            _ => None,
        },
        _ => None,
    }
}

fn extract_first_binding_name(pat: &hir::Pat<'_>) -> Option<String> {
    match &pat.kind {
        hir::PatKind::Binding(_, _, ident, _) => Some(ident.name.as_str().to_string()),
        hir::PatKind::TupleStruct(_, pats, _) | hir::PatKind::Tuple(pats, _) => {
            pats.iter().find_map(|pat| extract_first_binding_name(pat))
        }
        _ => None,
    }
}

fn infer_sequence_values_item_type<'tcx>(
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
                let args = segment.args?;
                let hir::GenericArg::Type(ty) = args.args.first()? else {
                    return snippet_generic_type_name(
                        &expr_snippet(tcx, expr),
                        "sequence_values::<",
                    )
                    .as_deref()
                    .map(lua_type_from_extracted_name);
                };
                let hir::TyKind::Path(qpath) = &ty.kind else {
                    return snippet_generic_type_name(
                        &expr_snippet(tcx, expr),
                        "sequence_values::<",
                    )
                    .as_deref()
                    .map(lua_type_from_extracted_name);
                };

                return extract_type_name_from_qpath(qpath)
                    .or_else(|| {
                        snippet_generic_type_name(&expr_snippet(tcx, expr), "sequence_values::<")
                    })
                    .as_deref()
                    .map(lua_type_from_extracted_name);
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

fn infer_userdata_binding_type_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    binding_name: &str,
) -> Option<LuaType> {
    let types = collect_userdata_binding_types_from_body(tcx, expr, binding_name);
    (!types.is_empty()).then(|| make_union(types))
}

fn collect_userdata_binding_types_from_body<'tcx>(
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
                for arg in *args {
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
            for arg in *args {
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
            for arm in *arms {
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

fn expr_refers_to_binding(expr: &hir::Expr<'_>, binding_name: &str) -> bool {
    path_tail_name(expr).as_deref() == Some(binding_name)
}

/// Infer a callback's signature by finding `.call()` / `.call_async()` invocations
/// on the given param variable within the body expression.
/// Returns `LuaType::FunctionSig` if a call is found, or `None`.
fn infer_callback_signature_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            let method_name = segment.ident.name.as_str();
            if matches!(method_name, "call" | "call_async")
                && expr_refers_to_binding(receiver, param_name)
            {
                // Found callback.call(args) or callback.call_async(args)
                // Use typeck to get the return type of the call expression
                let typeck = tcx.typeck(expr.hir_id.owner.def_id);
                let call_ty = typeck.node_type(expr.hir_id);
                let returns = map_return_ty(tcx, call_ty);

                // Extract param types from the argument expression
                let params = if let Some(arg_expr) = args.first() {
                    infer_callback_params_from_arg(tcx, arg_expr)
                } else {
                    vec![]
                };

                return Some(LuaType::FunctionSig {
                    params,
                    returns: returns.into_iter().map(|r| r.ty).collect(),
                });
            }

            // Recurse into receiver and args
            infer_callback_signature_from_body(tcx, receiver, param_name).or_else(|| {
                args.iter()
                    .find_map(|arg| infer_callback_signature_from_body(tcx, arg, param_name))
            })
        }
        hir::ExprKind::Block(block, _) => {
            // Check statements then tail expression
            for stmt in block.stmts {
                if let hir::StmtKind::Expr(e) | hir::StmtKind::Semi(e) = stmt.kind
                    && let Some(sig) = infer_callback_signature_from_body(tcx, e, param_name)
                {
                    return Some(sig);
                }
                if let hir::StmtKind::Let(local) = stmt.kind
                    && let Some(init) = local.init
                    && let Some(sig) = infer_callback_signature_from_body(tcx, init, param_name)
                {
                    return Some(sig);
                }
            }
            block
                .expr
                .and_then(|e| infer_callback_signature_from_body(tcx, e, param_name))
        }
        hir::ExprKind::Match(scrutinee, arms, _) => {
            infer_callback_signature_from_body(tcx, scrutinee, param_name).or_else(|| {
                arms.iter()
                    .find_map(|arm| infer_callback_signature_from_body(tcx, arm.body, param_name))
            })
        }
        hir::ExprKind::Call(callee, args) => {
            infer_callback_signature_from_body(tcx, callee, param_name).or_else(|| {
                args.iter()
                    .find_map(|a| infer_callback_signature_from_body(tcx, a, param_name))
            })
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            infer_callback_signature_from_body(tcx, cond, param_name)
                .or_else(|| infer_callback_signature_from_body(tcx, then_expr, param_name))
                .or_else(|| {
                    else_expr.and_then(|e| infer_callback_signature_from_body(tcx, e, param_name))
                })
        }
        _ => None,
    }
}

/// Infer callback param types from the argument expression passed to `.call(args)`.
/// The argument can be a tuple `(a, b, c)` or a single value.
fn infer_callback_params_from_arg<'tcx>(
    tcx: TyCtxt<'tcx>,
    arg_expr: &'tcx hir::Expr<'tcx>,
) -> Vec<LuaType> {
    let typeck = tcx.typeck(arg_expr.hir_id.owner.def_id);
    let ty = typeck.node_type(arg_expr.hir_id);

    match ty.kind() {
        // Empty tuple = no params
        ty::TyKind::Tuple(fields) if fields.is_empty() => vec![],
        // Tuple = multiple params
        ty::TyKind::Tuple(fields) => fields.iter().map(|t| map_ty_to_lua(tcx, t)).collect(),
        // Single non-unit value = one param
        _ => {
            let lua_ty = map_ty_to_lua(tcx, ty);
            if lua_ty == LuaType::Nil {
                vec![]
            } else {
                vec![lua_ty]
            }
        }
    }
}

fn infer_take_binding_type(
    tcx: TyCtxt<'_>,
    expr: &hir::Expr<'_>,
    binding_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);
    let hir::ExprKind::MethodCall(segment, receiver, _, _) = &expr.kind else {
        return None;
    };

    if segment.ident.name.as_str() != "take" || !expr_refers_to_binding(receiver, binding_name) {
        return None;
    }

    method_generic_lua_type(tcx, expr, segment)
}

fn infer_from_value_binding_type(
    tcx: TyCtxt<'_>,
    expr: &hir::Expr<'_>,
    args: &[hir::Expr<'_>],
    binding_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);
    let hir::ExprKind::MethodCall(segment, _receiver, _, _) = &expr.kind else {
        return None;
    };

    if segment.ident.name.as_str() != "from_value"
        || !args
            .first()
            .is_some_and(|arg| expr_refers_to_binding(arg, binding_name))
    {
        return None;
    }

    let ty = tcx.typeck(expr.hir_id.owner.def_id).expr_ty(expr);
    let lua_ty = map_ty_to_lua(tcx, unwrap_result_ty(tcx, ty));
    is_informative(&lua_ty).then_some(lua_ty)
}

fn lua_type_from_hir_ty<'hir, A>(ty: &hir::Ty<'hir, A>) -> Option<LuaType> {
    match &ty.kind {
        hir::TyKind::Ref(_, inner) => lua_type_from_hir_ty(inner.ty),
        hir::TyKind::Path(qpath) => {
            let (name, args) = match qpath {
                hir::QPath::Resolved(_, path) => {
                    let seg = path.segments.last()?;
                    (seg.ident.name.as_str(), seg.args)
                }
                hir::QPath::TypeRelative(_, seg) => (seg.ident.name.as_str(), seg.args),
            };

            let first_type_arg = || {
                args.and_then(|args| match args.args.first() {
                    Some(hir::GenericArg::Type(ty)) => Some(*ty),
                    _ => None,
                })
            };

            match name {
                "Option" => Some(LuaType::Optional(Box::new(lua_type_from_hir_ty(
                    first_type_arg()?,
                )?))),
                "Vec" | "VecDeque" => Some(LuaType::Array(Box::new(lua_type_from_hir_ty(
                    first_type_arg()?,
                )?))),
                "UserDataRef" | "UserDataRefMut" => lua_type_from_hir_ty(first_type_arg()?),
                "String" | "str" => Some(LuaType::String),
                "Integer" => Some(LuaType::Integer),
                "Number" => Some(LuaType::Number),
                "Boolean" => Some(LuaType::Boolean),
                "bool" => Some(LuaType::Boolean),
                "f32" | "f64" => Some(LuaType::Number),
                "i8" | "i16" | "i32" | "i64" | "i128" | "isize" => Some(LuaType::Integer),
                "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => Some(LuaType::Integer),
                "Table" => Some(LuaType::Table),
                "Function" => Some(LuaType::Function),
                "Thread" => Some(LuaType::Thread),
                "Value" | "AnyUserData" => Some(LuaType::Any),
                _ => Some(LuaType::Class(extract_type_name_from_qpath(qpath)?)),
            }
        }
        _ => None,
    }
}

fn resolved_hir_ty_lua_type<'hir, A>(tcx: TyCtxt<'_>, ty: &hir::Ty<'hir, A>) -> Option<LuaType> {
    let hir::TyKind::Path(qpath) = &ty.kind else {
        return None;
    };

    let (res, def_id) = match qpath {
        hir::QPath::Resolved(_, path) => (path.res, path.res.opt_def_id()?),
        hir::QPath::TypeRelative(_, _) => return None,
    };

    // Only call type_of on definitions that actually have types (structs,
    // enums, type aliases, etc.). Modules, consts, and other non-type defs
    // will ICE if passed to type_of.
    let has_type = matches!(
        res,
        rustc_hir::def::Res::Def(
            rustc_hir::def::DefKind::Struct
                | rustc_hir::def::DefKind::Enum
                | rustc_hir::def::DefKind::Union
                | rustc_hir::def::DefKind::TyAlias
                | rustc_hir::def::DefKind::AssocTy
                | rustc_hir::def::DefKind::ForeignTy
                | rustc_hir::def::DefKind::OpaqueTy
                | rustc_hir::def::DefKind::TraitAlias,
            _
        ) | rustc_hir::def::Res::SelfTyAlias { .. }
            | rustc_hir::def::Res::SelfTyParam { .. }
    );

    let resolved = type_alias_def_lua_type(tcx, def_id).or_else(|| {
        if !has_type {
            return None;
        }
        let ty = map_ty_to_lua(tcx, tcx.type_of(def_id).skip_binder());
        is_informative(&ty).then_some(ty)
    })?;
    let fallback = lua_type_from_hir_ty(ty);

    match fallback.as_ref() {
        Some(existing) if *existing == resolved => None,
        Some(existing)
            if !is_informative(existing)
                || is_better_inferred_type(existing, &resolved)
                || matches!(existing, LuaType::Class(_)) =>
        {
            Some(resolved)
        }
        None => Some(resolved),
        _ => None,
    }
}

fn type_alias_def_lua_type(tcx: TyCtxt<'_>, def_id: rustc_hir::def_id::DefId) -> Option<LuaType> {
    let snippet = def_snippet(tcx, def_id)?;
    let eq = snippet.find('=')?;
    if !snippet[..eq].contains("type ") {
        return None;
    }

    let rhs = snippet[eq + 1..].split(';').next()?.trim();
    (!rhs.is_empty()).then(|| lua_type_from_extracted_name(rhs))
}

fn lua_type_from_hir_ty_with_snippet<'hir, A>(
    tcx: TyCtxt<'_>,
    ty: &hir::Ty<'hir, A>,
) -> Option<LuaType> {
    resolved_hir_ty_lua_type(tcx, ty)
        .or_else(|| ty_snippet_to_lua_type(tcx, ty))
        .or_else(|| lua_type_from_hir_ty(ty))
}

fn ty_snippet_to_lua_type<'hir, A>(tcx: TyCtxt<'_>, ty: &hir::Ty<'hir, A>) -> Option<LuaType> {
    let mut snippet = tcx.sess.source_map().span_to_snippet(ty.span).ok()?;
    snippet = snippet.trim().to_string();

    while let Some(rest) = snippet.strip_prefix('&') {
        snippet = rest.trim_start().to_string();
    }
    if let Some(rest) = snippet.strip_prefix("mut ") {
        snippet = rest.trim_start().to_string();
    }

    (!snippet.is_empty()).then(|| lua_type_from_extracted_name(&snippet))
}

fn method_generic_lua_type(
    tcx: TyCtxt<'_>,
    expr: &hir::Expr<'_>,
    segment: &hir::PathSegment<'_>,
) -> Option<LuaType> {
    if let Some(args) = segment.args
        && let Some(hir::GenericArg::Type(ty)) = args.args.first()
        && let Some(lua_ty) = lua_type_from_hir_ty_with_snippet(tcx, ty)
    {
        return Some(lua_ty);
    }

    let marker = format!("{}::<", segment.ident.name.as_str());
    snippet_generic_type_name(&expr_snippet(tcx, expr), &marker)
        .as_deref()
        .map(lua_type_from_extracted_name)
}

fn callee_try_from_type_name(tcx: TyCtxt<'_>, callee: &hir::Expr<'_>) -> Option<String> {
    let callee = peel_expr(callee);
    let name = match &callee.kind {
        hir::ExprKind::Path(qpath) => match qpath {
            hir::QPath::TypeRelative(ty, seg) if seg.ident.name.as_str() == "try_from" => {
                let hir::TyKind::Path(qpath) = &ty.kind else {
                    return None;
                };
                extract_type_name_from_qpath(qpath)
            }
            hir::QPath::Resolved(_, path) => {
                let seg = path.segments.last()?;
                if seg.ident.name.as_str() != "try_from" {
                    return None;
                }
                let prev = path.segments.iter().rev().nth(1)?;
                Some(prev.ident.name.as_str().to_string())
            }
            _ => None,
        },
        _ => None,
    };

    name.or_else(|| snippet_type_before_method(&expr_snippet(tcx, callee), "try_from"))
}

fn qpath_last_name(qpath: &hir::QPath<'_>) -> Option<String> {
    match qpath {
        hir::QPath::Resolved(_, path) => path
            .segments
            .last()
            .map(|seg| seg.ident.name.as_str().to_string()),
        hir::QPath::TypeRelative(_, seg) => Some(seg.ident.name.as_str().to_string()),
    }
}

fn infer_any_userdata_param_type_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Match(scrutinee, arms, _)
            if is_type_id_call_on_binding(scrutinee, param_name) =>
        {
            let mut tys: Vec<_> = arms
                .iter()
                .filter_map(|arm| extract_type_id_guard_lua_type(tcx, arm.guard))
                .collect();
            if tys.is_empty() {
                tys = extract_all_generic_type_names(&expr_snippet(tcx, expr), "TypeId::of::<")
                    .into_iter()
                    .map(LuaType::Class)
                    .collect();
            }
            (!tys.is_empty()).then(|| make_union(tys))
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .find_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local.init.and_then(|init| {
                    infer_any_userdata_param_type_from_body(tcx, init, param_name)
                }),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    infer_any_userdata_param_type_from_body(tcx, expr, param_name)
                }
                _ => None,
            })
            .or_else(|| {
                block
                    .expr
                    .and_then(|expr| infer_any_userdata_param_type_from_body(tcx, expr, param_name))
            }),
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            infer_any_userdata_param_type_from_body(tcx, cond, param_name)
                .or_else(|| infer_any_userdata_param_type_from_body(tcx, then_expr, param_name))
                .or_else(|| {
                    else_expr.and_then(|expr| {
                        infer_any_userdata_param_type_from_body(tcx, expr, param_name)
                    })
                })
        }
        hir::ExprKind::Call(callee, args) => {
            infer_any_userdata_param_type_from_body(tcx, callee, param_name).or_else(|| {
                args.iter()
                    .find_map(|arg| infer_any_userdata_param_type_from_body(tcx, arg, param_name))
            })
        }
        hir::ExprKind::Binary(_, lhs, rhs) => {
            infer_any_userdata_param_type_from_body(tcx, lhs, param_name)
                .or_else(|| infer_any_userdata_param_type_from_body(tcx, rhs, param_name))
        }
        hir::ExprKind::Let(let_expr) => {
            infer_any_userdata_param_type_from_body(tcx, let_expr.init, param_name)
        }
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            infer_any_userdata_param_type_from_body(tcx, receiver, param_name).or_else(|| {
                args.iter()
                    .find_map(|arg| infer_any_userdata_param_type_from_body(tcx, arg, param_name))
            })
        }
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            infer_any_userdata_param_type_from_body(tcx, body.value, param_name)
        }
        hir::ExprKind::Ret(Some(inner)) => {
            infer_any_userdata_param_type_from_body(tcx, inner, param_name)
        }
        _ => None,
    }
}

/// Infer a param's type from `from_lua::<T>(param)` or `let x: T = from_lua(param)?` patterns.
/// When a `Value` param is immediately converted via `from_lua`, the target type reveals the
/// actual expected Lua type.
fn infer_from_lua_conversion_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Block(block, _) => {
            for stmt in block.stmts {
                // Look for: let var: T = from_lua(param)?;
                if let hir::StmtKind::Let(local) = &stmt.kind
                    && let Some(init) = local.init
                {
                    let init = peel_try_expr(init);
                    if is_from_lua_call_on_param(init, param_name) {
                        // Get the type of the let binding from typeck
                        let typeck = tcx.typeck(local.pat.hir_id.owner.def_id);
                        let ty = typeck.node_type(local.pat.hir_id);
                        let lua_ty = map_ty_to_lua(tcx, ty);
                        if is_informative(&lua_ty) {
                            return Some(lua_ty);
                        }
                    }
                }
                // Recurse into expressions
                if let hir::StmtKind::Expr(e) | hir::StmtKind::Semi(e) = &stmt.kind
                    && let Some(ty) = infer_from_lua_conversion_type(tcx, e, param_name)
                {
                    return Some(ty);
                }
            }
            block
                .expr
                .and_then(|e| infer_from_lua_conversion_type(tcx, e, param_name))
        }
        _ => None,
    }
}

/// Check if an expression is a `from_lua(param)` or `from_dynamic(param)` call.
fn is_from_lua_call_on_param(expr: &hir::Expr<'_>, param_name: &str) -> bool {
    match &expr.kind {
        hir::ExprKind::Call(callee, args) => {
            let name = called_path_name(callee);
            if name.as_deref().is_some_and(|n| {
                n == "from_lua" || n == "from_dynamic" || n == "from_lua_value_dynamic"
            }) && args
                .iter()
                .any(|arg| expr_refers_to_binding(arg, param_name))
            {
                return true;
            }
            false
        }
        _ => false,
    }
}

fn infer_json_like_param_type_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Call(callee, args) => {
            if let Some(name) = called_path_name(callee)
                && matches!(
                    name.as_str(),
                    "to_string" | "to_string_pretty" | "to_vec" | "to_vec_pretty"
                )
                && args
                    .first()
                    .is_some_and(|arg| expr_refers_to_binding(arg, param_name))
                && path_contains_segment(callee, "serde_json")
            {
                return Some(json_like_lua_type());
            }

            infer_json_like_param_type_from_body(tcx, callee, param_name).or_else(|| {
                args.iter()
                    .find_map(|arg| infer_json_like_param_type_from_body(tcx, arg, param_name))
            })
        }
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            infer_json_like_param_type_from_body(tcx, receiver, param_name).or_else(|| {
                args.iter()
                    .find_map(|arg| infer_json_like_param_type_from_body(tcx, arg, param_name))
            })
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .find_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local
                    .init
                    .and_then(|init| infer_json_like_param_type_from_body(tcx, init, param_name)),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    infer_json_like_param_type_from_body(tcx, expr, param_name)
                }
                _ => None,
            })
            .or_else(|| {
                block
                    .expr
                    .and_then(|expr| infer_json_like_param_type_from_body(tcx, expr, param_name))
            }),
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            infer_json_like_param_type_from_body(tcx, cond, param_name)
                .or_else(|| infer_json_like_param_type_from_body(tcx, then_expr, param_name))
                .or_else(|| {
                    else_expr.and_then(|expr| {
                        infer_json_like_param_type_from_body(tcx, expr, param_name)
                    })
                })
        }
        hir::ExprKind::Match(scrutinee, arms, _) => {
            infer_json_like_param_type_from_body(tcx, scrutinee, param_name).or_else(|| {
                arms.iter()
                    .find_map(|arm| infer_json_like_param_type_from_body(tcx, arm.body, param_name))
            })
        }
        hir::ExprKind::Assign(lhs, rhs, _) | hir::ExprKind::AssignOp(_, lhs, rhs) => {
            infer_json_like_param_type_from_body(tcx, lhs, param_name)
                .or_else(|| infer_json_like_param_type_from_body(tcx, rhs, param_name))
        }
        hir::ExprKind::Binary(_, lhs, rhs) => {
            infer_json_like_param_type_from_body(tcx, lhs, param_name)
                .or_else(|| infer_json_like_param_type_from_body(tcx, rhs, param_name))
        }
        hir::ExprKind::Let(let_expr) => {
            infer_json_like_param_type_from_body(tcx, let_expr.init, param_name)
        }
        hir::ExprKind::Struct(_, fields, tail) => fields
            .iter()
            .find_map(|field| infer_json_like_param_type_from_body(tcx, field.expr, param_name))
            .or_else(|| match tail {
                hir::StructTailExpr::Base(base) => {
                    infer_json_like_param_type_from_body(tcx, base, param_name)
                }
                _ => None,
            }),
        hir::ExprKind::Array(items) | hir::ExprKind::Tup(items) => items
            .iter()
            .find_map(|item| infer_json_like_param_type_from_body(tcx, item, param_name)),
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            infer_json_like_param_type_from_body(tcx, body.value, param_name)
        }
        hir::ExprKind::Ret(Some(inner)) => {
            infer_json_like_param_type_from_body(tcx, inner, param_name)
        }
        _ => None,
    }
}

fn infer_table_param_type_from_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Option<LuaType> {
    let mut value_types = collect_table_param_value_types(tcx, expr, param_name);
    value_types.extend(collect_nested_table_param_value_types(
        tcx, expr, param_name,
    ));
    value_types = normalize_table_value_types(value_types);
    if value_types.is_empty() {
        None
    } else {
        Some(normalize_inferred_lua_type(LuaType::Map(
            Box::new(LuaType::String),
            Box::new(make_union(value_types)),
        )))
    }
}

fn infer_table_param_type_from_converter_calls<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Call(callee, args) => {
            infer_table_converter_call_type(tcx, callee, args, param_name)
                .or_else(|| infer_table_param_type_from_converter_calls(tcx, callee, param_name))
                .or_else(|| {
                    args.iter().find_map(|arg| {
                        infer_table_param_type_from_converter_calls(tcx, arg, param_name)
                    })
                })
        }
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            infer_table_param_type_from_converter_calls(tcx, receiver, param_name).or_else(|| {
                args.iter().find_map(|arg| {
                    infer_table_param_type_from_converter_calls(tcx, arg, param_name)
                })
            })
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .find_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local.init.and_then(|init| {
                    infer_table_param_type_from_converter_calls(tcx, init, param_name)
                }),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    infer_table_param_type_from_converter_calls(tcx, expr, param_name)
                }
                _ => None,
            })
            .or_else(|| {
                block.expr.and_then(|expr| {
                    infer_table_param_type_from_converter_calls(tcx, expr, param_name)
                })
            }),
        hir::ExprKind::Match(scrutinee, arms, _) => {
            infer_table_param_type_from_converter_calls(tcx, scrutinee, param_name).or_else(|| {
                arms.iter().find_map(|arm| {
                    infer_table_param_type_from_converter_calls(tcx, arm.body, param_name)
                })
            })
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            infer_table_param_type_from_converter_calls(tcx, cond, param_name)
                .or_else(|| infer_table_param_type_from_converter_calls(tcx, then_expr, param_name))
                .or_else(|| {
                    else_expr.and_then(|expr| {
                        infer_table_param_type_from_converter_calls(tcx, expr, param_name)
                    })
                })
        }
        hir::ExprKind::Assign(lhs, rhs, _)
        | hir::ExprKind::AssignOp(_, lhs, rhs)
        | hir::ExprKind::Binary(_, lhs, rhs) => {
            infer_table_param_type_from_converter_calls(tcx, lhs, param_name)
                .or_else(|| infer_table_param_type_from_converter_calls(tcx, rhs, param_name))
        }
        hir::ExprKind::Let(let_expr) => {
            infer_table_param_type_from_converter_calls(tcx, let_expr.init, param_name)
        }
        hir::ExprKind::Array(items) | hir::ExprKind::Tup(items) => items
            .iter()
            .find_map(|item| infer_table_param_type_from_converter_calls(tcx, item, param_name)),
        hir::ExprKind::Struct(_, fields, tail) => fields
            .iter()
            .find_map(|field| {
                infer_table_param_type_from_converter_calls(tcx, field.expr, param_name)
            })
            .or_else(|| match tail {
                hir::StructTailExpr::Base(base) => {
                    infer_table_param_type_from_converter_calls(tcx, base, param_name)
                }
                _ => None,
            }),
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            infer_table_param_type_from_converter_calls(tcx, body.value, param_name)
        }
        hir::ExprKind::Ret(Some(inner)) => {
            infer_table_param_type_from_converter_calls(tcx, inner, param_name)
        }
        _ => None,
    }
}

fn infer_table_converter_call_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    callee: &'tcx hir::Expr<'tcx>,
    args: &'tcx [hir::Expr<'tcx>],
    param_name: &str,
) -> Option<LuaType> {
    let _arg_index = args
        .iter()
        .position(|arg| expr_refers_to_binding(arg, param_name))?;
    let callee_name = called_path_name(callee);
    let snippet = expr_def_id(tcx, callee).and_then(|def_id| def_snippet(tcx, def_id));

    if callee_name.as_deref() == Some("table_to_args") {
        let key_ty = make_union(vec![LuaType::Integer, LuaType::String]);
        let value_ty = match snippet
            .as_deref()
            .map(infer_value_converter_types_from_snippet)
            .filter(|tys| !tys.is_empty())
        {
            Some(tys) => make_union(normalize_table_value_types(tys)),
            None => json_like_lua_type(),
        };

        return Some(LuaType::Map(Box::new(key_ty), Box::new(value_ty)));
    }

    let snippet = snippet?;

    if !snippet.contains("pairs::<Value, Value>()")
        && !snippet.contains("pairs::<mlua::Value, mlua::Value>()")
    {
        return None;
    }

    let mut key_types = Vec::new();
    if snippet.contains("Value::Integer") || snippet.contains("DataKey::Integer") {
        key_types.push(LuaType::Integer);
    }
    if snippet.contains("Value::String") || snippet.contains("DataKey::String") {
        key_types.push(LuaType::String);
    }
    if key_types.is_empty() {
        key_types.push(LuaType::String);
    }

    let mut value_types = infer_value_converter_types_from_snippet(&snippet);
    value_types = normalize_table_value_types(value_types);
    if value_types.is_empty() {
        value_types.push(LuaType::Any);
    }

    Some(normalize_inferred_lua_type(LuaType::Map(
        Box::new(make_union(key_types)),
        Box::new(make_union(value_types)),
    )))
}

fn normalize_table_value_types(mut value_types: Vec<LuaType>) -> Vec<LuaType> {
    let has_generic_userdata_ref = value_types.iter().any(is_generic_userdata_ref_type);
    if has_generic_userdata_ref {
        for ty in &mut value_types {
            normalize_userdata_ref_alias_type(ty);
        }
    }

    let has_specific_userdata = value_types
        .iter()
        .any(|ty| !is_generic_userdata_ref_type(ty) && is_informative(ty));
    if has_specific_userdata {
        value_types.retain(|ty| !is_generic_userdata_ref_type(ty));
    }

    let has_specific = value_types.iter().any(|ty| !matches!(ty, LuaType::Table));

    if has_specific {
        value_types.retain(|ty| !matches!(ty, LuaType::Table));
    }

    value_types
}

fn normalize_inferred_lua_type(ty: LuaType) -> LuaType {
    match ty {
        LuaType::Array(inner) => LuaType::Array(Box::new(normalize_inferred_lua_type(*inner))),
        LuaType::Optional(inner) => {
            LuaType::Optional(Box::new(normalize_inferred_lua_type(*inner)))
        }
        LuaType::Map(key, value) => LuaType::Map(
            Box::new(normalize_inferred_lua_type(*key)),
            Box::new(normalize_inferred_lua_type(*value)),
        ),
        LuaType::Union(items) => {
            let mut items: Vec<_> = items.into_iter().map(normalize_inferred_lua_type).collect();

            let has_generic_userdata_ref = items.iter().any(is_generic_userdata_ref_type);
            if has_generic_userdata_ref {
                for item in &mut items {
                    normalize_userdata_ref_alias_type(item);
                }

                if items
                    .iter()
                    .any(|item| !is_generic_userdata_ref_type(item) && is_informative(item))
                {
                    items.retain(|item| !is_generic_userdata_ref_type(item));
                }
            }

            make_union(items)
        }
        LuaType::Variadic(inner) => {
            LuaType::Variadic(Box::new(normalize_inferred_lua_type(*inner)))
        }
        LuaType::FunctionSig { params, returns } => LuaType::FunctionSig {
            params: params
                .into_iter()
                .map(normalize_inferred_lua_type)
                .collect(),
            returns: returns
                .into_iter()
                .map(normalize_inferred_lua_type)
                .collect(),
        },
        other => other,
    }
}

fn is_generic_userdata_ref_type(ty: &LuaType) -> bool {
    matches!(ty, LuaType::Class(name) if matches!(name.as_str(), "UserDataRef" | "UserDataRefMut"))
}

fn is_any_userdata_type_snippet(snippet: &str) -> bool {
    let snippet = snippet.trim();
    matches!(
        snippet,
        "AnyUserData"
            | "mlua::AnyUserData"
            | "mlua::userdata::AnyUserData"
            | "mlua::prelude::LuaAnyUserData"
    )
}

fn normalize_userdata_ref_alias_type(ty: &mut LuaType) {
    match ty {
        LuaType::Class(name) => {
            if matches!(name.as_str(), "UserDataRef" | "UserDataRefMut") {
                return;
            }
            for suffix in ["RefMut", "Ref"] {
                if let Some(base) = name.strip_suffix(suffix)
                    && !base.is_empty()
                    && base
                        .chars()
                        .next()
                        .is_some_and(|ch| ch.is_ascii_uppercase())
                {
                    *name = base.to_string();
                    break;
                }
            }
        }
        LuaType::Array(inner) | LuaType::Optional(inner) | LuaType::Variadic(inner) => {
            normalize_userdata_ref_alias_type(inner);
        }
        LuaType::Map(key, value) => {
            normalize_userdata_ref_alias_type(key);
            normalize_userdata_ref_alias_type(value);
        }
        LuaType::Union(items) => {
            for item in items {
                normalize_userdata_ref_alias_type(item);
            }
        }
        LuaType::FunctionSig { params, returns } => {
            for param in params {
                normalize_userdata_ref_alias_type(param);
            }
            for ret in returns {
                normalize_userdata_ref_alias_type(ret);
            }
        }
        _ => {}
    }
}

fn collect_nested_table_param_value_types<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Vec<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Block(block, _) => {
            let mut tys = Vec::new();

            for stmt in block.stmts {
                if let hir::StmtKind::Let(local) = &stmt.kind
                    && let Some(init) = local.init
                    && let Some(_key) = raw_get_table_binding_key(init, param_name)
                    && let Some(ty) = infer_table_binding_usage_type(
                        tcx,
                        block.expr.unwrap_or(expr),
                        &pat_to_name(local.pat),
                    )
                {
                    tys.push(ty);
                }
            }

            for stmt in block.stmts {
                match &stmt.kind {
                    hir::StmtKind::Let(local) => {
                        if let Some(init) = local.init {
                            tys.extend(collect_nested_table_param_value_types(
                                tcx, init, param_name,
                            ));
                        }
                    }
                    hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                        tys.extend(collect_nested_table_param_value_types(
                            tcx, expr, param_name,
                        ));
                    }
                    _ => {}
                }
            }
            if let Some(tail) = block.expr {
                tys.extend(collect_nested_table_param_value_types(
                    tcx, tail, param_name,
                ));
            }
            tys
        }
        hir::ExprKind::Match(scrutinee, arms, _) => {
            let mut tys = collect_nested_table_param_value_types(tcx, scrutinee, param_name);
            for arm in *arms {
                tys.extend(collect_nested_table_param_value_types(
                    tcx, arm.body, param_name,
                ));
            }
            tys
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            let mut tys = collect_nested_table_param_value_types(tcx, cond, param_name);
            tys.extend(collect_nested_table_param_value_types(
                tcx, then_expr, param_name,
            ));
            if let Some(else_expr) = else_expr {
                tys.extend(collect_nested_table_param_value_types(
                    tcx, else_expr, param_name,
                ));
            }
            tys
        }
        hir::ExprKind::Call(callee, args) => {
            let mut tys = collect_nested_table_param_value_types(tcx, callee, param_name);
            for arg in *args {
                tys.extend(collect_nested_table_param_value_types(tcx, arg, param_name));
            }
            tys
        }
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            let mut tys = collect_nested_table_param_value_types(tcx, receiver, param_name);
            for arg in *args {
                tys.extend(collect_nested_table_param_value_types(tcx, arg, param_name));
            }
            tys
        }
        hir::ExprKind::Assign(lhs, rhs, _) | hir::ExprKind::AssignOp(_, lhs, rhs) => {
            let mut tys = collect_nested_table_param_value_types(tcx, lhs, param_name);
            tys.extend(collect_nested_table_param_value_types(tcx, rhs, param_name));
            tys
        }
        hir::ExprKind::Binary(_, lhs, rhs) => {
            let mut tys = collect_nested_table_param_value_types(tcx, lhs, param_name);
            tys.extend(collect_nested_table_param_value_types(tcx, rhs, param_name));
            tys
        }
        hir::ExprKind::Let(let_expr) => {
            collect_nested_table_param_value_types(tcx, let_expr.init, param_name)
        }
        hir::ExprKind::Array(items) | hir::ExprKind::Tup(items) => items
            .iter()
            .flat_map(|item| collect_nested_table_param_value_types(tcx, item, param_name))
            .collect(),
        hir::ExprKind::Struct(_, fields, tail) => fields
            .iter()
            .flat_map(|field| collect_nested_table_param_value_types(tcx, field.expr, param_name))
            .chain(match tail {
                hir::StructTailExpr::Base(base) => {
                    collect_nested_table_param_value_types(tcx, base, param_name)
                }
                _ => Vec::new(),
            })
            .collect(),
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            collect_nested_table_param_value_types(tcx, body.value, param_name)
        }
        hir::ExprKind::Ret(Some(inner)) => {
            collect_nested_table_param_value_types(tcx, inner, param_name)
        }
        _ => Vec::new(),
    }
}

fn raw_get_table_binding_key<'tcx>(
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Option<String> {
    let expr = peel_try_expr(expr);
    let hir::ExprKind::MethodCall(segment, receiver, args, _) = &expr.kind else {
        return None;
    };
    if !matches!(segment.ident.name.as_str(), "get" | "raw_get")
        || !expr_refers_to_binding(receiver, param_name)
        || args.is_empty()
    {
        return None;
    }
    extract_string_literal(&args[0])
}

fn infer_table_binding_usage_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    binding_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, _args, _) => {
            if path_tail_name(receiver).as_deref() == Some(binding_name) {
                match segment.ident.name.as_str() {
                    "sequence_values" => {
                        if let Some(ty) = method_generic_lua_type(tcx, expr, segment) {
                            return Some(LuaType::Array(Box::new(ty)));
                        }
                    }
                    "pairs" => {
                        if let Some(args) = segment.args {
                            let generic_types: Vec<_> = args
                                .args
                                .iter()
                                .filter_map(|arg| match arg {
                                    hir::GenericArg::Type(ty) => lua_type_from_hir_ty(*ty),
                                    _ => None,
                                })
                                .collect();
                            if generic_types.len() == 2 {
                                return Some(LuaType::Map(
                                    Box::new(generic_types[0].clone()),
                                    Box::new(generic_types[1].clone()),
                                ));
                            }
                        }
                    }
                    _ => {}
                }
            }

            infer_table_binding_usage_type(tcx, receiver, binding_name)
        }
        hir::ExprKind::Call(callee, args) => {
            infer_table_binding_usage_type(tcx, callee, binding_name).or_else(|| {
                args.iter()
                    .find_map(|arg| infer_table_binding_usage_type(tcx, arg, binding_name))
            })
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .find_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local
                    .init
                    .and_then(|init| infer_table_binding_usage_type(tcx, init, binding_name)),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    infer_table_binding_usage_type(tcx, expr, binding_name)
                }
                _ => None,
            })
            .or_else(|| {
                block
                    .expr
                    .and_then(|expr| infer_table_binding_usage_type(tcx, expr, binding_name))
            }),
        hir::ExprKind::Match(scrutinee, arms, _) => {
            infer_table_binding_usage_type(tcx, scrutinee, binding_name).or_else(|| {
                arms.iter()
                    .find_map(|arm| infer_table_binding_usage_type(tcx, arm.body, binding_name))
            })
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            infer_table_binding_usage_type(tcx, cond, binding_name)
                .or_else(|| infer_table_binding_usage_type(tcx, then_expr, binding_name))
                .or_else(|| {
                    else_expr
                        .and_then(|expr| infer_table_binding_usage_type(tcx, expr, binding_name))
                })
        }
        hir::ExprKind::Assign(lhs, rhs, _) | hir::ExprKind::AssignOp(_, lhs, rhs) => {
            infer_table_binding_usage_type(tcx, lhs, binding_name)
                .or_else(|| infer_table_binding_usage_type(tcx, rhs, binding_name))
        }
        hir::ExprKind::Binary(_, lhs, rhs) => {
            infer_table_binding_usage_type(tcx, lhs, binding_name)
                .or_else(|| infer_table_binding_usage_type(tcx, rhs, binding_name))
        }
        hir::ExprKind::Let(let_expr) => {
            infer_table_binding_usage_type(tcx, let_expr.init, binding_name)
        }
        hir::ExprKind::Array(items) | hir::ExprKind::Tup(items) => items
            .iter()
            .find_map(|item| infer_table_binding_usage_type(tcx, item, binding_name)),
        hir::ExprKind::Struct(_, fields, tail) => fields
            .iter()
            .find_map(|field| infer_table_binding_usage_type(tcx, field.expr, binding_name))
            .or_else(|| match tail {
                hir::StructTailExpr::Base(base) => {
                    infer_table_binding_usage_type(tcx, base, binding_name)
                }
                _ => None,
            }),
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            infer_table_binding_usage_type(tcx, body.value, binding_name)
        }
        hir::ExprKind::Ret(Some(inner)) => infer_table_binding_usage_type(tcx, inner, binding_name),
        _ => None,
    }
}

fn collect_table_param_value_types<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Vec<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            let mut tys = Vec::new();

            if matches!(segment.ident.name.as_str(), "get" | "raw_get")
                && expr_refers_to_binding(receiver, param_name)
                && !args.is_empty()
                && extract_string_literal(&args[0]).is_some()
            {
                let lua_ty = method_generic_lua_type(tcx, expr, segment).unwrap_or_else(|| {
                    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
                    let ty = unwrap_result_ty(tcx, typeck.expr_ty(expr));
                    map_ty_to_lua(tcx, ty)
                });
                if is_informative(&lua_ty) {
                    tys.push(lua_ty);
                }
            }

            if matches!(segment.ident.name.as_str(), "set" | "raw_set")
                && expr_refers_to_binding(receiver, param_name)
                && args.len() >= 2
                && extract_string_literal(&args[0]).is_some()
            {
                tys.push(infer_value_expr_lua_type(tcx, &args[1]));
            }

            tys.extend(collect_table_param_value_types(tcx, receiver, param_name));
            for arg in *args {
                tys.extend(collect_table_param_value_types(tcx, arg, param_name));
            }
            tys
        }
        hir::ExprKind::Call(callee, args) => {
            let mut tys = collect_table_param_value_types(tcx, callee, param_name);
            for arg in *args {
                tys.extend(collect_table_param_value_types(tcx, arg, param_name));
            }
            tys
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .flat_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => {
                    let mut tys = local
                        .init
                        .map(|init| collect_table_param_value_types(tcx, init, param_name))
                        .unwrap_or_default();
                    if let Some(ty) = infer_table_param_let_binding_type(tcx, local, param_name) {
                        tys.push(ty);
                    }
                    tys
                }
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    collect_table_param_value_types(tcx, expr, param_name)
                }
                _ => Vec::new(),
            })
            .chain(
                block
                    .expr
                    .into_iter()
                    .flat_map(|expr| collect_table_param_value_types(tcx, expr, param_name)),
            )
            .collect(),
        hir::ExprKind::Match(scrutinee, arms, _) => {
            let mut tys = collect_table_param_value_types(tcx, scrutinee, param_name);
            for arm in *arms {
                tys.extend(collect_table_param_value_types(tcx, arm.body, param_name));
            }
            tys
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            let mut tys = collect_table_param_value_types(tcx, cond, param_name);
            tys.extend(collect_table_param_value_types(tcx, then_expr, param_name));
            if let Some(else_expr) = else_expr {
                tys.extend(collect_table_param_value_types(tcx, else_expr, param_name));
            }
            tys
        }
        hir::ExprKind::Assign(lhs, rhs, _) | hir::ExprKind::AssignOp(_, lhs, rhs) => {
            let mut tys = collect_table_param_value_types(tcx, lhs, param_name);
            tys.extend(collect_table_param_value_types(tcx, rhs, param_name));
            tys
        }
        hir::ExprKind::Binary(_, lhs, rhs) => {
            let mut tys = collect_table_param_value_types(tcx, lhs, param_name);
            tys.extend(collect_table_param_value_types(tcx, rhs, param_name));
            tys
        }
        hir::ExprKind::Let(let_expr) => {
            collect_table_param_value_types(tcx, let_expr.init, param_name)
        }
        hir::ExprKind::Struct(_, fields, tail) => fields
            .iter()
            .flat_map(|field| collect_table_param_value_types(tcx, field.expr, param_name))
            .chain(match tail {
                hir::StructTailExpr::Base(base) => {
                    collect_table_param_value_types(tcx, base, param_name)
                }
                _ => Vec::new(),
            })
            .collect(),
        hir::ExprKind::Array(items) | hir::ExprKind::Tup(items) => items
            .iter()
            .flat_map(|item| collect_table_param_value_types(tcx, item, param_name))
            .collect(),
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            collect_table_param_value_types(tcx, body.value, param_name)
        }
        hir::ExprKind::Ret(Some(inner)) => collect_table_param_value_types(tcx, inner, param_name),
        _ => Vec::new(),
    }
}

fn infer_table_param_let_binding_type(
    tcx: TyCtxt<'_>,
    local: &hir::LetStmt<'_>,
    param_name: &str,
) -> Option<LuaType> {
    let init = peel_expr(unwrap_try_expr(local.init?));
    let hir::ExprKind::MethodCall(segment, receiver, args, _) = &init.kind else {
        return None;
    };

    if !matches!(segment.ident.name.as_str(), "get" | "raw_get")
        || !expr_refers_to_binding(receiver, param_name)
        || args.is_empty()
        || extract_string_literal(&args[0]).is_none()
    {
        return None;
    }

    local
        .ty
        .and_then(|ty| lua_type_from_hir_ty_with_snippet(tcx, ty))
}

fn json_like_lua_type() -> LuaType {
    make_union(vec![
        LuaType::Nil,
        LuaType::Boolean,
        LuaType::Number,
        LuaType::String,
        LuaType::Table,
    ])
}

fn called_path_name(expr: &hir::Expr<'_>) -> Option<String> {
    let expr = peel_expr(expr);
    let hir::ExprKind::Path(qpath) = &expr.kind else {
        return None;
    };
    qpath_last_name(qpath)
}

fn path_contains_segment(expr: &hir::Expr<'_>, needle: &str) -> bool {
    let expr = peel_expr(expr);
    let hir::ExprKind::Path(qpath) = &expr.kind else {
        return false;
    };

    match qpath {
        hir::QPath::Resolved(_, path) => path
            .segments
            .iter()
            .any(|segment| segment.ident.name.as_str() == needle),
        hir::QPath::TypeRelative(ty, seg) => {
            seg.ident.name.as_str() == needle
                || matches!(&ty.kind, hir::TyKind::Path(inner) if qpath_last_name(inner).as_deref() == Some(needle))
        }
    }
}

fn infer_param_type_from_local_call<'tcx>(
    tcx: TyCtxt<'tcx>,
    callee: &'tcx hir::Expr<'tcx>,
    args: &'tcx [hir::Expr<'tcx>],
    param_name: &str,
) -> Option<LuaType> {
    let arg_index = args
        .iter()
        .position(|arg| path_tail_name(arg).as_deref() == Some(param_name))?;
    let def_id = expr_def_id(tcx, callee)?.as_local()?;
    infer_local_callable_param_type(tcx, def_id, arg_index)
}

fn infer_param_type_from_local_method_call<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);
    let hir::ExprKind::MethodCall(_, receiver, args, _) = &expr.kind else {
        return None;
    };

    let arg_index = args
        .iter()
        .position(|arg| path_tail_name(arg).as_deref() == Some(param_name))?;

    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    let method_local = resolve_local_method_def_id(tcx, typeck, expr, receiver)?;

    infer_local_callable_param_type(tcx, method_local, arg_index + 1)
}

fn resolve_local_method_def_id<'tcx>(
    tcx: TyCtxt<'tcx>,
    typeck: &'tcx ty::TypeckResults<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    receiver: &'tcx hir::Expr<'tcx>,
) -> Option<LocalDefId> {
    let def_id = typeck.type_dependent_def_id(expr.hir_id)?;

    def_id.as_local().or_else(|| {
        let receiver_ty = typeck.expr_ty(receiver);
        let ty::TyKind::Adt(adt_def, _) = receiver_ty.kind() else {
            return None;
        };

        let method_name = match &expr.kind {
            hir::ExprKind::MethodCall(segment, _, _, _) => segment.ident.name,
            _ => return None,
        };

        tcx.inherent_impls(adt_def.did())
            .iter()
            .find_map(|impl_def_id| {
                tcx.associated_items(*impl_def_id)
                    .in_definition_order()
                    .find(|item| item.ident(tcx).name == method_name)
                    .and_then(|item| item.def_id.as_local())
            })
    })
}

fn infer_param_type_from_converter_call<'tcx>(
    tcx: TyCtxt<'tcx>,
    callee: &'tcx hir::Expr<'tcx>,
    args: &'tcx [hir::Expr<'tcx>],
    param_name: &str,
) -> Option<LuaType> {
    let arg_index = args
        .iter()
        .position(|arg| expr_refers_to_binding(arg, param_name))?;
    let def_id = expr_def_id(tcx, callee)?;
    let snippet = def_snippet(tcx, def_id)?;

    let mut tys = Vec::new();
    if arg_index > 0 || snippet.contains("Value") {
        tys.extend(infer_value_converter_types_from_snippet(&snippet));
    }
    if tys.is_empty() {
        None
    } else {
        Some(make_union(tys))
    }
}

fn infer_value_converter_types_from_snippet(snippet: &str) -> Vec<LuaType> {
    let mut tys = Vec::new();

    for (needle, ty) in [
        ("Value::Nil", LuaType::Nil),
        ("Value::Boolean", LuaType::Boolean),
        ("Value::Integer", LuaType::Integer),
        ("Value::Number", LuaType::Number),
        ("Value::String", LuaType::String),
        ("Value::Table", LuaType::Table),
        ("Value::Function", LuaType::Function),
        ("Value::Thread", LuaType::Thread),
    ] {
        if snippet.contains(needle) {
            tys.push(ty);
        }
    }

    let markers = [
        "TypeId::of::<",
        "take::<",
        "borrow::<",
        "borrow_mut::<",
        "try_borrow::<",
        "try_borrow_mut::<",
    ];
    for marker in markers {
        for name in extract_all_generic_type_names(snippet, marker) {
            tys.push(lua_type_from_extracted_name(&name));
        }
    }

    if tys.is_empty()
        && (snippet.contains("value_to_data(")
            || snippet.contains("from_value(")
            || snippet.contains("::from_lua(")
            || snippet.contains(" from_lua("))
    {
        tys.extend(match json_like_lua_type() {
            LuaType::Union(items) => items,
            ty => vec![ty],
        });
    }

    tys
}

fn is_type_id_call_on_binding(expr: &hir::Expr<'_>, binding_name: &str) -> bool {
    let expr = peel_try_expr(expr);
    matches!(
        &expr.kind,
        hir::ExprKind::MethodCall(segment, receiver, _, _)
            if segment.ident.name.as_str() == "type_id"
                && path_tail_name(receiver).as_deref() == Some(binding_name)
    )
}

fn extract_type_id_guard_lua_type(
    tcx: TyCtxt<'_>,
    guard: Option<&hir::Expr<'_>>,
) -> Option<LuaType> {
    let expr = peel_expr(unwrap_try_expr(guard?));
    let hir::ExprKind::Binary(op, lhs, rhs) = &expr.kind else {
        return None;
    };

    if op.node != hir::BinOpKind::Eq {
        return None;
    }

    extract_type_id_of_lua_type(tcx, lhs).or_else(|| extract_type_id_of_lua_type(tcx, rhs))
}

fn extract_type_id_of_lua_type(tcx: TyCtxt<'_>, expr: &hir::Expr<'_>) -> Option<LuaType> {
    let expr = peel_try_expr(expr);
    let hir::ExprKind::Call(callee, _) = &expr.kind else {
        return snippet_generic_type_name(&expr_snippet(tcx, expr), "TypeId::of::<")
            .map(LuaType::Class);
    };
    let args = match &callee.kind {
        hir::ExprKind::Path(hir::QPath::TypeRelative(_, seg))
            if seg.ident.name.as_str() == "of" =>
        {
            seg.args?
        }
        hir::ExprKind::Path(hir::QPath::Resolved(_, path)) => {
            let seg = path.segments.last()?;
            if seg.ident.name.as_str() != "of" {
                return None;
            }
            seg.args?
        }
        _ => return None,
    };

    let hir::GenericArg::Type(ty) = args.args.first()? else {
        return snippet_generic_type_name(&expr_snippet(tcx, expr), "TypeId::of::<")
            .map(LuaType::Class);
    };

    let hir::TyKind::Path(qpath) = &ty.kind else {
        return snippet_generic_type_name(&expr_snippet(tcx, expr), "TypeId::of::<")
            .map(LuaType::Class);
    };

    Some(LuaType::Class(extract_type_name_from_qpath(qpath)?))
}

fn lua_type_from_extracted_name(name: &str) -> LuaType {
    let name = name.trim();
    let qualified_name = last_path_segment(name).trim();

    if let Some(inner) = qualified_name
        .strip_prefix("Option<")
        .and_then(|inner| inner.strip_suffix('>'))
    {
        return LuaType::Optional(Box::new(lua_type_from_extracted_name(inner.trim())));
    }

    if let Some(inner) = qualified_name
        .strip_prefix("Vec<")
        .and_then(|inner| inner.strip_suffix('>'))
    {
        return LuaType::Array(Box::new(lua_type_from_extracted_name(inner.trim())));
    }

    for wrapper in [
        "UserDataRef<",
        "UserDataRefMut<",
        "Ref<",
        "RefMut<",
        "Cow<",
        "MutexGuard<",
        "RwLockReadGuard<",
        "RwLockWriteGuard<",
    ] {
        if let Some(inner) = qualified_name
            .strip_prefix(wrapper)
            .and_then(|inner| inner.strip_suffix('>'))
        {
            return lua_type_from_extracted_name(inner.trim());
        }
    }

    // Variadic<T> → T...
    if let Some(inner) = qualified_name
        .strip_prefix("Variadic<")
        .and_then(|inner| inner.strip_suffix('>'))
    {
        return LuaType::Variadic(Box::new(lua_type_from_extracted_name(inner.trim())));
    }

    // MultiValue → any...
    if qualified_name == "MultiValue" {
        return LuaType::Variadic(Box::new(LuaType::Any));
    }

    match qualified_name {
        "String" | "BString" | "LuaString" | "PathBuf" | "OsString" | "OsStr" | "CString"
        | "CStr" => LuaType::String,
        "str" => LuaType::String,
        "Integer" => LuaType::Integer,
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" => LuaType::Integer,
        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => LuaType::Integer,
        "Number" => LuaType::Number,
        "f32" | "f64" => LuaType::Number,
        "Boolean" => LuaType::Boolean,
        "bool" => LuaType::Boolean,
        "Table" | "LuaTable" => LuaType::Table,
        "Value" | "LuaValue" => LuaType::Any,
        "Function" | "LuaFunction" => LuaType::Function,
        "AnyUserData" | "LuaAnyUserData" => LuaType::Any,
        "Thread" | "LuaThread" => LuaType::Thread,
        "Lua" => LuaType::Any,
        "Error" | "LuaError" => LuaType::Nil,
        "()" => LuaType::Nil,
        _ => LuaType::Class(qualified_name.to_string()),
    }
}

fn last_path_segment(name: &str) -> &str {
    let mut depth = 0usize;
    let mut last = 0usize;
    let bytes = name.as_bytes();
    let mut i = 0usize;

    while i + 1 < bytes.len() {
        match bytes[i] as char {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ':' if depth == 0 && bytes[i + 1] as char == ':' => {
                last = i + 2;
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }

    &name[last..]
}

fn snippet_generic_type_name(snippet: &str, marker: &str) -> Option<String> {
    extract_all_generic_type_names(snippet, marker)
        .into_iter()
        .next()
}

fn extract_all_generic_type_names(snippet: &str, marker: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = snippet;

    while let Some(start) = rest.find(marker) {
        let tail = &rest[start + marker.len()..];
        let Some((name, consumed)) = extract_balanced_generic_arg(tail) else {
            break;
        };
        if let Some(name) = name
            .rsplit("::")
            .next()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            names.push(name.to_string());
        }
        rest = &tail[consumed..];
    }

    names
}

fn extract_balanced_generic_arg(input: &str) -> Option<(&str, usize)> {
    let mut depth = 0usize;

    for (idx, ch) in input.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => {
                if depth == 0 {
                    return Some((input[..idx].trim(), idx + ch.len_utf8()));
                }
                depth -= 1;
            }
            _ => {}
        }
    }

    None
}

fn snippet_type_before_method(snippet: &str, method: &str) -> Option<String> {
    let marker = format!("::{method}");
    let idx = snippet.find(&marker)?;
    let prefix = &snippet[..idx];
    let name = prefix.rsplit("::").next()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Get a name from a simple binding pattern, falling back to `_`.
fn pat_to_name(pat: &hir::Pat<'_>) -> String {
    match &pat.kind {
        hir::PatKind::Binding(_, _, ident, _) => {
            let name = ident.name.as_str();
            // Strip leading underscores (Rust unused-variable convention)
            let stripped = name.trim_start_matches('_');
            if stripped.is_empty() {
                name.to_string()
            } else {
                stripped.to_string()
            }
        }
        _ => "_".to_string(),
    }
}

fn terminal_return_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<&'tcx hir::Expr<'tcx>> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Block(block, _) => {
            block.expr.and_then(|expr| terminal_return_expr(tcx, expr))
        }
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            terminal_return_expr(tcx, body.value)
        }
        hir::ExprKind::Ret(Some(inner)) => terminal_return_expr(tcx, inner),
        hir::ExprKind::Call(callee, args) => {
            if let hir::ExprKind::Path(qpath) = &callee.kind {
                let name = match qpath {
                    hir::QPath::Resolved(_, path) => {
                        path.segments.last().map(|seg| seg.ident.name.as_str())
                    }
                    hir::QPath::TypeRelative(_, seg) => Some(seg.ident.name.as_str()),
                };
                if matches!(name, Some("Ok" | "Some")) && args.len() == 1 {
                    return terminal_return_expr(tcx, &args[0]);
                }
            }
            Some(expr)
        }
        hir::ExprKind::Match(_, arms, _) => arms
            .first()
            .and_then(|arm| terminal_return_expr(tcx, arm.body)),
        hir::ExprKind::If(_, then_expr, else_expr) => terminal_return_expr(tcx, then_expr)
            .or_else(|| else_expr.and_then(|expr| terminal_return_expr(tcx, expr))),
        _ => Some(expr),
    }
}

fn terminal_return_is_opaque_any_userdata<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> bool {
    let Some(expr) = terminal_return_expr(tcx, expr) else {
        return false;
    };

    if infer_wrapper_method_lua_type(tcx, peel_lua_conversion_expr(expr))
        .is_some_and(|ty| is_informative(&ty) && !matches!(ty, LuaType::Any))
    {
        return false;
    }

    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    let ty = unwrap_result_ty(tcx, typeck.expr_ty(expr));
    is_any_user_data(tcx, ty)
}

fn is_method_registration_name(name: &str) -> bool {
    matches!(
        name,
        "add_method"
            | "add_method_mut"
            | "add_method_once"
            | "add_async_method"
            | "add_async_method_mut"
            | "add_async_method_once"
            | "add_function"
            | "add_function_mut"
            | "add_async_function"
    )
}

fn should_trace_class_name(name: &str) -> bool {
    matches!(name, "Access" | "Path" | "Url" | "Style" | "File" | "Tab")
}

fn method_trace_summary(method: &LuaMethod) -> String {
    format!(
        "{} {:?} params={:?} returns={:?}",
        method.name, method.kind, method.params, method.returns
    )
}

fn field_trace_summary(field: &LuaField) -> String {
    format!(
        "{} ty={:?} writable={}",
        field.name, field.ty, field.writable
    )
}

fn trace_class_snapshot(label: &str, class_name: &str, fields: &[LuaField], methods: &[LuaMethod]) {
    if !should_trace_class_name(class_name) {
        return;
    }

    trace(format!(
        "{label} class={class_name} fields={:?} methods={:?}",
        fields.iter().map(field_trace_summary).collect::<Vec<_>>(),
        methods.iter().map(method_trace_summary).collect::<Vec<_>>()
    ));
}

fn trace_global_function_snapshot(label: &str, func: &LuaFunction) {
    if !matches!(func.name.as_str(), "Url" | "Path" | "File") {
        return;
    }

    trace(format!(
        "{label} global_function={} params={:?} returns={:?}",
        func.name, func.params, func.returns
    ));
}

fn body_returns_named_userdata_self(expr: &hir::Expr<'_>, self_name: &str) -> bool {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Block(block, _) => block
            .expr
            .is_some_and(|expr| body_returns_named_userdata_self(expr, self_name)),
        hir::ExprKind::Ret(Some(inner)) => body_returns_named_userdata_self(inner, self_name),
        hir::ExprKind::Call(callee, args) => {
            if let hir::ExprKind::Path(qpath) = &callee.kind {
                let name = match qpath {
                    hir::QPath::Resolved(_, path) => {
                        path.segments.last().map(|seg| seg.ident.name.as_str())
                    }
                    hir::QPath::TypeRelative(_, seg) => Some(seg.ident.name.as_str()),
                };
                if matches!(name, Some("Ok" | "Some")) && args.len() == 1 {
                    return body_returns_named_userdata_self(&args[0], self_name);
                }
            }
            false
        }
        hir::ExprKind::MethodCall(segment, receiver, _, _) => {
            matches!(
                segment.ident.name.as_str(),
                "into_lua" | "into" | "clone" | "to_owned" | "borrow" | "as_ref"
            ) && body_returns_named_userdata_self(receiver, self_name)
        }
        hir::ExprKind::Match(_, arms, _) => arms
            .iter()
            .any(|arm| body_returns_named_userdata_self(arm.body, self_name)),
        hir::ExprKind::If(_, then_expr, else_expr) => {
            body_returns_named_userdata_self(then_expr, self_name)
                || else_expr.is_some_and(|expr| body_returns_named_userdata_self(expr, self_name))
        }
        hir::ExprKind::Path(hir::QPath::Resolved(_, path)) => path
            .segments
            .last()
            .is_some_and(|seg| seg.ident.name.as_str() == self_name),
        _ => false,
    }
}

fn extract_params_from_tuple<'tcx>(
    tcx: TyCtxt<'tcx>,
    ty: ty::Ty<'tcx>,
    hir_names: &[String],
) -> Vec<LuaParam> {
    match ty.kind() {
        ty::TyKind::Tuple(fields) => fields
            .iter()
            .enumerate()
            .map(|(i, field_ty)| LuaParam {
                name: hir_names
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("p{}", i + 1)),
                ty: map_ty_to_lua(tcx, field_ty),
            })
            .collect(),
        _ => {
            if ty.is_unit() {
                Vec::new()
            } else {
                let name = hir_names
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "p1".to_string());
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
        "std::vec::Vec"
            | "alloc::vec::Vec"
            | "std::collections::VecDeque"
            | "std::collections::vec_deque::VecDeque"
            | "arrayvec::ArrayVec"
            | "smallvec::SmallVec"
            | "tinyvec::TinyVec"
            | "tinyvec::ArrayVec"
            | "thin_vec::ThinVec"
    )
}

/// Check if a trait path is a Fn-family trait (Fn, FnMut, FnOnce, async variants).
fn is_fn_trait(path: &str) -> bool {
    matches!(
        path,
        "std::ops::Fn"
            | "core::ops::Fn"
            | "std::ops::FnMut"
            | "core::ops::FnMut"
            | "std::ops::FnOnce"
            | "core::ops::FnOnce"
            | "std::ops::AsyncFn"
            | "core::ops::AsyncFn"
            | "std::ops::AsyncFnMut"
            | "core::ops::AsyncFnMut"
            | "std::ops::AsyncFnOnce"
            | "core::ops::AsyncFnOnce"
    )
}

/// Check if a trait path is a string-like trait (Display, ToString, Error).
fn is_string_trait(path: &str) -> bool {
    matches!(
        path,
        "std::fmt::Display"
            | "core::fmt::Display"
            | "std::string::ToString"
            | "alloc::string::ToString"
            | "std::error::Error"
            | "core::error::Error"
    )
}

/// Check if a trait path is Iterator or IntoIterator.
fn is_iterator_trait(path: &str) -> bool {
    matches!(
        path,
        "std::iter::Iterator"
            | "core::iter::Iterator"
            | "std::iter::IntoIterator"
            | "core::iter::IntoIterator"
    )
}

/// Check if a trait path is AsRef or Borrow (generic over the target type).
fn is_asref_trait(path: &str) -> bool {
    matches!(
        path,
        "std::convert::AsRef"
            | "core::convert::AsRef"
            | "std::borrow::Borrow"
            | "core::borrow::Borrow"
    )
}

/// Check if a trait path is Into or From (generic conversion trait).
fn is_into_trait(path: &str) -> bool {
    matches!(path, "std::convert::Into" | "core::convert::Into")
}

/// Check if a trait path is Deref.
fn is_deref_trait(path: &str) -> bool {
    matches!(path, "std::ops::Deref" | "core::ops::Deref")
}

/// Find a projection for a given associated type name (e.g. "Item", "Target", "Output").
fn find_projection<'tcx>(
    tcx: TyCtxt<'tcx>,
    predicates: &'tcx ty::List<ty::PolyExistentialPredicate<'tcx>>,
    assoc_name: &str,
) -> Option<ty::Ty<'tcx>> {
    for pred in predicates.iter() {
        if let ty::ExistentialPredicate::Projection(proj) = pred.skip_binder()
            && tcx.item_name(proj.def_id).as_str() == assoc_name
        {
            return proj.term.as_type();
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
        if let ty::ExistentialPredicate::Trait(trait_ref) = pred.skip_binder() {
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
                        if lua_ty == LuaType::Nil {
                            vec![]
                        } else {
                            vec![lua_ty]
                        }
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
            if is_asref_trait(&trait_path)
                && let Some(target) = trait_ref.args.iter().find_map(|a| a.as_type())
            {
                return map_ty_to_lua(tcx, target);
            }

            // dyn Into<T> → map T
            if is_into_trait(&trait_path)
                && let Some(target) = trait_ref.args.iter().find_map(|a| a.as_type())
            {
                return map_ty_to_lua(tcx, target);
            }

            // dyn Deref<Target = T> → map T
            if is_deref_trait(&trait_path)
                && let Some(target_ty) = find_projection(tcx, predicates, "Target")
            {
                return map_ty_to_lua(tcx, target_ty);
            }

            // dyn Iterator<Item = T> / dyn IntoIterator<Item = T> → T[]
            if is_iterator_trait(&trait_path) {
                if let Some(item_ty) = find_projection(tcx, predicates, "Item") {
                    return LuaType::Array(Box::new(map_ty_to_lua(tcx, item_ty)));
                }
                return LuaType::Array(Box::new(LuaType::Any));
            }
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
            let params: Vec<LuaType> = sig
                .inputs()
                .iter()
                .map(|t| map_ty_to_lua(tcx, *t))
                .collect();
            let ret = map_ty_to_lua(tcx, sig.output());
            let returns = if ret == LuaType::Nil {
                vec![]
            } else {
                vec![ret]
            };
            if params.is_empty() && returns.is_empty() {
                LuaType::Function
            } else {
                LuaType::FunctionSig { params, returns }
            }
        }

        // FnDef and closures → function (signature not easily extractable inline)
        ty::TyKind::FnDef(..) | ty::TyKind::Closure(..) => LuaType::Function,

        // dyn Trait → inspect the trait to pick a better type
        ty::TyKind::Dynamic(predicates, ..) => map_dyn_trait(tcx, predicates),

        ty::TyKind::Adt(adt_def, args) => {
            let path = qualified_def_path_str(tcx, adt_def.did());

            // Vec<u8> / VecDeque<u8> etc. → string (byte buffer)
            if is_byte_container(&path)
                && let Some(inner) = args.types().next()
                && is_u8(&inner)
            {
                return LuaType::String;
            }

            let type_args: Vec<LuaType> = args.types().map(|t| map_ty_to_lua(tcx, t)).collect();
            map_rust_type(&path, &type_args)
        }

        ty::TyKind::Alias(_, alias) => {
            let instantiated = tcx.type_of(alias.def_id).instantiate(tcx, alias.args);
            let mapped = map_ty_to_lua(tcx, instantiated);
            if is_informative(&mapped) {
                mapped
            } else {
                let path = qualified_def_path_str(tcx, alias.def_id);
                let type_args: Vec<LuaType> =
                    alias.args.types().map(|t| map_ty_to_lua(tcx, t)).collect();
                let fallback = map_rust_type(&path, &type_args);
                if is_informative(&fallback) {
                    fallback
                } else {
                    let name = path.rsplit("::").next().unwrap_or_default();
                    // Opaque types (impl Trait) resolve to {opaque#N} — fall back to any
                    if name.starts_with('{') || path.contains("{opaque") {
                        LuaType::Any
                    } else {
                        let has_informative_args =
                            type_args.iter().any(|a| !matches!(a, LuaType::Any));
                        if has_informative_args {
                            let args = type_args
                                .iter()
                                .map(|a| a.to_string())
                                .collect::<Vec<_>>()
                                .join(", ");
                            LuaType::Class(format!("{name}<{args}>"))
                        } else {
                            LuaType::Class(name.to_string())
                        }
                    }
                }
            }
        }

        _ => LuaType::Any,
    }
}

/// Returns true if the type is an error class that leaked through Result unwrapping.
/// In Lua, errors are thrown (not returned), so these should be stripped from returns.
fn is_error_class(ty: &LuaType) -> bool {
    matches!(ty, LuaType::Class(name) if name == "Error" || name == "LuaError"
        || name.ends_with("::Error") || name.ends_with("::LuaError"))
}

/// Map a return type to a list of Lua return values.
/// Handles Result unwrapping and tuple decomposition for multiple returns.
fn map_return_ty<'tcx>(tcx: TyCtxt<'tcx>, ty: ty::Ty<'tcx>) -> Vec<LuaReturn> {
    // Unwrap Coroutine (async closures) to get the actual return type
    let ty = unwrap_coroutine_ty(tcx, ty);
    // Unwrap impl Future<Output = T> from async fn signatures
    let ty = unwrap_future_output(tcx, ty);
    // Then unwrap Result<T, _> → T
    let ty = unwrap_result_ty(tcx, ty);

    match ty.kind() {
        // Empty tuple = no return values
        ty::TyKind::Tuple(fields) if fields.is_empty() => Vec::new(),

        // Non-empty tuple = multiple return values
        ty::TyKind::Tuple(fields) => fields
            .iter()
            .map(|t| LuaReturn::from(map_ty_to_lua(tcx, t)))
            .filter(|r| !is_error_class(&r.ty))
            .collect(),

        // Single return value
        _ => {
            let lua_ty = map_ty_to_lua(tcx, ty);
            if lua_ty == LuaType::Nil || is_error_class(&lua_ty) {
                Vec::new()
            } else {
                vec![lua_ty.into()]
            }
        }
    }
}

/// Unwrap Result<T, _> to get T. If not a Result, returns the type unchanged.
/// Unwrap `impl Future<Output = T>` from async fn signatures.
/// The opaque type from `fn_sig` resolves through its bounds to find the Output projection.
fn unwrap_future_output<'tcx>(tcx: TyCtxt<'tcx>, ty: ty::Ty<'tcx>) -> ty::Ty<'tcx> {
    if let ty::TyKind::Alias(_, alias) = ty.kind()
        && matches!(
            tcx.def_kind(alias.def_id),
            rustc_hir::def::DefKind::OpaqueTy
        )
    {
        let bounds = tcx.explicit_item_bounds(alias.def_id);
        for (bound, _) in bounds.skip_binder() {
            if let ty::ClauseKind::Projection(proj) = bound.kind().skip_binder()
                && tcx.item_name(proj.projection_term.def_id).as_str() == "Output"
                && let Some(output_ty) = proj.term.as_type()
            {
                return ty::EarlyBinder::bind(output_ty).instantiate(tcx, alias.args);
            }
        }
    }
    ty
}

fn unwrap_result_ty<'tcx>(tcx: TyCtxt<'tcx>, ty: ty::Ty<'tcx>) -> ty::Ty<'tcx> {
    match ty.kind() {
        ty::TyKind::Adt(adt_def, args) => {
            let path = tcx.def_path_str(adt_def.did());
            if (path == "std::result::Result" || path == "core::result::Result")
                && let Some(inner) = args.types().next()
            {
                return inner;
            }
            ty
        }
        // Type aliases like anyhow::Result<T> resolve through Alias
        ty::TyKind::Alias(_, alias) => {
            let instantiated = tcx.type_of(alias.def_id).instantiate(tcx, alias.args);
            let unwrapped = unwrap_result_ty(tcx, instantiated);
            if unwrapped != instantiated {
                unwrapped
            } else {
                ty
            }
        }
        _ => ty,
    }
}

/// Unwrap a Coroutine type (from async closures) to find the Result<T, E> return type.
/// Async closures produce: Coroutine(DefId, [move_id, (), ResumeTy, (), Result<T, E>, ...])
fn unwrap_coroutine_ty<'tcx>(tcx: TyCtxt<'tcx>, ty: ty::Ty<'tcx>) -> ty::Ty<'tcx> {
    if let ty::TyKind::Coroutine(_, args) = ty.kind() {
        let mut fallback = None;

        // Coroutine args contain many intermediate future/output types.
        // Prefer the last Result<_, mlua::Error>, which best matches the
        // async closure's actual output, and otherwise fall back to the last Result.
        for arg in args.iter().rev() {
            let Some(inner_ty) = arg.as_type() else {
                continue;
            };
            let ty::TyKind::Adt(adt_def, result_args) = inner_ty.kind() else {
                continue;
            };

            let path = tcx.def_path_str(adt_def.did());
            if path != "std::result::Result" && path != "core::result::Result" {
                continue;
            }

            if fallback.is_none() {
                fallback = Some(inner_ty);
            }

            if let Some(err_ty) = result_args.types().nth(1)
                && let ty::TyKind::Adt(err_def, _) = err_ty.kind()
            {
                let err_path = tcx.def_path_str(err_def.did());
                if (err_path.contains("mlua") && err_path.ends_with("Error"))
                    || err_path.ends_with("mlua::Error")
                    || err_path == "mlua::Error"
                {
                    return inner_ty;
                }
            }
        }

        if let Some(result_ty) = fallback {
            return result_ty;
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
/// Also rejects Optional(Any) — "maybe anything" is not useful.
fn is_informative(ty: &LuaType) -> bool {
    match ty {
        LuaType::Any | LuaType::Nil | LuaType::Function => false,
        LuaType::Optional(inner) => is_informative(inner),
        LuaType::Variadic(inner) => is_informative(inner),
        LuaType::Class(name) if name.contains("{opaque") || name.starts_with('{') => false,
        _ => true,
    }
}

/// Map a `LuaValue::Variant` constructor name to a concrete LuaType.
/// Returns None if the variant doesn't map to a specific type (e.g. UserData, Error).
fn lua_value_variant_to_type(variant_name: &str) -> Option<LuaType> {
    match variant_name {
        "Nil" => Some(LuaType::Nil),
        "Boolean" => Some(LuaType::Boolean),
        "Integer" => Some(LuaType::Integer),
        "Number" => Some(LuaType::Number),
        "String" => Some(LuaType::String),
        "LightUserData" => Some(LuaType::Any),
        "Table" => Some(LuaType::Table),
        "Function" => Some(LuaType::Function),
        "Thread" => Some(LuaType::Thread),
        _ => None, // UserData, Error — can't narrow further
    }
}

/// Check if an expression is a `LuaValue::Variant(...)` or `Value::Variant(...)` constructor
/// and return the corresponding LuaType.
fn try_lua_value_constructor<'tcx>(
    tcx: TyCtxt<'tcx>,
    typeck: &'tcx ty::TypeckResults<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    match &expr.kind {
        // LuaValue::Nil (path expression, no call)
        hir::ExprKind::Path(qpath) => {
            let variant = match qpath {
                hir::QPath::Resolved(_, path) => {
                    path.segments.last().map(|seg| seg.ident.name.as_str())
                }
                hir::QPath::TypeRelative(_, seg) => Some(seg.ident.name.as_str()),
            }?;

            let ty = typeck.expr_ty(expr);
            if let ty::TyKind::Adt(adt_def, _) = ty.kind() {
                let path_str = tcx.def_path_str(adt_def.did());
                if path_str.contains("Value") && !path_str.contains("MultiValue") {
                    return lua_value_variant_to_type(variant);
                }
            }
            None
        }
        // LuaValue::String(x), LuaValue::Integer(x), etc. (call expression)
        hir::ExprKind::Call(callee, _) => {
            if let hir::ExprKind::Path(hir::QPath::TypeRelative(_, seg)) = &callee.kind {
                return lua_value_variant_to_type(seg.ident.name.as_str());
            }
            if let hir::ExprKind::Path(hir::QPath::Resolved(_, path)) = &callee.kind
                && let Some(seg) = path.segments.last()
            {
                // Verify the return type is a Value type
                let ty = typeck.expr_ty(expr);
                if let ty::TyKind::Adt(adt_def, _) = ty.kind() {
                    let path_str = tcx.def_path_str(adt_def.did());
                    if path_str.contains("Value") && !path_str.contains("MultiValue") {
                        return lua_value_variant_to_type(seg.ident.name.as_str());
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Extract a type name from a QPath (e.g. `TypeName::make`)
fn extract_type_name_from_qpath(qpath: &hir::QPath<'_>) -> Option<String> {
    let name = match qpath {
        hir::QPath::TypeRelative(ty, _) => {
            if let hir::TyKind::Path(hir::QPath::Resolved(_, path)) = &ty.kind {
                path.segments
                    .last()
                    .map(|s| s.ident.name.as_str().to_string())
            } else {
                None
            }
        }
        hir::QPath::Resolved(_, path) => {
            let last = path.segments.last()?.ident.name.as_str().to_string();
            let last_starts_like_type = last.chars().next().is_some_and(|c| c.is_uppercase());

            if last_starts_like_type || path.segments.len() == 1 {
                Some(last)
            } else if path.segments.len() >= 2 {
                Some(
                    path.segments[path.segments.len() - 2]
                        .ident
                        .name
                        .as_str()
                        .to_string(),
                )
            } else {
                None
            }
        }
    }?;

    let starts_like_type = name.chars().next().is_some_and(|c| c.is_uppercase());

    if matches!(name.as_str(), "Ok" | "Err" | "Some" | "None")
        || name.starts_with("__")
        || (!starts_like_type && name != "Self")
    {
        None
    } else {
        Some(name)
    }
}

/// Extract the return type name from a `fn(...) -> Result<TypeName>` cast expression.
/// Uses typeck to get the actual return type of the call expression, then
/// checks if the cast target (the $value closure) body reveals the type.
fn extract_return_type_from_cast<'tcx>(
    tcx: TyCtxt<'tcx>,
    cast_expr: &'tcx hir::Expr<'tcx>,
    _cast_ty: &hir::Ty<'_>,
) -> Option<String> {
    // The cast expression is typically a closure literal like `|_, me| Mode::make(&me.mode)`
    // Try to extract the type from its body
    extract_type_from_closure_expr(tcx, cast_expr)
}

fn extract_type_name_from_value_path_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<String> {
    let hir::ExprKind::Path(qpath) = &expr.kind else {
        return None;
    };

    match qpath {
        hir::QPath::TypeRelative(_, _) => extract_type_name_from_qpath(qpath),
        hir::QPath::Resolved(_, _) => {
            let res = tcx
                .typeck(expr.hir_id.owner.def_id)
                .qpath_res(qpath, expr.hir_id);
            matches!(
                res,
                rustc_hir::def::Res::Def(
                    rustc_hir::def::DefKind::Ctor(rustc_hir::def::CtorOf::Struct, _),
                    _
                )
            )
            .then(|| extract_type_name_from_qpath(qpath))
            .flatten()
        }
    }
}

/// Try to extract a class name from a closure expression by looking at what it calls.
/// Recursively searches through closures, blocks, method chains, and call expressions.
fn extract_type_from_closure_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<String> {
    match &expr.kind {
        hir::ExprKind::DropTemps(inner)
        | hir::ExprKind::Use(inner, _)
        | hir::ExprKind::Cast(inner, _)
        | hir::ExprKind::Type(inner, _)
        | hir::ExprKind::AddrOf(_, _, inner)
        | hir::ExprKind::Unary(_, inner)
        | hir::ExprKind::Field(inner, _)
        | hir::ExprKind::Become(inner)
        | hir::ExprKind::Yield(inner, _)
        | hir::ExprKind::UnsafeBinderCast(_, inner, _) => {
            extract_type_from_closure_expr(tcx, inner)
        }
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            extract_type_from_closure_expr(tcx, body.value)
        }
        hir::ExprKind::Block(block, _) => {
            if let Some(tail) = block.expr
                && let Some(name) = extract_type_from_closure_expr(tcx, tail)
            {
                return Some(name);
            }
            // Check statements too
            for stmt in block.stmts {
                if let hir::StmtKind::Semi(e) | hir::StmtKind::Expr(e) = &stmt.kind
                    && let Some(name) = extract_type_from_closure_expr(tcx, e)
                {
                    return Some(name);
                }
            }
            None
        }
        hir::ExprKind::Call(callee, args) => {
            if let Some(name) = extract_type_name_from_value_path_expr(tcx, callee) {
                return Some(name);
            }
            // Check args (e.g. the closure in .map(|x| TypeName::make(x)))
            for arg in *args {
                if let Some(name) = extract_type_from_closure_expr(tcx, arg) {
                    return Some(name);
                }
            }
            None
        }
        // Method chains like .map(|x| TypeName::make(x)).transpose()
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            // Check receiver (the chain before this method)
            if let Some(name) = extract_type_from_closure_expr(tcx, receiver) {
                return Some(name);
            }
            // Check args (closures passed to .map(), .and_then(), etc.)
            for arg in *args {
                if let Some(name) = extract_type_from_closure_expr(tcx, arg) {
                    return Some(name);
                }
            }
            None
        }
        hir::ExprKind::Match(scrut, arms, _) => {
            if let Some(name) = extract_type_from_closure_expr(tcx, scrut) {
                return Some(name);
            }
            for arm in *arms {
                if let Some(name) = extract_type_from_closure_expr(tcx, arm.body) {
                    return Some(name);
                }
            }
            None
        }
        // If/else — check both branches
        hir::ExprKind::If(_, then_expr, else_expr) => {
            if let Some(name) = extract_type_from_closure_expr(tcx, then_expr) {
                return Some(name);
            }
            if let Some(e) = else_expr {
                extract_type_from_closure_expr(tcx, e)
            } else {
                None
            }
        }
        // Bare function reference like Filter::make — extract the type from the path
        hir::ExprKind::Path(_) => extract_type_name_from_value_path_expr(tcx, expr),
        _ => None,
    }
}

/// Try to resolve the class name from an expression that produces AnyUserData.
/// Handles patterns like:
///   - `TypeName::make(...)` — associated function call where TypeName is a UserData class
///   - `lua.create_any_userdata(TypeName { ... })` — direct creation
///   - `expr?` (Result unwrapping via ? operator, desugared to Match)
fn resolve_any_user_data_class<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    match &expr.kind {
        // TypeName::method(...) — check if TypeName is a known class
        hir::ExprKind::Call(callee, args) => {
            // Check callee path for TypeName::make(...) pattern
            let type_name = match &callee.kind {
                hir::ExprKind::Path(qpath) => extract_type_name_from_qpath(qpath),
                // ($value as fn(&Lua, &Self) -> Result<_>)(lua, me) — cast expr
                hir::ExprKind::Cast(inner, cast_ty) => {
                    extract_return_type_from_cast(tcx, inner, cast_ty)
                }
                _ => None,
            };
            if let Some(name) = type_name {
                return Some(LuaType::Class(name));
            }
            // Recurse into call args
            for arg in *args {
                if let Some(ty) = resolve_any_user_data_class(tcx, arg) {
                    return Some(ty);
                }
            }
            None
        }
        // expr? desugars to Match — recurse into the Ok arm
        hir::ExprKind::Match(scrut, arms, _) => {
            resolve_any_user_data_class(tcx, scrut).or_else(|| {
                arms.iter()
                    .find_map(|arm| resolve_any_user_data_class(tcx, arm.body))
            })
        }
        // expr.method()? or similar chains
        // Also handles lua.create_any_userdata(concrete_value) — trace arg type
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            let method_name = segment.ident.name.as_str();
            if method_name == "create_any_userdata" && !args.is_empty() {
                let typeck = tcx.typeck(args[0].hir_id.owner.def_id);
                let arg_ty = typeck.expr_ty(&args[0]);
                if let ty::TyKind::Adt(..) = arg_ty.kind() {
                    let name = type_display_name(tcx, arg_ty);
                    return Some(LuaType::Class(name));
                }
            }
            if is_passthrough_method(method_name) {
                resolve_any_user_data_class(tcx, receiver)
            } else {
                args.iter()
                    .find_map(|arg| resolve_any_user_data_class(tcx, arg))
            }
        }
        // Block — check tail expression
        hir::ExprKind::Block(block, _) => {
            block.expr.and_then(|e| resolve_any_user_data_class(tcx, e))
        }
        // Closure — cross boundary
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            resolve_any_user_data_class(tcx, body.value)
        }
        _ => None,
    }
}

/// Walk a closure body to infer the concrete type when the signature says `Value` (any).
/// Recursively searches the entire expression tree for `.into_lua()` calls and
/// returns the type of the receiver — the concrete type before Lua conversion.
fn infer_concrete_type_from_body<'tcx>(
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
fn infer_multi_returns_from_body<'tcx>(
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

fn infer_forwarded_multivalue_returns<'tcx>(
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

fn extract_runtime_method_name(tcx: TyCtxt<'_>, expr: &hir::Expr<'_>) -> Option<String> {
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

fn infer_forwarded_userdata_state_type<'tcx>(
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

fn infer_informative_expr_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    infer_from_expr(tcx, typeck, expr).filter(is_informative)
}

fn infer_userdata_field_init_type<'tcx>(
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

fn infer_struct_field_init_type_from_item<'tcx>(
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

fn expr_userdata_container_name<'tcx>(
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

fn infer_struct_field_init_type_from_expr<'tcx>(
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

fn infer_pairs_forward_returns(state: &LuaType) -> Vec<LuaReturn> {
    let value = infer_pairs_value_type(state).unwrap_or(LuaType::Any);
    vec![
        LuaType::FunctionSig {
            params: vec![state.clone()],
            returns: vec![
                LuaType::Optional(Box::new(LuaType::Integer)),
                LuaType::Optional(Box::new(value)),
            ],
        }
        .into(),
        state.clone().into(),
        LuaType::Nil.into(),
    ]
}

fn infer_pairs_state_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    state: &LuaType,
) -> LuaType {
    let LuaType::Class(name) = state else {
        return state.clone();
    };

    // Handle bare "Iter" or "Iter<...>" (where the first generic arg is an
    // internal Rust iterator type that shouldn't leak into Lua annotations).
    let is_iter = name == "Iter" || name.starts_with("Iter<");
    if !is_iter {
        return state.clone();
    }

    // If the class name already carries a value type (second generic arg),
    // extract it directly rather than re-inferring from the expression.
    if let Some(value) = extract_iter_value_arg_from_class_name(name) {
        return LuaType::Class(format!("Iter<any, {value}>"));
    }

    // Bare "Iter" — try to recover the value type from the expression.
    let Some(value) = infer_iter_alias_value_type(tcx, expr)
        .or_else(|| infer_iter_alias_value_type_from_wrapper_arg(tcx, expr))
    else {
        return state.clone();
    };

    LuaType::Class(format!("Iter<any, {value}>"))
}

/// Given a class name like `"Iter<SomeComplexType, Url>"`, extract the second
/// generic argument (`"Url"`) if present.
fn extract_iter_value_arg_from_class_name(name: &str) -> Option<String> {
    let inner = name.strip_prefix("Iter<")?.strip_suffix('>')?;
    // Split at top-level commas (respecting nested angle brackets).
    let mut depth = 0i32;
    let mut last_comma = None;
    for (i, ch) in inner.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => last_comma = Some(i),
            _ => {}
        }
    }
    let comma_pos = last_comma?;
    let value = inner[comma_pos + 1..].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn infer_iter_alias_value_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let ty = tcx.typeck(expr.hir_id.owner.def_id).expr_ty(expr);
    extract_iter_value_type_from_ty(tcx, ty)
}

fn infer_iter_alias_value_type_from_wrapper_arg<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let expr = peel_expr(peel_try_expr(expr));
    let hir::ExprKind::MethodCall(_, _, args, _) = &expr.kind else {
        return None;
    };

    args.get(1).and_then(|arg| {
        let arg = peel_expr(peel_try_expr(arg));
        if let hir::ExprKind::MethodCall(segment, receiver, _, _) = &arg.kind
            && segment.ident.name.as_str() == "clone"
        {
            infer_iter_alias_value_type(tcx, receiver)
        } else {
            infer_iter_alias_value_type(tcx, arg)
        }
    })
}

fn extract_iter_value_type_from_ty<'tcx>(tcx: TyCtxt<'tcx>, ty: ty::Ty<'tcx>) -> Option<LuaType> {
    match ty.kind() {
        ty::TyKind::Alias(_, alias) => extract_iter_value_type_from_ty(
            tcx,
            tcx.type_of(alias.def_id).instantiate(tcx, alias.args),
        ),
        ty::TyKind::Adt(adt_def, args) => {
            let path = qualified_def_path_str(tcx, adt_def.did());
            if path.ends_with("::Iter") || path == "Iter" {
                args.types().nth(1).map(|ty| map_ty_to_lua(tcx, ty))
            } else {
                None
            }
        }
        ty::TyKind::Ref(_, inner, _) => extract_iter_value_type_from_ty(tcx, *inner),
        _ => None,
    }
}

fn infer_pairs_value_type(state: &LuaType) -> Option<LuaType> {
    let LuaType::Class(name) = state else {
        return None;
    };
    if name == "Iter" {
        return Some(LuaType::Any);
    }
    if let Some((base, args)) = parse_embedded_class_args(name) {
        return (base == "Iter" && args.len() >= 2).then(|| args[1].clone());
    }

    None
}

fn parse_embedded_class_args(name: &str) -> Option<(&str, Vec<LuaType>)> {
    let start = name.find('<')?;
    let inner = name.get(start + 1..name.len().checked_sub(1)?)?;
    name.ends_with('>').then_some(())?;

    Some((
        name[..start].trim(),
        split_top_level(inner, ',')
            .into_iter()
            .map(lua_type_from_extracted_name)
            .collect(),
    ))
}

fn infer_returns_from_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<Vec<LuaReturn>> {
    let expr = peel_expr(peel_try_expr(expr));

    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            match segment.ident.name.as_str() {
                "into_lua_multi" => Some(infer_into_lua_multi_returns(tcx, receiver)),
                "into_lua" => Some(vec![infer_value_expr_lua_type(tcx, receiver).into()]),
                // lua.to_value(&data) → infer type from the data argument
                "to_value" | "to_value_with" if !args.is_empty() => {
                    let typeck = tcx.typeck(args[0].hir_id.owner.def_id);
                    let ty = typeck.node_type(args[0].hir_id);
                    let lua_ty = map_ty_to_lua(tcx, ty);
                    if is_informative(&lua_ty) {
                        Some(vec![lua_ty.into()])
                    } else {
                        None
                    }
                }
                // func.call(args) / func.call_async(args) where func comes from
                // table.get("name") — emit deferred lookup marker for cross-crate resolution
                "call" | "call_async"
                    if let Some(func_name) = resolve_table_get_function_name(tcx, receiver) =>
                {
                    Some(vec![LuaType::Class(format!("@lookup:{func_name}")).into()])
                }
                _ if infer_forwarded_multivalue_returns(tcx, expr, segment, args).is_some() => {
                    infer_forwarded_multivalue_returns(tcx, expr, segment, args)
                }
                _ => infer_wrapper_method_lua_type(tcx, expr).map(|ty| vec![ty.into()]),
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
                    return infer_returns_from_expr(tcx, &args[0]);
                }
                // to_lua / dynamic_to_lua_value / to_dynamic → infer from data arg
                if matches!(
                    name,
                    Some(
                        "to_lua" | "to_dynamic" | "from_lua_value_dynamic" | "dynamic_to_lua_value"
                    )
                ) && !args.is_empty()
                {
                    // Last arg is the data (first might be lua context)
                    let data_arg = args.last().unwrap();
                    // Peel .to_dynamic() to get the original typed value
                    let data_arg = peel_to_dynamic(data_arg);
                    let typeck = tcx.typeck(data_arg.hir_id.owner.def_id);
                    let ty = typeck.node_type(data_arg.hir_id);
                    let lua_ty = map_ty_to_lua(tcx, ty);
                    if is_informative(&lua_ty) {
                        return Some(vec![lua_ty.into()]);
                    }
                }
            }
            None
        }
        hir::ExprKind::Tup([]) => Some(Vec::new()),
        hir::ExprKind::Block(block, _) => {
            let mut branches = Vec::new();

            for stmt in block.stmts {
                match &stmt.kind {
                    hir::StmtKind::Let(local) => {
                        // Skip ? desugaring in let bindings — those are error paths
                        if let Some(init) = local.init
                            && !is_try_desugar_expr(init)
                            && expr_contains_return(init)
                            && let Some(returns) = infer_returns_from_expr(tcx, init)
                        {
                            branches.push(returns);
                        }
                    }
                    hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                        // Skip ? desugaring statements — those are error paths
                        if !is_try_desugar_expr(expr)
                            && expr_contains_return(expr)
                            && let Some(returns) = infer_returns_from_expr(tcx, expr)
                        {
                            branches.push(returns);
                        }
                    }
                    _ => {}
                }
            }

            if let Some(tail) = block
                .expr
                .and_then(|tail| infer_returns_from_expr(tcx, tail))
            {
                branches.push(tail);
            }

            merge_return_branches(branches)
        }
        hir::ExprKind::Match(scrutinee, arms, source) => {
            let mut branches = Vec::new();
            if (matches!(source, hir::MatchSource::TryDesugar(_))
                || is_result_like_expr(tcx, scrutinee))
                && let Some(returns) = infer_returns_from_expr(tcx, scrutinee)
            {
                branches.push(returns);
            }
            branches.extend(
                arms.iter()
                    .filter_map(|arm| infer_returns_from_expr(tcx, arm.body)),
            );
            merge_return_branches(branches)
        }
        hir::ExprKind::If(_, then_expr, else_expr) => {
            let mut branches = Vec::new();
            if let Some(returns) = infer_returns_from_expr(tcx, then_expr) {
                branches.push(returns);
            }
            if let Some(else_expr) = else_expr.and_then(|expr| infer_returns_from_expr(tcx, expr)) {
                branches.push(else_expr);
            }
            merge_return_branches(branches)
        }
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            infer_returns_from_expr(tcx, body.value)
        }
        hir::ExprKind::Ret(Some(expr)) => infer_returns_from_expr(tcx, expr),
        // Resolve local variable references to their initializer
        hir::ExprKind::Path(hir::QPath::Resolved(_, path))
            if matches!(path.res, rustc_hir::def::Res::Local(_)) =>
        {
            if let rustc_hir::def::Res::Local(hir_id) = path.res
                && let rustc_hir::Node::Pat(pat) = tcx.hir_node(hir_id)
                && let rustc_hir::Node::LetStmt(local) = tcx.hir_node(tcx.parent_hir_id(pat.hir_id))
                && let Some(init) = local.init
            {
                let init = peel_try_expr(init);
                infer_returns_from_expr(tcx, init)
            } else {
                None
            }
        }
        _ => {
            let typeck = tcx.typeck(expr.hir_id.owner.def_id);
            infer_from_expr(tcx, typeck, expr).map(|ty| vec![ty.into()])
        }
    }
}

fn expr_contains_return(expr: &hir::Expr<'_>) -> bool {
    match &expr.kind {
        hir::ExprKind::Ret(_) => true,
        hir::ExprKind::Block(block, _) => {
            block.stmts.iter().any(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local.init.is_some_and(expr_contains_return),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => expr_contains_return(expr),
                _ => false,
            }) || block.expr.is_some_and(expr_contains_return)
        }
        hir::ExprKind::Call(callee, args) => {
            expr_contains_return(callee) || args.iter().any(|arg| expr_contains_return(arg))
        }
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            expr_contains_return(receiver) || args.iter().any(|arg| expr_contains_return(arg))
        }
        hir::ExprKind::Match(scrutinee, arms, _) => {
            expr_contains_return(scrutinee) || arms.iter().any(|arm| expr_contains_return(arm.body))
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            expr_contains_return(cond)
                || expr_contains_return(then_expr)
                || else_expr.is_some_and(expr_contains_return)
        }
        hir::ExprKind::Closure(_) => false,
        _ => false,
    }
}

fn infer_into_lua_multi_returns<'tcx>(
    tcx: TyCtxt<'tcx>,
    receiver: &'tcx hir::Expr<'tcx>,
) -> Vec<LuaReturn> {
    let receiver = peel_lua_conversion_expr(receiver);
    let typeck = tcx.typeck(receiver.hir_id.owner.def_id);
    let receiver_ty = typeck.expr_ty(receiver);

    if let ty::TyKind::Tuple(fields) = receiver_ty.kind() {
        let names = extract_tuple_element_names(receiver);
        return fields
            .iter()
            .enumerate()
            .map(|(i, ty)| LuaReturn {
                ty: map_ty_to_lua(tcx, ty),
                name: names.get(i).and_then(|name| name.clone()),
            })
            .collect();
    }

    vec![infer_value_expr_lua_type(tcx, receiver).into()]
}

fn is_result_like_expr<'tcx>(tcx: TyCtxt<'tcx>, expr: &'tcx hir::Expr<'tcx>) -> bool {
    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    let ty = typeck.expr_ty(expr);

    matches!(ty.kind(), ty::TyKind::Adt(adt_def, _) if {
        let path = tcx.def_path_str(adt_def.did());
        path == "std::result::Result" || path == "core::result::Result"
    })
}

fn merge_return_branches(branches: Vec<Vec<LuaReturn>>) -> Option<Vec<LuaReturn>> {
    let max_len = branches.iter().map(Vec::len).max()?;
    let mut merged = Vec::with_capacity(max_len);

    for index in 0..max_len {
        let mut tys = Vec::new();
        let mut name: Option<String> = None;
        let mut same_name = true;

        for branch in &branches {
            let item = branch.get(index);
            let ty = item.map(|ret| ret.ty.clone()).unwrap_or(LuaType::Nil);
            tys.push(ty);

            let branch_name = item.and_then(|ret| ret.name.clone());
            if name.is_none() {
                name = branch_name.clone();
            } else if name != branch_name {
                same_name = false;
            }
        }

        merged.push(LuaReturn {
            ty: make_union(tys),
            name: if same_name { name } else { None },
        });
    }

    Some(merged)
}

/// Try to extract return-value names from the body's return tuple expression.
/// Works on `Ok((a.x, a.y, z))` and bare `(a.x, a.y, z)` patterns.
/// Enriches existing typed returns with names when the tuple element count matches.
fn enrich_return_names<'tcx>(
    _tcx: TyCtxt<'tcx>,
    body: &'tcx hir::Expr<'tcx>,
    returns: &mut [LuaReturn],
) {
    if returns.len() < 2 || returns.iter().any(|r| r.name.is_some()) {
        return;
    }
    if let Some(names) = find_return_tuple_names(body)
        && names.len() == returns.len()
    {
        for (ret, name) in returns.iter_mut().zip(names) {
            ret.name = name;
        }
    }
}

/// Walk the body to find the return tuple expression and extract element names.
fn find_return_tuple_names<'tcx>(expr: &'tcx hir::Expr<'tcx>) -> Option<Vec<Option<String>>> {
    match &expr.kind {
        // Ok((a, b, c)) — unwrap the Ok() call
        hir::ExprKind::Call(callee, args) => {
            if let hir::ExprKind::Path(qpath) = &callee.kind {
                let name = match qpath {
                    hir::QPath::Resolved(_, path) => {
                        path.segments.last().map(|s| s.ident.name.as_str())
                    }
                    hir::QPath::TypeRelative(_, seg) => Some(seg.ident.name.as_str()),
                };
                if matches!(name, Some("Ok" | "Some")) && args.len() == 1 {
                    return find_return_tuple_names(&args[0]);
                }
            }
            None
        }
        // (a, b, c) tuple literal
        hir::ExprKind::Tup(elements) => {
            Some(elements.iter().map(|e| extract_return_name(e)).collect())
        }
        // Block — check tail expression
        hir::ExprKind::Block(block, _) => {
            if let Some(tail) = block.expr {
                return find_return_tuple_names(tail);
            }
            None
        }
        // Match (try desugar `?`) — look through
        hir::ExprKind::Match(scrut, _, hir::MatchSource::TryDesugar(_)) => {
            find_return_tuple_names(scrut)
        }
        // Regular match — check first arm
        hir::ExprKind::Match(_, arms, _) => {
            for arm in *arms {
                if let Some(names) = find_return_tuple_names(arm.body) {
                    return Some(names);
                }
            }
            None
        }
        _ => None,
    }
}

/// Extract a name from a tuple element expression for return value naming.
/// Handles field accesses (`self.x` → "x"), variable references (`pos` → "pos"),
/// and method calls (`self.get_x()` → "x").
fn extract_return_name(expr: &hir::Expr<'_>) -> Option<String> {
    match &expr.kind {
        // self.field or obj.field → "field"
        hir::ExprKind::Field(_, ident) => {
            let name = ident.name.as_str();
            // Skip underscore-prefixed names
            if name.starts_with('_') {
                None
            } else {
                Some(name.to_string())
            }
        }
        // Variable reference → use variable name
        hir::ExprKind::Path(hir::QPath::Resolved(_, path)) => {
            if let Some(seg) = path.segments.last() {
                let name = seg.ident.name.as_str();
                if name.starts_with('_')
                    || name == "self"
                    || name.chars().next().is_some_and(|c| c.is_uppercase())
                {
                    None
                } else {
                    Some(name.to_string())
                }
            } else {
                None
            }
        }
        // method_call(...)? → unwrap try and recurse
        hir::ExprKind::Match(scrut, _, hir::MatchSource::TryDesugar(_)) => {
            extract_return_name(scrut)
        }
        // method().into_lua() or similar chains → look at receiver
        hir::ExprKind::MethodCall(seg, receiver, _, _) => {
            let method = seg.ident.name.as_str();
            // Skip conversion methods, look through to receiver
            if matches!(
                method,
                "into_lua"
                    | "into_lua_multi"
                    | "clone"
                    | "to_owned"
                    | "to_string"
                    | "into"
                    | "as_ref"
                    | "borrow"
            ) {
                return extract_return_name(receiver);
            }
            // get_x() → "x", x() → "x"
            let name = method.strip_prefix("get_").unwrap_or(method);
            if name.starts_with('_') {
                None
            } else {
                Some(name.to_string())
            }
        }
        // Function call: look at callee for associated function name
        hir::ExprKind::Call(callee, _) => {
            if let hir::ExprKind::Path(qpath) = &callee.kind {
                match qpath {
                    hir::QPath::TypeRelative(_, seg) => {
                        let name = seg.ident.name.as_str();
                        if name == "new"
                            || name.starts_with('_')
                            || name.chars().next().is_some_and(|c| c.is_uppercase())
                        {
                            None
                        } else {
                            Some(name.to_string())
                        }
                    }
                    hir::QPath::Resolved(_, path) => path.segments.last().and_then(|s| {
                        let name = s.ident.name.as_str();
                        (!name.chars().next().is_some_and(|c| c.is_uppercase()))
                            .then(|| name.to_string())
                    }),
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Extract names from a tuple expression's elements.
fn extract_tuple_element_names<'tcx>(receiver: &'tcx hir::Expr<'tcx>) -> Vec<Option<String>> {
    match &receiver.kind {
        hir::ExprKind::Tup(elements) => elements.iter().map(|e| extract_return_name(e)).collect(),
        // Look through blocks to find the tuple
        hir::ExprKind::Block(block, _) => {
            if let Some(tail) = block.expr {
                return extract_tuple_element_names(tail);
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// Search an expression tree for `.into_lua_multi()` calls on tuple receivers.
/// Returns the decomposed tuple elements as multiple Lua return values with names.
fn find_into_lua_multi_tuple<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<Vec<LuaReturn>> {
    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, _, _) => {
            if segment.ident.name.as_str() == "into_lua_multi" {
                let typeck = tcx.typeck(receiver.hir_id.owner.def_id);
                let receiver_ty = typeck.expr_ty(receiver);
                if let ty::TyKind::Tuple(fields) = receiver_ty.kind()
                    && !fields.is_empty()
                {
                    let names = extract_tuple_element_names(receiver);
                    let returns: Vec<LuaReturn> = fields
                        .iter()
                        .enumerate()
                        .map(|(i, t)| {
                            let ty = map_ty_to_lua(tcx, t);
                            let name = names.get(i).and_then(|n| n.clone());
                            LuaReturn { ty, name }
                        })
                        .collect();
                    return Some(returns);
                }
            }
            // Recurse
            if let Some(r) = find_into_lua_multi_tuple(tcx, receiver) {
                return Some(r);
            }
            None
        }
        hir::ExprKind::Block(block, _) => {
            if let Some(tail) = block.expr
                && let Some(r) = find_into_lua_multi_tuple(tcx, tail)
            {
                return Some(r);
            }
            for stmt in block.stmts {
                if let hir::StmtKind::Semi(e) | hir::StmtKind::Expr(e) = &stmt.kind
                    && let Some(r) = find_into_lua_multi_tuple(tcx, e)
                {
                    return Some(r);
                }
            }
            None
        }
        hir::ExprKind::Match(scrut, arms, _) => {
            if let Some(r) = find_into_lua_multi_tuple(tcx, scrut) {
                return Some(r);
            }
            for arm in *arms {
                if let Some(r) = find_into_lua_multi_tuple(tcx, arm.body) {
                    return Some(r);
                }
            }
            None
        }
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            find_into_lua_multi_tuple(tcx, body.value)
        }
        hir::ExprKind::Call(callee, args) => {
            if let Some(r) = find_into_lua_multi_tuple(tcx, callee) {
                return Some(r);
            }
            for arg in *args {
                if let Some(r) = find_into_lua_multi_tuple(tcx, arg) {
                    return Some(r);
                }
            }
            None
        }
        hir::ExprKind::If(_, then_expr, else_expr) => {
            if let Some(r) = find_into_lua_multi_tuple(tcx, then_expr) {
                return Some(r);
            }
            if let Some(e) = else_expr {
                find_into_lua_multi_tuple(tcx, e)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Recursively search an expression tree for `.into_lua()` calls and return
/// the concrete type of the receiver. Crosses closure boundaries to handle
/// patterns like `borrow_mut_scoped(|me| { ... value.into_lua(lua) })`.
fn find_into_lua_receiver_type<'tcx>(
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
            for arg in *args {
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
            for arm in *arms {
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
            for arg in *args {
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

fn infer_from_expr<'tcx>(
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
            for arm in *arms {
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
                    if !args.is_empty()
                        && let Some(name) = extract_string_literal(&args[0])
                    {
                        if let Some(existing) = fields.iter_mut().find(|f| f.name == name) {
                            existing.writable = true;
                        } else {
                            fields.push(writable_field(name, LuaType::Any));
                        }
                    }
                }
                // Static field: add_field("name", value)
                // We can't easily infer the type from a value expression,
                // but we can try to get it from typeck
                "add_field" | "add_meta_field" => {
                    if !args.is_empty()
                        && let Some(name) = extract_string_literal(&args[0])
                    {
                        let ty = if args.len() >= 2 {
                            infer_expr_lua_type(tcx, &args[1])
                        } else {
                            LuaType::Any
                        };
                        fields.push(readonly_field(name, ty));
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
        _ => {
            visit_recursive_expr_children(expr, |child| {
                visit_expr_for_fields(tcx, child, fields);
            });
        }
    }
}

fn extract_field_getter<'tcx>(
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

    let ret_ty = sig.output();
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
