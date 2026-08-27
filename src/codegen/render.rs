use std::fmt::Write as _;

use crate::{
    AliasDef, AnnotationId, ClassDef, FieldDef, LuaAccess, LuaLiteral, LuaName, LuaPath,
    LuaStringLiteral, MethodKind, OverloadSet, SemanticApi, Signature, TableDef, TypeDef, TypeExpr,
    ValueDef,
};

pub(super) fn document(api: &SemanticApi) -> String {
    let mut out = String::from("---@meta\n");
    for (id, definition) in api.types() {
        out.push('\n');
        match definition {
            TypeDef::Class(class) => write_class(&mut out, id.as_str(), class),
            TypeDef::Alias(alias) => write_alias(&mut out, id.as_str(), alias),
        }
    }
    for (path, value) in api.values() {
        out.push('\n');
        write_value(&mut out, path, value);
    }
    out
}

fn write_alias(out: &mut String, id: &str, alias: &AliasDef) {
    write_doc(out, &alias.doc);
    writeln!(out, "---@alias {id} {}", type_expr(&alias.target)).unwrap();
    if !alias.constructors.is_empty() {
        writeln!(out, "\n---@class (exact) {id}Actions").unwrap();
        for (name, ty) in &alias.constructors {
            writeln!(out, "---@field {} {}", field_name(name), type_expr(ty)).unwrap();
        }
    }
}

fn write_class(out: &mut String, id: &str, class: &ClassDef) {
    write_doc(out, &class.doc);
    writeln!(
        out,
        "---@class (exact) {id}{}",
        generic_declaration(&class.parameters)
    )
    .unwrap();
    for (name, field) in &class.fields {
        write_field(out, name, field);
    }
    let local = safe_local(id);
    writeln!(out, "local {local} = {{}}").unwrap();
    for (name, overloads) in &class.methods {
        out.push('\n');
        write_callable(out, CallableAccess::Member(&local, name), overloads);
    }
}

fn write_value(out: &mut String, path: &LuaPath, value: &ValueDef) {
    let access = LuaAccess::for_path(path);
    match value {
        ValueDef::Table(table) => write_table(out, path, &access, table),
        ValueDef::Function(overloads) => {
            write_callable(out, CallableAccess::Exact(&access), overloads)
        }
        ValueDef::Typed(field) => {
            write_doc(out, &field.doc);
            writeln!(out, "---@type {}", type_expr(&field.ty)).unwrap();
            writeln!(out, "{access} = {access}").unwrap();
        }
    }
}

fn write_table(out: &mut String, path: &LuaPath, access: &LuaAccess, table: &TableDef) {
    write_doc(out, &table.doc);
    let annotation = AnnotationId::for_path(path);
    writeln!(out, "---@class (exact) {annotation}").unwrap();
    for (name, field) in &table.fields {
        write_field(out, name, field);
    }
    writeln!(out, "---@type {annotation}").unwrap();
    writeln!(out, "{access} = {access} or {{}}").unwrap();
    for (name, overloads) in &table.functions {
        out.push('\n');
        write_callable(
            out,
            CallableAccess::Member(access.as_str(), name),
            overloads,
        );
    }
}

enum CallableAccess<'a> {
    Exact(&'a LuaAccess),
    Member(&'a str, &'a LuaName),
}

fn write_callable(out: &mut String, access: CallableAccess<'_>, overloads: &OverloadSet) {
    let primary = overloads.primary();
    write_docs(out, &primary.docs);
    for signature in overloads.iter().skip(1) {
        writeln!(out, "---@overload {}", function_type(signature)).unwrap();
    }
    if primary.is_async {
        writeln!(out, "---@async").unwrap();
    }
    for parameter in &primary.inputs {
        writeln!(
            out,
            "---@param {} {}",
            parameter.name,
            type_expr(&parameter.ty)
        )
        .unwrap();
    }
    if let Some(variadic) = &primary.variadic {
        writeln!(out, "---@param ... {}", type_expr(variadic)).unwrap();
    }
    for value in &primary.outputs {
        match &value.name {
            Some(name) => writeln!(out, "---@return {} {name}", type_expr(&value.ty)).unwrap(),
            None => writeln!(out, "---@return {}", type_expr(&value.ty)).unwrap(),
        }
    }
    let mut params = primary
        .inputs
        .iter()
        .map(|parameter| parameter.name.as_str())
        .collect::<Vec<_>>();
    if primary.variadic.is_some() {
        params.push("...");
    }
    let params = params.join(", ");
    match access {
        CallableAccess::Exact(access) => {
            writeln!(out, "{access} = function({params}) end").unwrap()
        }
        CallableAccess::Member(owner, name) => {
            let params = if primary.receiver == Some(MethodKind::Method) {
                if params.is_empty() {
                    "self".to_owned()
                } else {
                    format!("self, {params}")
                }
            } else {
                params
            };
            writeln!(
                out,
                "{owner}[{}] = function({params}) end",
                LuaStringLiteral::for_name(name)
            )
            .unwrap();
        }
    }
}

