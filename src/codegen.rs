use crate::{
    CodegenTarget, LuaApi, LuaClass, LuaEnum, LuaFunction, LuaModule, LuaType, MethodKind,
};
use std::fmt::Write;
use std::path::Path;

fn should_trace_codegen_class(name: &str) -> bool {
    matches!(name, "Access" | "Path" | "Url" | "Style" | "File" | "Tab")
}

fn should_trace_codegen_global(name: &str) -> bool {
    matches!(name, "Url" | "Path" | "File")
}

/// Generates LuaCATS stub content for a complete API.
pub fn generate_stubs(api: &LuaApi) -> String {
    generate_stubs_for(api, CodegenTarget::default())
}

/// Generates stub content for a complete API targeting a specific language server.
pub fn generate_stubs_for(api: &LuaApi, target: CodegenTarget) -> String {
    let mut out = String::new();
    writeln!(out, "---@meta").unwrap();

    for e in &api.enums {
        writeln!(out).unwrap();
        write_enum(&mut out, e, target);
    }

    for class in &api.classes {
        writeln!(out).unwrap();
        write_class(&mut out, class, target);
    }

    for module in &api.modules {
        writeln!(out).unwrap();
        write_module(&mut out, module, target);
    }

    for field in &api.global_fields {
        writeln!(out).unwrap();
        write_global_field(&mut out, field);
    }

    for func in &api.global_functions {
        writeln!(out).unwrap();
        write_global_function(&mut out, func, target);
    }

    out
}

/// Write doc comment lines as `--- text`.
fn write_doc(out: &mut String, doc: &Option<String>) {
    if let Some(doc) = doc {
        for line in doc.lines() {
            if line.is_empty() {
                writeln!(out, "---").unwrap();
            } else {
                writeln!(out, "--- {line}").unwrap();
            }
        }
    }
}

fn write_enum(out: &mut String, e: &LuaEnum, _target: CodegenTarget) {
    write_doc(out, &e.doc);
    let variants = e
        .variants
        .iter()
        .map(|v| format!("\"{v}\""))
        .collect::<Vec<_>>()
        .join(" | ");
    writeln!(out, "---@alias {} {variants}", e.name).unwrap();

    // If we have PascalCase variants, emit an enum constructor class
    // so that `Enum<T>.__index` resolves to real fields for autocomplete.
    if !e.pascal_variants.is_empty() {
        writeln!(out).unwrap();
        writeln!(out, "---@class {}Actions", e.name).unwrap();
        for variant in &e.pascal_variants {
            writeln!(out, "---@field {variant} {} | EnumVariant", e.name).unwrap();
        }
    }
}

fn write_class(out: &mut String, class: &LuaClass, target: CodegenTarget) {
    write_doc(out, &class.doc);
    let class_decl = class.name.as_str();
    let class_value = class_decl.split('<').next().unwrap_or(class_decl);
    match target {
        CodegenTarget::LuaLS => {
            writeln!(out, "---@class {class_decl}").unwrap();
        }
        CodegenTarget::EmmyLua => {
            writeln!(out, "---@class (exact) {class_decl}").unwrap();
        }
    }
    for field in &class.fields {
        write_field(out, field);
    }
    writeln!(out, "local {class_value} = {{}}").unwrap();

    // Detect overloaded methods (same name, multiple definitions)
    let mut method_counts = std::collections::HashMap::new();
    for method in &class.methods {
        *method_counts.entry(&method.name).or_insert(0usize) += 1;
    }

    let mut written_overloads = std::collections::HashSet::new();
    for method in &class.methods {
        if method_counts.get(&method.name).copied().unwrap_or(0) > 1 {
            // Write as @overload — only on first occurrence
            if written_overloads.insert(&method.name) {
                writeln!(out).unwrap();
                write_overloaded_method(out, class_value, &class.methods, &method.name, target);
            }
        } else {
            writeln!(out).unwrap();
            write_method(out, class_value, method, target);
        }
    }
}

/// Lua reserved keywords that must be quoted in field names.
const LUA_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Format a field name, quoting it if it's a Lua keyword or not a valid identifier.
fn format_field_name(name: &str) -> String {
    if LUA_KEYWORDS.contains(&name) || !is_lua_ident(name) {
        format!("[\"{name}\"]")
    } else {
        name.to_string()
    }
}

