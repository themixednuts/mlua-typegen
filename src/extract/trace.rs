use mlua_typegen::{LuaField, LuaFunction, LuaMethod};

fn trace_enabled() -> bool {
    std::env::var_os("MLUA_TYPEGEN_TRACE").is_some()
}

pub(super) fn trace(msg: impl AsRef<str>) {
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

pub(super) fn should_trace_method_name(name: &str) -> bool {
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
            | "__tostring"
            | "__pairs"
    )
}

pub(super) fn should_trace_field_name(name: &str) -> bool {
    matches!(name, "base" | "parent" | "raw" | "url" | "path")
}

pub(super) fn should_trace_field_expr_snippet(snippet: &str) -> bool {
    snippet.contains("me.base()")
        || snippet.contains("me.parent()")
        || snippet.contains("map(Self::new)")
        || snippet.contains("($value as fn")
}

fn should_trace_class_name(name: &str) -> bool {
    matches!(name, "Access" | "Path" | "Url" | "Style" | "File" | "Tab")
}

pub(super) fn method_trace_summary(method: &LuaMethod) -> String {
    format!(
        "{} {:?} params={:?} returns={:?}",
        method.name, method.kind, method.params, method.returns
    )
}

pub(super) fn field_trace_summary(field: &LuaField) -> String {
    format!(
        "{} ty={:?} writable={}",
        field.name, field.ty, field.writable
    )
}

pub(super) fn trace_class_snapshot(
    label: &str,
    class_name: &str,
    fields: &[LuaField],
    methods: &[LuaMethod],
) {
    if !should_trace_class_name(class_name) {
        return;
    }

    trace(format!(
        "{label} class={class_name} fields={:?} methods={:?}",
        fields.iter().map(field_trace_summary).collect::<Vec<_>>(),
        methods.iter().map(method_trace_summary).collect::<Vec<_>>()
    ));
}

pub(super) fn trace_global_function_snapshot(label: &str, func: &LuaFunction) {
    if !matches!(func.name.as_str(), "Url" | "Path" | "File") {
        return;
    }

    trace(format!(
        "{label} global_function={} params={:?} returns={:?}",
        func.name, func.params, func.returns
    ));
}