fn write_field(out: &mut String, name: &LuaName, field: &FieldDef) {
    let readonly = if field.writable { "" } else { " (readonly)" };
    let doc = field
        .doc
        .as_deref()
        .and_then(|doc| doc.lines().next())
        .map(|line| format!(" {line}"))
        .unwrap_or_default();
    writeln!(
        out,
        "---@field {} {}{readonly}{doc}",
        field_name(name),
        type_expr(&field.ty)
    )
    .unwrap();
}

fn write_doc(out: &mut String, doc: &Option<String>) {
    if let Some(doc) = doc {
        for line in doc.lines() {
            writeln!(
                out,
                "---{prefix}{line}",
                prefix = if line.is_empty() { "" } else { " " }
            )
            .unwrap();
        }
    }
}

fn write_docs(out: &mut String, docs: &[String]) {
    for doc in docs {
        write_doc(out, &Some(doc.clone()));
    }
}

pub(super) fn type_expr(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Nil => "nil".into(),
        TypeExpr::Boolean => "boolean".into(),
        TypeExpr::Integer => "integer".into(),
        TypeExpr::Number => "number".into(),
        TypeExpr::String => "string".into(),
        TypeExpr::Table => "table".into(),
        TypeExpr::Function(signature) => function_type(signature),
        TypeExpr::Any => "any".into(),
        TypeExpr::Dynamic { .. } => "any".into(),
        TypeExpr::Thread => "thread".into(),
        TypeExpr::Literal(literal) => literal_type(literal),
        TypeExpr::Named { id, arguments } if arguments.is_empty() => id.to_string(),
        TypeExpr::Named { id, arguments } => format!(
            "{}<{}>",
            id,
            arguments
                .iter()
                .map(type_expr)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeExpr::Parameter(id) => id.to_string(),
        TypeExpr::Array(item) => format!("{}[]", parenthesize(item)),
        TypeExpr::Map { key, value } => format!("table<{}, {}>", type_expr(key), type_expr(value)),
        TypeExpr::Record { fields, .. } => format!(
            "{{ {} }}",
            fields
                .iter()
                .map(|(name, ty)| format!("{}: {}", field_name(name), type_expr(ty)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeExpr::Union(items) => items.iter().map(type_expr).collect::<Vec<_>>().join(" | "),
    }
}

fn function_type(signature: &Signature) -> String {
    let mut params = signature
        .inputs
        .iter()
        .map(|parameter| format!("{}: {}", parameter.name, type_expr(&parameter.ty)))
        .collect::<Vec<_>>();
    if let Some(variadic) = &signature.variadic {
        params.push(format!("...: {}", type_expr(variadic)));
    }
    let returns = signature
        .outputs
        .iter()
        .map(|value| type_expr(&value.ty))
        .collect::<Vec<_>>();
    let suffix = if returns.is_empty() {
        String::new()
    } else {
        format!(": {}", returns.join(", "))
    };
    format!("fun({}){suffix}", params.join(", "))
}

fn literal_type(literal: &LuaLiteral) -> String {
    match literal {
        LuaLiteral::Boolean(value) => value.to_string(),
        LuaLiteral::Integer(value) => value.to_string(),
        LuaLiteral::Number(bits) => f64::from_bits(*bits).to_string(),
        LuaLiteral::String(value) => {
            LuaStringLiteral::for_name(&LuaName::from(value.clone())).to_string()
        }
    }
}

fn generic_declaration(parameters: &[crate::TypeParameter]) -> String {
    if parameters.is_empty() {
        String::new()
    } else {
        format!(
            "<{}>",
            parameters
                .iter()
                .map(|parameter| parameter.id.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn field_name(name: &LuaName) -> String {
    match std::str::from_utf8(name.as_bytes()) {
        Ok(text) if is_identifier(text) => text.to_owned(),
        _ => format!("[{}]", LuaStringLiteral::for_name(name)),
    }
}

fn is_identifier(text: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if",
        "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
    ];
    let mut chars = text.chars();
    !KEYWORDS.contains(&text)
        && chars
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn parenthesize(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Union(_) | TypeExpr::Map { .. } | TypeExpr::Function(_) => {
            format!("({})", type_expr(ty))
        }
        _ => type_expr(ty),
    }
}

fn safe_local(id: &str) -> String {
    let base = id.split('<').next().unwrap_or(id);
    if is_identifier(base) {
        base.to_owned()
    } else {
        "__type".to_owned()
    }
}