/// Check if a string is a valid Lua identifier (non-empty, starts with letter/_, rest alphanumeric/_).
fn is_lua_ident(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn write_field(out: &mut String, field: &crate::LuaField) {
    let name = format_field_name(&field.name);
    let readonly = if field.writable { "" } else { " (readonly)" };
    let ty = resolve_enum_constructor_type(&field.ty);
    if let Some(doc) = &field.doc {
        let first_line = doc.lines().next().unwrap_or("");
        writeln!(out, "---@field {name} {ty}{} {first_line}", readonly).unwrap();
    } else {
        writeln!(out, "---@field {name} {ty}{}", readonly).unwrap();
    }
}

/// If a type is `Enum<T>`, resolve it to `TActions` for enum constructor autocomplete.
fn resolve_enum_constructor_type(ty: &LuaType) -> String {
    let s = ty.to_string();
    if let Some(inner) = s.strip_prefix("Enum<").and_then(|s| s.strip_suffix('>')) {
        format!("{inner}Actions")
    } else {
        s
    }
}

fn write_overloaded_method(
    out: &mut String,
    class_name: &str,
    all_methods: &[crate::LuaMethod],
    name: &str,
    _target: CodegenTarget,
) {
    let overloads: Vec<_> = all_methods.iter().filter(|m| m.name == name).collect();

    // Write doc from first overload
    if let Some(first) = overloads.first() {
        write_doc(out, &first.doc);
    }

    // Write @overload annotations for each variant
    for method in &overloads {
        let params_sig = method
            .params
            .iter()
            .filter(|p| p.ty != LuaType::Nil)
            .map(|p| {
                if let LuaType::Variadic(inner) = &p.ty {
                    format!("...: {inner}")
                } else {
                    format!("{}: {}", p.name, p.ty)
                }
            })
            .collect::<Vec<_>>()
            .join(", ");

        let ret_sig = if method.returns.is_empty() {
            String::new()
        } else {
            let rets = method
                .returns
                .iter()
                .map(|r| r.ty.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!(": {rets}")
        };

        writeln!(out, "---@overload fun({params_sig}){ret_sig}").unwrap();
    }

    // Write the stub with the first overload's params
    let first = &overloads[0];
    let params_str = params_to_str(&first.params);
    let sep = match first.kind {
        MethodKind::Method => ":",
        MethodKind::Function => ".",
    };
    let escaped = format_method_name(name, sep);
    writeln!(out, "function {class_name}{escaped}({params_str}) end").unwrap();
}

fn write_method(
    out: &mut String,
    class_name: &str,
    method: &crate::LuaMethod,
    _target: CodegenTarget,
) {
    write_doc(out, &method.doc);
    if method.is_async {
        writeln!(out, "---@async").unwrap();
    }

    write_params(out, &method.params);
    write_returns(out, &method.returns);

    let params_str = params_to_str(&method.params);

    let sep = match method.kind {
        MethodKind::Method => ":",
        MethodKind::Function => ".",
    };

    let escaped = format_method_name(&method.name, sep);
    writeln!(out, "function {class_name}{escaped}({params_str}) end").unwrap();
}

/// Format a method/function name, quoting Lua keywords as `["name"]`.
fn format_method_name(name: &str, sep: &str) -> String {
    if LUA_KEYWORDS.contains(&name) || !is_lua_ident(name) {
        format!("[\"{name}\"]")
    } else {
        format!("{sep}{name}")
    }
}

fn write_module(out: &mut String, module: &LuaModule, target: CodegenTarget) {
    write_doc(out, &module.doc);
    writeln!(out, "---@class {}", module.name).unwrap();
    for field in &module.fields {
        write_field(out, field);
    }
    writeln!(out, "{} = {{}}", module.name).unwrap();

    // Detect overloaded functions (same name, multiple definitions)
    let mut func_counts = std::collections::HashMap::new();
    for func in &module.functions {
        *func_counts.entry(&func.name).or_insert(0usize) += 1;
    }

    let mut written_overloads = std::collections::HashSet::new();
    for func in &module.functions {
        if func_counts.get(&func.name).copied().unwrap_or(0) > 1 {
            if written_overloads.insert(&func.name) {
                writeln!(out).unwrap();
                write_overloaded_function(out, &module.name, &module.functions, &func.name, target);
            }
        } else {
            writeln!(out).unwrap();
            write_namespaced_function(out, &module.name, func, target);
        }
    }
}

fn write_overloaded_function(
    out: &mut String,
    namespace: &str,
    all_functions: &[LuaFunction],
    name: &str,
    _target: CodegenTarget,
) {
    let overloads: Vec<_> = all_functions.iter().filter(|f| f.name == name).collect();

    if let Some(first) = overloads.first() {
        write_doc(out, &first.doc);
    }

    for func in &overloads {
        let params_sig = func
            .params
            .iter()
            .filter(|p| p.ty != LuaType::Nil)
            .map(|p| {
                if let LuaType::Variadic(inner) = &p.ty {
                    format!("...: {inner}")
                } else {
                    format!("{}: {}", p.name, p.ty)
                }
            })
            .collect::<Vec<_>>()
            .join(", ");

        let ret_sig = if func.returns.is_empty() {
            String::new()
        } else {
            let rets = func
                .returns
                .iter()
                .map(|r| r.ty.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!(": {rets}")
        };

        writeln!(out, "---@overload fun({params_sig}){ret_sig}").unwrap();
    }

    let first = &overloads[0];
    let params_str = params_to_str(&first.params);
    let escaped = format_method_name(name, ".");
    writeln!(out, "function {namespace}{escaped}({params_str}) end").unwrap();
}

fn write_namespaced_function(
    out: &mut String,
    namespace: &str,
    func: &LuaFunction,
    _target: CodegenTarget,
) {
    write_doc(out, &func.doc);
    if func.is_async {
        writeln!(out, "---@async").unwrap();
    }
    write_params(out, &func.params);
    write_returns(out, &func.returns);
    let params_str = params_to_str(&func.params);
    let escaped = format_method_name(&func.name, ".");
    writeln!(out, "function {namespace}{escaped}({params_str}) end").unwrap();
}

fn write_global_function(out: &mut String, func: &LuaFunction, _target: CodegenTarget) {
    write_doc(out, &func.doc);
    if func.is_async {
        writeln!(out, "---@async").unwrap();
    }
    write_params(out, &func.params);
    write_returns(out, &func.returns);
    let params_str = params_to_str(&func.params);
    // Note: global function names can't really be escaped in Lua syntax.
    // If someone registers a keyword as a global, it'll produce invalid Lua,
    // but that's inherently broken usage. We emit it as-is.
    writeln!(out, "function {}({params_str}) end", func.name).unwrap();
}

fn write_global_field(out: &mut String, field: &crate::LuaField) {
    write_doc(out, &field.doc);
    writeln!(out, "---@type {}", field.ty).unwrap();
    writeln!(out, "{} = {}", field.name, field.name).unwrap();
}

/// Write `---@param` annotations, with special handling for variadic params.
fn write_params(out: &mut String, params: &[crate::LuaParam]) {
    for param in params {
        // Skip unit/nil params — they represent () in Rust and have no Lua equivalent
        if param.ty == LuaType::Nil {
            continue;
        }
        if let LuaType::Variadic(inner) = &param.ty {
            writeln!(out, "---@param ... {inner}").unwrap();
        } else {
            writeln!(out, "---@param {} {}", param.name, param.ty).unwrap();
        }
    }
}

/// Write `---@return` annotations for multiple return values.
fn write_returns(out: &mut String, returns: &[crate::LuaReturn]) {
    for ret in returns {
        // Skip error types — in Lua, errors are thrown (not returned)
        if is_error_return(&ret.ty) {
            continue;
        }
        if let Some(name) = &ret.name {
            writeln!(out, "---@return {} {name}", ret.ty).unwrap();
        } else {
            writeln!(out, "---@return {}", ret.ty).unwrap();
        }
    }
}

/// Returns true if a type represents an error that shouldn't be a return annotation.
fn is_error_return(ty: &LuaType) -> bool {
    matches!(ty, LuaType::Class(name) if name == "Error" || name == "LuaError"
        || name.ends_with("::Error") || name.ends_with("::LuaError"))
}

/// Convert params to the function signature string, replacing variadic params with `...`.
fn params_to_str(params: &[crate::LuaParam]) -> String {
    params
        .iter()
        .filter(|p| p.ty != LuaType::Nil)
        .map(|p| {
            if matches!(p.ty, LuaType::Variadic(_)) {
                "..."
            } else {
                p.name.as_str()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Writes stub files to the output directory with default settings.
pub fn write_stubs(output_dir: &Path, api: &LuaApi, crate_name: &str) -> std::io::Result<()> {
    write_stubs_for(output_dir, api, CodegenTarget::default(), crate_name)
}

/// Writes stub files to the output directory for a specific language server target.
///
/// Modules are written to per-module files (e.g., `wezterm.lua`) so that functions from
/// multiple crates sharing the same module are merged into a single file. Classes, enums,
/// and globals from each crate go into `{crate_name}.lua`.
pub fn write_stubs_for(
    output_dir: &Path,
    api: &LuaApi,
    target: CodegenTarget,
    crate_name: &str,
) -> std::io::Result<()> {
    std::fs::create_dir_all(output_dir)?;

    if std::env::var("MLUA_TYPEGEN_TRACE").is_ok() && crate_name == "yazi_binding" {
        for class in &api.classes {
            if should_trace_codegen_class(&class.name) {
                eprintln!(
                    "[mlua-typegen] codegen class {} methods={:?}",
                    class.name,
                    class
                        .methods
                        .iter()
                        .map(|m| (&m.name, &m.params, &m.returns))
                        .collect::<Vec<_>>()
                );
            }
        }
        for func in &api.global_functions {
            if should_trace_codegen_global(&func.name) {
                eprintln!(
                    "[mlua-typegen] codegen global_function {} params={:?} returns={:?}",
                    func.name, func.params, func.returns
                );
            }
        }
    }

    // Write modules to per-module files so cross-crate modules merge naturally.
    for module in &api.modules {
        let module_file = output_dir.join(format!("{}.lua", module.name));
        let mut content = String::new();

        if module_file.exists() {
            // Append to existing module file — skip the header/class decl,
            // only add new functions and fields.
            content = std::fs::read_to_string(&module_file)?;
            // Strip trailing newlines to append cleanly
            let trimmed_len = content.trim_end().len();
            content.truncate(trimmed_len);
            content.push('\n');
            content.push('\n');

            // Collect existing function names to avoid duplicates
            let existing: std::collections::HashSet<String> = content
                .lines()
                .filter_map(|line| {
                    let line = line.trim();
                    // Match "function module.name(" or "function module:name("
                    line.strip_prefix("function ")
                        .and_then(|rest| rest.split('(').next())
                        .map(String::from)
                })
                .collect();

            // Add new fields that aren't already present
            for field in &module.fields {
                let field_marker = format!("---@field {} ", field.name);
                if !content.contains(&field_marker) {
                    // Insert field annotation before the table assignment line
                    // For simplicity, just write them at the append point
                    write_field(&mut content, field);
                }
            }

            // Only write functions not already in the file
            for func in &module.functions {
                let qualified = format!("{}.{}", module.name, func.name);
                let qualified_colon = format!("{}:{}", module.name, func.name);
                if !existing.contains(&qualified) && !existing.contains(&qualified_colon) {
                    write_namespaced_function(&mut content, &module.name, func, target);
                }
            }
        } else {
            // First time writing this module — generate full content
            writeln!(content, "---@meta").unwrap();
            content.push('\n');
            write_module(&mut content, module, target);
        }

        std::fs::write(&module_file, content)?;
    }

    // Write classes, enums, globals, and global functions to per-crate file
    let non_module_api = LuaApi {
        classes: api.classes.clone(),
        enums: api.enums.clone(),
        modules: vec![], // modules handled above
        global_fields: api.global_fields.clone(),
        global_functions: api.global_functions.clone(),
    };

    let non_module_content = generate_stubs_for(&non_module_api, target);
    // Only write the per-crate file if there's actual content beyond ---@meta
    if non_module_content.trim().len() > "---@meta".len() {
        let filename = format!("{crate_name}.lua");
        std::fs::write(output_dir.join(filename), non_module_content)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LuaField, LuaMethod, LuaParam};

    // ── LuaType Display tests ──────────────────────────────────────────

    #[test]
    fn display_nil() {
        assert_eq!(LuaType::Nil.to_string(), "nil");
    }

    #[test]
    fn display_boolean() {
        assert_eq!(LuaType::Boolean.to_string(), "boolean");
    }

    #[test]
    fn display_integer() {
        assert_eq!(LuaType::Integer.to_string(), "integer");
    }

    #[test]
    fn display_number() {
        assert_eq!(LuaType::Number.to_string(), "number");
    }

    #[test]
    fn display_string() {
        assert_eq!(LuaType::String.to_string(), "string");
    }

    #[test]
    fn display_table() {
        assert_eq!(LuaType::Table.to_string(), "table");
    }

    #[test]
    fn display_function() {
        assert_eq!(LuaType::Function.to_string(), "function");
    }

    #[test]
    fn display_any() {
        assert_eq!(LuaType::Any.to_string(), "any");
    }

    #[test]
    fn display_thread() {
        assert_eq!(LuaType::Thread.to_string(), "thread");
    }

    #[test]
    fn display_array() {
        assert_eq!(
            LuaType::Array(Box::new(LuaType::String)).to_string(),
            "string[]"
        );
    }

    #[test]
    fn display_nested_array() {
        assert_eq!(
            LuaType::Array(Box::new(LuaType::Array(Box::new(LuaType::Integer)))).to_string(),
            "integer[][]"
        );
    }

    #[test]
    fn display_optional() {
        assert_eq!(
            LuaType::Optional(Box::new(LuaType::String)).to_string(),
            "string?"
        );
    }

    #[test]
    fn display_optional_union() {
        assert_eq!(
            LuaType::Optional(Box::new(LuaType::Union(vec![
                LuaType::String,
                LuaType::Number
            ])))
            .to_string(),
            "(string | number)?"
        );
    }

    #[test]
    fn display_optional_array() {
        assert_eq!(
            LuaType::Optional(Box::new(LuaType::Array(Box::new(LuaType::Integer)))).to_string(),
            "integer[]?"
        );
    }

    #[test]
    fn display_map() {
        assert_eq!(
            LuaType::Map(Box::new(LuaType::String), Box::new(LuaType::Integer)).to_string(),
            "table<string, integer>"
        );
    }

    #[test]
    fn display_map_nested_value() {
        assert_eq!(
            LuaType::Map(
                Box::new(LuaType::String),
                Box::new(LuaType::Array(Box::new(LuaType::Boolean)))
            )
            .to_string(),
            "table<string, boolean[]>"
        );
    }

    #[test]
    fn display_class() {
        assert_eq!(LuaType::Class("MyClass".to_string()).to_string(), "MyClass");
    }

    #[test]
    fn display_string_literal_single() {
        assert_eq!(
            LuaType::StringLiteral(vec!["hello".to_string()]).to_string(),
            "\"hello\""
        );
    }

    #[test]
    fn display_string_literal_multiple() {
        assert_eq!(
            LuaType::StringLiteral(vec!["a".to_string(), "b".to_string(), "c".to_string()])
                .to_string(),
            "\"a\" | \"b\" | \"c\""
        );
    }

    #[test]
    fn display_variadic() {
        assert_eq!(
            LuaType::Variadic(Box::new(LuaType::String)).to_string(),
            "string..."
        );
    }

    #[test]
    fn display_variadic_any() {
        assert_eq!(
            LuaType::Variadic(Box::new(LuaType::Any)).to_string(),
            "any..."
        );
    }

    #[test]
    fn display_function_sig_no_args() {
        assert_eq!(
            LuaType::FunctionSig {
                params: vec![],
                returns: vec![]
            }
            .to_string(),
            "fun()"
        );
    }

    #[test]
    fn display_function_sig_with_params() {
        assert_eq!(
            LuaType::FunctionSig {
                params: vec![LuaType::Integer, LuaType::String],
                returns: vec![LuaType::Boolean],
            }
            .to_string(),
            "fun(p1: integer, p2: string): boolean"
        );
    }

    #[test]
    fn display_function_sig_multi_return() {
        assert_eq!(
            LuaType::FunctionSig {
                params: vec![LuaType::Any],
                returns: vec![LuaType::String, LuaType::Integer],
            }
            .to_string(),
            "fun(p1: any): string, integer"
        );
    }

    #[test]
    fn display_function_sig_no_return() {
        assert_eq!(
            LuaType::FunctionSig {
                params: vec![LuaType::String],
                returns: vec![],
            }
            .to_string(),
            "fun(p1: string)"
        );
    }

    // ── Codegen: class tests ───────────────────────────────────────────

    #[test]
    fn generates_class_with_fields_and_methods() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Buffer".to_string(),
                doc: None,
                fields: vec![LuaField {
                    name: "document_id".to_string(),
                    ty: LuaType::String,
                    writable: false,
                    doc: None,
                }],
                methods: vec![
                    LuaMethod {
                        name: "get_text".to_string(),
                        kind: MethodKind::Method,
                        is_async: false,
                        params: vec![],
                        returns: vec![LuaType::String.into()],
                        doc: None,
                    },
                    LuaMethod {
                        name: "get_range_text".to_string(),
                        kind: MethodKind::Method,
                        is_async: false,
                        params: vec![
                            LuaParam {
                                name: "start".to_string(),
                                ty: LuaType::Class("Position".to_string()),
                            },
                            LuaParam {
                                name: "stop".to_string(),
                                ty: LuaType::Class("Position".to_string()),
                            },
                        ],
                        returns: vec![LuaType::String.into()],
                        doc: None,
                    },
                    LuaMethod {
                        name: "diagnostics".to_string(),
                        kind: MethodKind::Method,
                        is_async: false,
                        params: vec![],
                        returns: vec![
                            LuaType::Array(Box::new(LuaType::Class("Diagnostic".to_string())))
                                .into(),
                        ],
                        doc: None,
                    },
                    LuaMethod {
                        name: "create".to_string(),
                        kind: MethodKind::Function,
                        is_async: false,
                        params: vec![LuaParam {
                            name: "path".to_string(),
                            ty: LuaType::String,
                        }],
                        returns: vec![LuaType::Class("Buffer".to_string()).into()],
                        doc: None,
                    },
                ],
            }],
            ..Default::default()
        };

        let output = generate_stubs(&api);
        let expected = "\
---@meta

---@class Buffer
---@field document_id string (readonly)
local Buffer = {}

---@return string
function Buffer:get_text() end

---@param start Position
---@param stop Position
---@return string
function Buffer:get_range_text(start, stop) end

---@return Diagnostic[]
function Buffer:diagnostics() end

---@param path string
---@return Buffer
function Buffer.create(path) end
";
        assert_eq!(output, expected);
    }

    #[test]
    fn generates_writable_field() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Config".to_string(),
                doc: None,
                fields: vec![LuaField {
                    name: "debug".to_string(),
                    ty: LuaType::Boolean,
                    writable: true,
                    doc: None,
                }],
                methods: vec![],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@field debug boolean\n"));
        assert!(!output.contains("(readonly)"));
    }

    #[test]
    fn generates_class_no_methods() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Point".to_string(),
                doc: None,
                fields: vec![
                    LuaField {
                        name: "x".to_string(),
                        ty: LuaType::Number,
                        writable: false,
                        doc: None,
                    },
                    LuaField {
                        name: "y".to_string(),
                        ty: LuaType::Number,
                        writable: false,
                        doc: None,
                    },
                ],
                methods: vec![],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@class Point\n---@field x number (readonly)\n---@field y number (readonly)\nlocal Point = {}\n"));
    }

    // ── Codegen: enum tests ────────────────────────────────────────────

    #[test]
    fn generates_enum_alias() {
        let api = LuaApi {
            enums: vec![LuaEnum {
                name: "LogLevel".to_string(),
                doc: None,
                variants: vec![
                    "debug".to_string(),
                    "info".to_string(),
                    "warn".to_string(),
                    "error".to_string(),
                ],
                pascal_variants: vec![
                    "Debug".to_string(),
                    "Info".to_string(),
                    "Warn".to_string(),
                    "Error".to_string(),
                ],
            }],
            ..Default::default()
        };

        let output = generate_stubs(&api);
        assert!(output.contains("---@alias LogLevel \"debug\" | \"info\" | \"warn\" | \"error\""));
        assert!(output.contains("---@class LogLevelActions"));
        assert!(output.contains("---@field Debug LogLevel | EnumVariant"));
        assert!(output.contains("---@field Error LogLevel | EnumVariant"));
    }

    #[test]
    fn generates_single_variant_enum() {
        let api = LuaApi {
            enums: vec![LuaEnum {
                name: "OnlyOne".to_string(),
                doc: None,
                variants: vec!["sole".to_string()],
                pascal_variants: vec!["Sole".to_string()],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@alias OnlyOne \"sole\"\n"));
    }

    // ── Codegen: module tests ──────────────────────────────────────────

    #[test]
    fn generates_module_with_functions() {
        let api = LuaApi {
            modules: vec![LuaModule {
                name: "fs".to_string(),
                doc: None,
                fields: vec![],
                functions: vec![
                    LuaFunction {
                        name: "read_file".to_string(),
                        is_async: false,
                        params: vec![LuaParam {
                            name: "path".to_string(),
                            ty: LuaType::String,
                        }],
                        returns: vec![LuaType::String.into()],
                        doc: None,
                    },
                    LuaFunction {
                        name: "exists".to_string(),
                        is_async: false,
                        params: vec![LuaParam {
                            name: "path".to_string(),
                            ty: LuaType::String,
                        }],
                        returns: vec![LuaType::Boolean.into()],
                        doc: None,
                    },
                ],
            }],
            ..Default::default()
        };

        let output = generate_stubs(&api);
        let expected = "\
---@meta

---@class fs
fs = {}

---@param path string
---@return string
function fs.read_file(path) end

---@param path string
---@return boolean
function fs.exists(path) end
";
        assert_eq!(output, expected);
    }

    #[test]
    fn generates_module_with_fields() {
        let api = LuaApi {
            modules: vec![LuaModule {
                name: "constants".to_string(),
                doc: None,
                fields: vec![
                    LuaField {
                        name: "VERSION".to_string(),
                        ty: LuaType::String,
                        writable: false,
                        doc: None,
                    },
                    LuaField {
                        name: "MAX_SIZE".to_string(),
                        ty: LuaType::Integer,
                        writable: false,
                        doc: None,
                    },
                ],
                functions: vec![],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@field VERSION string (readonly)\n"));
        assert!(output.contains("---@field MAX_SIZE integer (readonly)\n"));
    }

    // ── Codegen: global function tests ─────────────────────────────────

    #[test]
    fn generates_global_functions() {
        let api = LuaApi {
            global_fields: vec![],
            global_functions: vec![LuaFunction {
                name: "print_colored".to_string(),
                is_async: false,
                params: vec![
                    LuaParam {
                        name: "msg".to_string(),
                        ty: LuaType::String,
                    },
                    LuaParam {
                        name: "color".to_string(),
                        ty: LuaType::String,
                    },
                ],
                returns: vec![],
                doc: None,
            }],
            ..Default::default()
        };

        let output = generate_stubs(&api);
        let expected = "\
---@meta

---@param msg string
---@param color string
function print_colored(msg, color) end
";
        assert_eq!(output, expected);
    }

    #[test]
    fn generates_global_function_no_params_no_return() {
        let api = LuaApi {
            global_fields: vec![],
            global_functions: vec![LuaFunction {
                name: "noop".to_string(),
                is_async: false,
                params: vec![],
                returns: vec![],
                doc: None,
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("function noop() end\n"));
    }

    #[test]
    fn generates_global_fields() {
        let api = LuaApi {
            global_fields: vec![crate::LuaField {
                name: "rt".to_string(),
                ty: LuaType::Table,
                writable: true,
                doc: None,
            }],
            ..Default::default()
        };

        let output = generate_stubs(&api);
        assert!(output.contains("---@type table\nrt = rt\n"));
    }

    // ── Codegen: async tests ───────────────────────────────────────────

    #[test]
    fn generates_async_method() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Http".to_string(),
                doc: None,
                fields: vec![],
                methods: vec![LuaMethod {
                    name: "fetch".to_string(),
                    kind: MethodKind::Method,
                    is_async: true,
                    params: vec![LuaParam {
                        name: "url".to_string(),
                        ty: LuaType::String,
                    }],
                    returns: vec![LuaType::String.into()],
                    doc: None,
                }],
            }],
            ..Default::default()
        };

        let output = generate_stubs(&api);
        assert!(output.contains(
            "---@async\n---@param url string\n---@return string\nfunction Http:fetch(url) end"
        ));
    }

    #[test]
    fn generates_async_global_function() {
        let api = LuaApi {
            global_fields: vec![],
            global_functions: vec![LuaFunction {
                name: "sleep".to_string(),
                is_async: true,
                params: vec![LuaParam {
                    name: "ms".to_string(),
                    ty: LuaType::Integer,
                }],
                returns: vec![],
                doc: None,
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@async\n---@param ms integer\nfunction sleep(ms) end"));
    }

    #[test]
    fn generates_async_module_function() {
        let api = LuaApi {
            modules: vec![LuaModule {
                name: "net".to_string(),
                doc: None,
                fields: vec![],
                functions: vec![LuaFunction {
                    name: "request".to_string(),
                    is_async: true,
                    params: vec![LuaParam {
                        name: "url".to_string(),
                        ty: LuaType::String,
                    }],
                    returns: vec![LuaType::Table.into()],
                    doc: None,
                }],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains(
            "---@async\n---@param url string\n---@return table\nfunction net.request(url) end"
        ));
    }

    // ── Codegen: multi-return tests ────────────────────────────────────

    #[test]
    fn generates_multi_return() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Parser".to_string(),
                doc: None,
                fields: vec![],
                methods: vec![LuaMethod {
                    name: "parse".to_string(),
                    kind: MethodKind::Method,
                    is_async: false,
                    params: vec![LuaParam {
                        name: "input".to_string(),
                        ty: LuaType::String,
                    }],
                    returns: vec![LuaType::Boolean.into(), LuaType::String.into()],
                    doc: None,
                }],
            }],
            ..Default::default()
        };

        let output = generate_stubs(&api);
        assert!(output.contains("---@return boolean\n---@return string\n"));
    }

    #[test]
    fn generates_triple_return() {
        let api = LuaApi {
            global_fields: vec![],
            global_functions: vec![LuaFunction {
                name: "get_rgb".to_string(),
                is_async: false,
                params: vec![],
                returns: vec![
                    LuaType::Integer.into(),
                    LuaType::Integer.into(),
                    LuaType::Integer.into(),
                ],
                doc: None,
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains(
            "---@return integer\n---@return integer\n---@return integer\nfunction get_rgb() end"
        ));
    }

    // ── Codegen: variadic param tests ──────────────────────────────────

    #[test]
    fn generates_variadic_param() {
        let api = LuaApi {
            global_fields: vec![],
            global_functions: vec![LuaFunction {
                name: "log".to_string(),
                is_async: false,
                params: vec![
                    LuaParam {
                        name: "level".to_string(),
                        ty: LuaType::String,
                    },
                    LuaParam {
                        name: "args".to_string(),
                        ty: LuaType::Variadic(Box::new(LuaType::Any)),
                    },
                ],
                returns: vec![],
                doc: None,
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(
            output.contains(
                "---@param level string\n---@param ... any\nfunction log(level, ...) end"
            )
        );
    }

    #[test]
    fn generates_variadic_only_param() {
        let api = LuaApi {
            global_fields: vec![],
            global_functions: vec![LuaFunction {
                name: "print".to_string(),
                is_async: false,
                params: vec![LuaParam {
                    name: "values".to_string(),
                    ty: LuaType::Variadic(Box::new(LuaType::Any)),
                }],
                returns: vec![],
                doc: None,
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@param ... any\nfunction print(...) end"));
    }

    #[test]
    fn generates_variadic_typed_param() {
        let api = LuaApi {
            global_fields: vec![],
            global_functions: vec![LuaFunction {
                name: "sum".to_string(),
                is_async: false,
                params: vec![LuaParam {
                    name: "numbers".to_string(),
                    ty: LuaType::Variadic(Box::new(LuaType::Number)),
                }],
                returns: vec![LuaType::Number.into()],
                doc: None,
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@param ... number\n---@return number\nfunction sum(...) end"));
    }

    // ── Codegen: complex type tests ────────────────────────────────────

    #[test]
    fn generates_optional_return() {
        let api = LuaApi {
            global_fields: vec![],
            global_functions: vec![LuaFunction {
                name: "find".to_string(),
                is_async: false,
                params: vec![LuaParam {
                    name: "key".to_string(),
                    ty: LuaType::String,
                }],
                returns: vec![LuaType::Optional(Box::new(LuaType::String)).into()],
                doc: None,
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@return string?\n"));
    }

    #[test]
    fn generates_map_param() {
        let api = LuaApi {
            global_fields: vec![],
            global_functions: vec![LuaFunction {
                name: "merge".to_string(),
                is_async: false,
                params: vec![LuaParam {
                    name: "data".to_string(),
                    ty: LuaType::Map(Box::new(LuaType::String), Box::new(LuaType::Any)),
                }],
                returns: vec![LuaType::Table.into()],
                doc: None,
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@param data table<string, any>\n"));
    }

    #[test]
    fn generates_array_of_class_return() {
        let api = LuaApi {
            global_fields: vec![],
            global_functions: vec![LuaFunction {
                name: "get_items".to_string(),
                is_async: false,
                params: vec![],
                returns: vec![LuaType::Array(Box::new(LuaType::Class("Item".to_string()))).into()],
                doc: None,
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@return Item[]\n"));
    }

    #[test]
    fn generates_thread_return() {
        let api = LuaApi {
            global_fields: vec![],
            global_functions: vec![LuaFunction {
                name: "spawn".to_string(),
                is_async: false,
                params: vec![LuaParam {
                    name: "func".to_string(),
                    ty: LuaType::Function,
                }],
                returns: vec![LuaType::Thread.into()],
                doc: None,
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(
            output.contains("---@param func function\n---@return thread\nfunction spawn(func) end")
        );
    }

    #[test]
    fn generates_nil_return() {
        let api = LuaApi {
            global_fields: vec![],
            global_functions: vec![LuaFunction {
                name: "maybe".to_string(),
                is_async: false,
                params: vec![],
                returns: vec![LuaType::Nil.into()],
                doc: None,
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@return nil\n"));
    }

    // ── Codegen: method kind tests ─────────────────────────────────────

    #[test]
    fn generates_static_method_with_dot() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Vec3".to_string(),
                doc: None,
                fields: vec![],
                methods: vec![LuaMethod {
                    name: "new".to_string(),
                    kind: MethodKind::Function,
                    is_async: false,
                    params: vec![
                        LuaParam {
                            name: "x".to_string(),
                            ty: LuaType::Number,
                        },
                        LuaParam {
                            name: "y".to_string(),
                            ty: LuaType::Number,
                        },
                        LuaParam {
                            name: "z".to_string(),
                            ty: LuaType::Number,
                        },
                    ],
                    returns: vec![LuaType::Class("Vec3".to_string()).into()],
                    doc: None,
                }],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("function Vec3.new(x, y, z) end"));
    }

    #[test]
    fn generates_instance_method_with_colon() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Vec3".to_string(),
                doc: None,
                fields: vec![],
                methods: vec![LuaMethod {
                    name: "length".to_string(),
                    kind: MethodKind::Method,
                    is_async: false,
                    params: vec![],
                    returns: vec![LuaType::Number.into()],
                    doc: None,
                }],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("function Vec3:length() end"));
    }

    // ── Codegen: EmmyLua target tests ──────────────────────────────────

    #[test]
    fn emmylua_class_uses_exact() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Foo".to_string(),
                doc: None,
                fields: vec![],
                methods: vec![],
            }],
            ..Default::default()
        };
        let output = generate_stubs_for(&api, CodegenTarget::EmmyLua);
        assert!(output.contains("---@class (exact) Foo\n"));
    }

    #[test]
    fn luals_class_no_exact() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Foo".to_string(),
                doc: None,
                fields: vec![],
                methods: vec![],
            }],
            ..Default::default()
        };
        let output = generate_stubs_for(&api, CodegenTarget::LuaLS);
        assert!(output.contains("---@class Foo\n"));
        assert!(!output.contains("(exact)"));
    }

    // ── Codegen: combined API tests ────────────────────────────────────

    #[test]
    fn generates_full_api_with_all_sections() {
        let api = LuaApi {
            enums: vec![LuaEnum {
                name: "Color".to_string(),
                doc: None,
                variants: vec!["red".to_string(), "green".to_string(), "blue".to_string()],
                pascal_variants: vec!["Red".to_string(), "Green".to_string(), "Blue".to_string()],
            }],
            classes: vec![LuaClass {
                name: "Canvas".to_string(),
                doc: None,
                fields: vec![LuaField {
                    name: "width".to_string(),
                    ty: LuaType::Integer,
                    writable: true,
                    doc: None,
                }],
                methods: vec![LuaMethod {
                    name: "draw".to_string(),
                    kind: MethodKind::Method,
                    is_async: false,
                    params: vec![],
                    returns: vec![],
                    doc: None,
                }],
            }],
            modules: vec![LuaModule {
                name: "gfx".to_string(),
                doc: None,
                fields: vec![],
                functions: vec![LuaFunction {
                    name: "init".to_string(),
                    is_async: false,
                    params: vec![],
                    returns: vec![LuaType::Boolean.into()],
                    doc: None,
                }],
            }],
            global_fields: vec![],
            global_functions: vec![LuaFunction {
                name: "quit".to_string(),
                is_async: false,
                params: vec![],
                returns: vec![],
                doc: None,
            }],
        };
        let output = generate_stubs(&api);
        let alias_pos = output.find("---@alias Color").unwrap();
        let class_pos = output.find("---@class Canvas").unwrap();
        let module_pos = output.find("---@class gfx").unwrap();
        let global_pos = output.find("function quit()").unwrap();
        assert!(alias_pos < class_pos);
        assert!(class_pos < module_pos);
        assert!(module_pos < global_pos);
    }

    #[test]
    fn generates_string_literal_field_type() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Event".to_string(),
                doc: None,
                fields: vec![LuaField {
                    name: "kind".to_string(),
                    ty: LuaType::StringLiteral(vec!["click".to_string(), "hover".to_string()]),
                    writable: false,
                    doc: None,
                }],
                methods: vec![],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@field kind \"click\" | \"hover\" (readonly)"));
    }

    #[test]
    fn empty_api_generates_only_meta() {
        let api = LuaApi::default();
        let output = generate_stubs(&api);
        assert_eq!(output, "---@meta\n");
    }

    // ── Codegen: doc comment tests ─────────────────────────────────────

    #[test]
    fn generates_class_doc() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Player".to_string(),
                doc: Some("Represents a player in the game.".to_string()),
                fields: vec![],
                methods: vec![],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("--- Represents a player in the game.\n---@class Player\n"));
    }

    #[test]
    fn generates_multiline_class_doc() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Player".to_string(),
                doc: Some("Represents a player.\n\nThis is the main entity.".to_string()),
                fields: vec![],
                methods: vec![],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains(
            "--- Represents a player.\n---\n--- This is the main entity.\n---@class Player\n"
        ));
    }

    #[test]
    fn generates_method_doc() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Player".to_string(),
                doc: None,
                fields: vec![],
                methods: vec![LuaMethod {
                    name: "move_to".to_string(),
                    kind: MethodKind::Method,
                    is_async: false,
                    params: vec![
                        LuaParam {
                            name: "x".to_string(),
                            ty: LuaType::Number,
                        },
                        LuaParam {
                            name: "y".to_string(),
                            ty: LuaType::Number,
                        },
                    ],
                    returns: vec![],
                    doc: Some("Move the player to the given position.".to_string()),
                }],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(
            output.contains("--- Move the player to the given position.\n---@param x number\n")
        );
    }

    #[test]
    fn generates_enum_doc() {
        let api = LuaApi {
            enums: vec![LuaEnum {
                name: "Direction".to_string(),
                doc: Some("Cardinal directions.".to_string()),
                variants: vec!["north".to_string(), "south".to_string()],
                pascal_variants: vec!["North".to_string(), "South".to_string()],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("--- Cardinal directions.\n---@alias Direction"));
    }

    #[test]
    fn generates_module_doc() {
        let api = LuaApi {
            modules: vec![LuaModule {
                name: "fs".to_string(),
                doc: Some("File system utilities.".to_string()),
                fields: vec![],
                functions: vec![],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("--- File system utilities.\n---@class fs\n"));
    }

    #[test]
    fn generates_global_function_doc() {
        let api = LuaApi {
            global_fields: vec![],
            global_functions: vec![LuaFunction {
                name: "greet".to_string(),
                is_async: false,
                params: vec![LuaParam {
                    name: "name".to_string(),
                    ty: LuaType::String,
                }],
                returns: vec![LuaType::String.into()],
                doc: Some("Greet someone by name.".to_string()),
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("--- Greet someone by name.\n---@param name string\n---@return string\nfunction greet(name) end"));
    }

    #[test]
    fn generates_field_doc_inline() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Config".to_string(),
                doc: None,
                fields: vec![LuaField {
                    name: "verbose".to_string(),
                    ty: LuaType::Boolean,
                    writable: true,
                    doc: Some("Enable verbose logging.".to_string()),
                }],
                methods: vec![],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@field verbose boolean Enable verbose logging.\n"));
    }

    // ── Codegen: overload tests ─────────────────────────────────────────

    #[test]
    fn generates_overloaded_methods() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Buffer".to_string(),
                doc: None,
                fields: vec![],
                methods: vec![
                    LuaMethod {
                        name: "insert".to_string(),
                        kind: MethodKind::Method,
                        is_async: false,
                        params: vec![LuaParam {
                            name: "text".to_string(),
                            ty: LuaType::String,
                        }],
                        returns: vec![],
                        doc: Some("Insert text at cursor.".to_string()),
                    },
                    LuaMethod {
                        name: "insert".to_string(),
                        kind: MethodKind::Method,
                        is_async: false,
                        params: vec![
                            LuaParam {
                                name: "pos".to_string(),
                                ty: LuaType::Integer,
                            },
                            LuaParam {
                                name: "text".to_string(),
                                ty: LuaType::String,
                            },
                        ],
                        returns: vec![],
                        doc: None,
                    },
                ],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@overload fun(text: string)"));
        assert!(output.contains("---@overload fun(pos: integer, text: string)"));
        assert!(output.contains("function Buffer:insert(text) end"));
    }

    #[test]
    fn generates_overloaded_with_returns() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Parser".to_string(),
                doc: None,
                fields: vec![],
                methods: vec![
                    LuaMethod {
                        name: "parse".to_string(),
                        kind: MethodKind::Method,
                        is_async: false,
                        params: vec![LuaParam {
                            name: "input".to_string(),
                            ty: LuaType::String,
                        }],
                        returns: vec![LuaType::Table.into()],
                        doc: None,
                    },
                    LuaMethod {
                        name: "parse".to_string(),
                        kind: MethodKind::Method,
                        is_async: false,
                        params: vec![
                            LuaParam {
                                name: "input".to_string(),
                                ty: LuaType::String,
                            },
                            LuaParam {
                                name: "opts".to_string(),
                                ty: LuaType::Table,
                            },
                        ],
                        returns: vec![LuaType::Table.into(), LuaType::Boolean.into()],
                        doc: None,
                    },
                ],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@overload fun(input: string): table"));
        assert!(output.contains("---@overload fun(input: string, opts: table): table, boolean"));
    }

    // ── Codegen: field keyword escaping tests ──────────────────────────

    #[test]
    fn escapes_lua_keyword_field_name() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Node".to_string(),
                doc: None,
                fields: vec![
                    LuaField {
                        name: "end".to_string(),
                        ty: LuaType::Integer,
                        writable: false,
                        doc: None,
                    },
                    LuaField {
                        name: "function".to_string(),
                        ty: LuaType::String,
                        writable: false,
                        doc: None,
                    },
                ],
                methods: vec![],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@field [\"end\"] integer (readonly)"));
        assert!(output.contains("---@field [\"function\"] string (readonly)"));
    }

    #[test]
    fn normal_field_not_escaped() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Foo".to_string(),
                doc: None,
                fields: vec![LuaField {
                    name: "name".to_string(),
                    ty: LuaType::String,
                    writable: false,
                    doc: None,
                }],
                methods: vec![],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@field name string (readonly)"));
    }

    #[test]
    fn escapes_non_ident_field_name() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Foo".to_string(),
                doc: None,
                fields: vec![LuaField {
                    name: "my-field".to_string(),
                    ty: LuaType::String,
                    writable: false,
                    doc: None,
                }],
                methods: vec![],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@field [\"my-field\"] string (readonly)"));
    }

    // ── Codegen: module overload tests ──────────────────────────────────

    #[test]
    fn generates_overloaded_module_functions() {
        let api = LuaApi {
            modules: vec![LuaModule {
                name: "fs".to_string(),
                doc: None,
                fields: vec![],
                functions: vec![
                    LuaFunction {
                        name: "read".to_string(),
                        is_async: false,
                        params: vec![LuaParam {
                            name: "path".to_string(),
                            ty: LuaType::String,
                        }],
                        returns: vec![LuaType::String.into()],
                        doc: None,
                    },
                    LuaFunction {
                        name: "read".to_string(),
                        is_async: false,
                        params: vec![
                            LuaParam {
                                name: "path".to_string(),
                                ty: LuaType::String,
                            },
                            LuaParam {
                                name: "encoding".to_string(),
                                ty: LuaType::String,
                            },
                        ],
                        returns: vec![LuaType::String.into()],
                        doc: None,
                    },
                ],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("---@overload fun(path: string): string"));
        assert!(output.contains("---@overload fun(path: string, encoding: string): string"));
        assert!(output.contains("function fs.read(path) end"));
    }

    #[test]
    fn no_extra_blank_lines_for_overloads() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "T".to_string(),
                doc: None,
                fields: vec![],
                methods: vec![
                    LuaMethod {
                        name: "f".to_string(),
                        kind: MethodKind::Method,
                        is_async: false,
                        params: vec![],
                        returns: vec![],
                        doc: None,
                    },
                    LuaMethod {
                        name: "f".to_string(),
                        kind: MethodKind::Method,
                        is_async: false,
                        params: vec![LuaParam {
                            name: "x".to_string(),
                            ty: LuaType::Integer,
                        }],
                        returns: vec![],
                        doc: None,
                    },
                    LuaMethod {
                        name: "g".to_string(),
                        kind: MethodKind::Method,
                        is_async: false,
                        params: vec![],
                        returns: vec![],
                        doc: None,
                    },
                ],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        // Should not have triple newlines from skipped overload entries
        assert!(!output.contains("\n\n\n"));
    }

    #[test]
    fn generates_doc_before_async() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Net".to_string(),
                doc: None,
                fields: vec![],
                methods: vec![LuaMethod {
                    name: "fetch".to_string(),
                    kind: MethodKind::Method,
                    is_async: true,
                    params: vec![LuaParam {
                        name: "url".to_string(),
                        ty: LuaType::String,
                    }],
                    returns: vec![LuaType::String.into()],
                    doc: Some("Fetch a URL.".to_string()),
                }],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(output.contains("--- Fetch a URL.\n---@async\n---@param url string\n"));
    }

    // ── Display: array of optional needs parens ────────────────────────

    #[test]
    fn display_array_of_optional() {
        // Array(Optional(String)) → "(string?)[]" not "string?[]"
        assert_eq!(
            LuaType::Array(Box::new(LuaType::Optional(Box::new(LuaType::String)))).to_string(),
            "(string?)[]"
        );
    }

    #[test]
    fn display_array_of_map() {
        // Array(Map(K,V)) → "(table<string, integer>)[]"
        assert_eq!(
            LuaType::Array(Box::new(LuaType::Map(
                Box::new(LuaType::String),
                Box::new(LuaType::Integer),
            )))
            .to_string(),
            "(table<string, integer>)[]"
        );
    }

    #[test]
    fn display_array_of_simple_no_parens() {
        // Simple inner types don't get parens
        assert_eq!(
            LuaType::Array(Box::new(LuaType::String)).to_string(),
            "string[]"
        );
        assert_eq!(
            LuaType::Array(Box::new(LuaType::Class("Player".to_string()))).to_string(),
            "Player[]"
        );
    }

    // ── Method name keyword escaping ──────────────────────────────────

    #[test]
    fn escapes_lua_keyword_method_name() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Obj".to_string(),
                doc: None,
                fields: vec![],
                methods: vec![LuaMethod {
                    name: "end".to_string(),
                    kind: MethodKind::Method,
                    is_async: false,
                    params: vec![],
                    returns: vec![],
                    doc: None,
                }],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(
            output.contains("function Obj[\"end\"]() end"),
            "got: {output}"
        );
    }

    #[test]
    fn normal_method_name_not_escaped() {
        let api = LuaApi {
            classes: vec![LuaClass {
                name: "Obj".to_string(),
                doc: None,
                fields: vec![],
                methods: vec![LuaMethod {
                    name: "update".to_string(),
                    kind: MethodKind::Method,
                    is_async: false,
                    params: vec![],
                    returns: vec![],
                    doc: None,
                }],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(
            output.contains("function Obj:update() end"),
            "got: {output}"
        );
    }

    #[test]
    fn escapes_lua_keyword_module_function_name() {
        let api = LuaApi {
            modules: vec![LuaModule {
                name: "mymod".to_string(),
                doc: None,
                functions: vec![LuaFunction {
                    name: "repeat".to_string(),
                    is_async: false,
                    params: vec![],
                    returns: vec![],
                    doc: None,
                }],
                fields: vec![],
            }],
            ..Default::default()
        };
        let output = generate_stubs(&api);
        assert!(
            output.contains("function mymod[\"repeat\"]() end"),
            "got: {output}"
        );
    }
}
