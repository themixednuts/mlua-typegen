pub mod codegen;
pub mod config;
#[doc(hidden)]
pub mod driver_protocol;
pub mod generation;
pub mod identity;
pub mod semantic;
pub mod typemap;

pub use identity::{AnnotationId, LuaAccess, LuaName, LuaPath, LuaStringLiteral};
pub use semantic::{
    AliasDef, ApiContribution, ClassDef, ExtractedUnit, FieldDef, Origins, OverloadSet, Parameter,
    Provenance, ReturnValue, SemanticApi, SemanticError, SemanticErrors, Signature, TableDef,
    TypeDef, TypeExpr, TypeId, TypeParameter, TypeParameterId, ValueDef,
};

/// Stable `mlua` releases analyzed by this `mlua-typegen` minor line.
pub const SUPPORTED_MLUA_VERSIONS: &str = ">=0.9.0, <0.13.0";

use std::fmt;

/// Annotation dialect emitted from the target-neutral semantic API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dialect {
    /// LuaCATS annotations (used by LuaLS). This is the default.
    #[default]
    LuaLs,
    /// EmmyLua annotations (EmmyLua plugin for IntelliJ / VS Code).
    EmmyLua,
}

/// Represents a Lua type extracted from Rust code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LuaType {
    Nil,
    Boolean,
    Integer,
    Number,
    String,
    Table,
    Function,
    Any,
    Dynamic(String),
    Literal(LuaLiteral),
    /// A typed array: `T[]`
    Array(Box<LuaType>),
    /// An optional type: `T?`
    Optional(Box<LuaType>),
    /// A typed table: `table<K, V>`
    Map(Box<LuaType>, Box<LuaType>),
    /// An inline table shape proven from conversion code.
    Record {
        fields: Vec<LuaRecordField>,
        open: bool,
    },
    /// A reference to another UserData class or alias by name.
    Class(String),
    /// A resolved Rust definition whose Lua contract still needs native,
    /// extracted, or configured evidence.
    #[doc(hidden)]
    UnresolvedRust {
        path: String,
        arguments: Vec<LuaType>,
    },
    /// A definition owned by another compiled Rust crate whose source evidence
    /// may need to be replayed through the driver.
    #[doc(hidden)]
    ExternalRust {
        path: String,
        arguments: Vec<LuaType>,
        stable_crate_id: u64,
    },
    /// A statically known Rust type form that the extractor cannot prove.
    #[doc(hidden)]
    UnsupportedStatic {
        rust_type: String,
        reason: String,
    },
    /// A union of string literals: `"a" | "b" | "c"`
    StringLiteral(Vec<String>),
    /// A union of types: `T | U`
    Union(Vec<LuaType>),
    /// A Lua thread (coroutine).
    Thread,
    /// Variadic: `T...`
    Variadic(Box<LuaType>),
    /// A typed function signature: `fun(p1: T, p2: U): V`
    FunctionSig {
        params: Vec<LuaType>,
        returns: Vec<LuaType>,
    },
}

/// A scalar literal proven by conversion-body analysis.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum LuaLiteral {
    Boolean(bool),
    Integer(i64),
    /// IEEE-754 bits preserve equality without imposing an artificial ordering.
    Number(u64),
    String(String),
}

/// The openness of a table produced or accepted by a conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TableOpenness {
    Closed,
    Open,
}

/// A statically proven field in a Lua table conversion.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct LuaTableField {
    pub name: String,
    pub value: LuaValueDomain,
}

/// One field in an inline structural Lua table type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaRecordField {
    pub name: String,
    pub ty: LuaType,
}

/// A table shape derived from resolved table operations.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct LuaTableShape {
    pub fields: Vec<LuaTableField>,
    pub openness: TableOpenness,
}

/// The exact Lua value domain of one conversion direction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LuaValueDomain {
    Nil,
    Boolean,
    Integer,
    Number,
    String,
    Literal(LuaLiteral),
    Table(LuaTableShape),
    UserData(String),
    Function,
    Thread,
    Union(Vec<LuaValueDomain>),
}

/// The independently analyzed input and output contracts of a Rust type.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConversionContract {
    pub rust_path: String,
    pub rust_type: String,
    pub into_lua: Option<LuaValueDomain>,
    pub from_lua: Option<LuaValueDomain>,
}

impl fmt::Display for LuaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LuaType::Nil => write!(f, "nil"),
            LuaType::Boolean => write!(f, "boolean"),
            LuaType::Integer => write!(f, "integer"),
            LuaType::Number => write!(f, "number"),
            LuaType::String => write!(f, "string"),
            LuaType::Table => write!(f, "table"),
            LuaType::Function => write!(f, "function"),
            LuaType::Any => write!(f, "any"),
            LuaType::Dynamic(_) => write!(f, "any"),
            LuaType::Literal(value) => match value {
                LuaLiteral::Boolean(value) => write!(f, "{value}"),
                LuaLiteral::Integer(value) => write!(f, "{value}"),
                LuaLiteral::Number(bits) => write!(f, "{}", f64::from_bits(*bits)),
                LuaLiteral::String(value) => write!(f, "\"{value}\""),
            },
            LuaType::Array(inner) => {
                if matches!(inner.as_ref(), LuaType::Optional(_) | LuaType::Map(_, _)) {
                    write!(f, "({inner})[]")
                } else {
                    write!(f, "{inner}[]")
                }
            }
            LuaType::Optional(inner) => {
                if matches!(
                    inner.as_ref(),
                    LuaType::Union(_) | LuaType::StringLiteral(_) | LuaType::FunctionSig { .. }
                ) {
                    write!(f, "({inner})?")
                } else {
                    write!(f, "{inner}?")
                }
            }
            LuaType::Map(k, v) => write!(f, "table<{k}, {v}>"),
            LuaType::Record { fields, .. } => {
                f.write_str("{ ")?;
                for (index, field) in fields.iter().enumerate() {
                    if index != 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}: {}", field.name, field.ty)?;
                }
                f.write_str(" }")
            }
            LuaType::Class(name) => write!(f, "{}", format_embedded_class_name(name)),
            LuaType::UnresolvedRust { path, arguments } => {
                if arguments.is_empty() {
                    f.write_str(path)
                } else {
                    write!(
                        f,
                        "{}<{}>",
                        path,
                        arguments
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
            LuaType::ExternalRust {
                path, arguments, ..
            } => {
                if arguments.is_empty() {
                    f.write_str(path)
                } else {
                    write!(
                        f,
                        "{}<{}>",
                        path,
                        arguments
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
            LuaType::UnsupportedStatic { rust_type, .. } => f.write_str(rust_type),
            LuaType::StringLiteral(variants) => {
                for (i, v) in variants.iter().enumerate() {
                    if i > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "\"{v}\"")?;
                }
                Ok(())
            }
            LuaType::Union(types) => {
                for (i, t) in types.iter().enumerate() {
                    if i > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "{t}")?;
                }
                Ok(())
            }
            LuaType::Thread => write!(f, "thread"),
            LuaType::Variadic(inner) => write!(f, "{inner}..."),
            LuaType::FunctionSig { params, returns } => {
                write!(f, "fun(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "p{}: {p}", i + 1)?;
                }
                write!(f, ")")?;
                if !returns.is_empty() {
                    write!(f, ": ")?;
                    for (i, r) in returns.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{r}")?;
                    }
                }
                Ok(())
            }
        }
    }
}

fn format_embedded_class_name(name: &str) -> String {
    parse_embedded_lua_type(name)
        .map(|ty| ty.to_string())
        .unwrap_or_else(|| name.to_string())
}

fn parse_embedded_lua_type(text: &str) -> Option<LuaType> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    if let Some(inner) = strip_wrapping_parens(text) {
        return parse_embedded_lua_type(inner);
    }

    let union_parts = split_top_level(text, '|');
    if union_parts.len() > 1 {
        return Some(make_union(
            union_parts
                .into_iter()
                .map(parse_embedded_lua_type_atom)
                .collect(),
        ));
    }

    if let Some(inner) = text.strip_suffix('?') {
        return Some(LuaType::Optional(Box::new(parse_embedded_lua_type_atom(
            inner.trim(),
        ))));
    }

    if let Some(inner) = text.strip_suffix("[]") {
        return Some(LuaType::Array(Box::new(parse_embedded_lua_type_atom(
            inner.trim(),
        ))));
    }

    if let Some(inner) = text.strip_suffix("...") {
        return Some(LuaType::Variadic(Box::new(parse_embedded_lua_type_atom(
            inner.trim(),
        ))));
    }

    if let Some(inner) = text
        .strip_prefix("table<")
        .and_then(|inner| inner.strip_suffix('>'))
    {
        let parts = split_top_level(inner, ',');
        if parts.len() == 2 {
            return Some(LuaType::Map(
                Box::new(parse_embedded_lua_type_atom(parts[0])),
                Box::new(parse_embedded_lua_type_atom(parts[1])),
            ));
        }
    }

    None
}

fn parse_embedded_lua_type_atom(text: &str) -> LuaType {
    parse_embedded_lua_type(text).unwrap_or_else(|| match text.trim() {
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
    })
}

fn strip_wrapping_parens(text: &str) -> Option<&str> {
    let inner = text.strip_prefix('(')?.strip_suffix(')')?;
    let mut depth = 0usize;
    for ch in inner.chars() {
        match ch {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    (depth == 0).then_some(inner.trim())
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

/// A parameter in a Lua method/function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaParam {
    pub name: String,
    pub ty: LuaType,
}

/// A return value from a Lua method/function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaReturn {
    pub ty: LuaType,
    pub name: Option<String>,
}

impl From<LuaType> for LuaReturn {
    fn from(ty: LuaType) -> Self {
        Self { ty, name: None }
    }
}

impl LuaReturn {
    pub fn named(ty: LuaType, name: impl Into<String>) -> Self {
        Self {
            ty,
            name: Some(name.into()),
        }
    }
}

/// Whether a method takes `self` or is static.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MethodKind {
    Method,
    Function,
}

/// A method or function on a Lua class.
#[derive(Debug, Clone)]
pub struct LuaMethod {
    pub name: String,
    pub kind: MethodKind,
    pub is_async: bool,
    pub params: Vec<LuaParam>,
    /// Multiple return values (empty = void/nil).
    pub returns: Vec<LuaReturn>,
    pub doc: Option<String>,
}

/// A field on a Lua class.
#[derive(Debug, Clone)]
pub struct LuaField {
    pub name: String,
    pub ty: LuaType,
    pub writable: bool,
    pub doc: Option<String>,
}

/// A complete Lua class extracted from a `impl UserData` block.
#[derive(Debug, Clone)]
pub struct LuaClass {
    pub name: String,
    pub doc: Option<String>,
    pub fields: Vec<LuaField>,
    pub methods: Vec<LuaMethod>,
}

/// A Lua type alias, typically from a Rust enum with `IntoLua`/`FromLua`.
/// Generates `---@alias Name "variant1" | "variant2" | ...`
#[derive(Debug, Clone)]
pub struct LuaEnum {
    pub name: String,
    pub doc: Option<String>,
    /// Snake_case variant names for the alias (e.g., "spawn_tab").
    pub variants: Vec<String>,
    /// Original PascalCase variant names for enum constructor fields (e.g., "SpawnTab").
    pub pascal_variants: Vec<String>,
}

/// A standalone function registered on a table or as a global.
#[derive(Debug, Clone)]
pub struct LuaFunction {
    pub name: String,
    pub is_async: bool,
    pub params: Vec<LuaParam>,
    /// Multiple return values (empty = void/nil).
    pub returns: Vec<LuaReturn>,
    pub doc: Option<String>,
}

/// A module (table namespace) containing functions and values.
#[derive(Debug, Clone)]
pub struct LuaModule {
    pub name: String,
    pub doc: Option<String>,
    pub functions: Vec<LuaFunction>,
    pub fields: Vec<LuaField>,
}

/// Build a union type from collected branch types.
/// Deduplicates, collapses `T | nil` -> `Optional(T)`, and simplifies single-element unions.
pub fn make_union(types: Vec<LuaType>) -> LuaType {
    let mut flat: Vec<LuaType> = Vec::new();
    for ty in types {
        match ty {
            LuaType::Union(inner) => flat.extend(inner),
            LuaType::Optional(inner) => {
                flat.push(*inner);
                flat.push(LuaType::Nil);
            }
            other => flat.push(other),
        }
    }

    let mut seen = Vec::new();
    for ty in flat {
        if !seen.contains(&ty) {
            seen.push(ty);
        }
    }

    normalize_userdata_ref_union_items(&mut seen);

    let has_concrete = seen
        .iter()
        .any(|t| !matches!(t, LuaType::Any | LuaType::Nil));
    if has_concrete {
        seen.retain(|t| !matches!(t, LuaType::Any));
    }

    let has_nil = seen.iter().any(|t| matches!(t, LuaType::Nil));
    if has_nil {
        seen.retain(|t| !matches!(t, LuaType::Nil));
    }

    let base = match seen.len() {
        0 => {
            return if has_nil { LuaType::Nil } else { LuaType::Any };
        }
        1 => seen.into_iter().next().unwrap(),
        _ => LuaType::Union(seen),
    };

    if has_nil {
        LuaType::Optional(Box::new(base))
    } else {
        base
    }
}

fn normalize_userdata_ref_union_items(types: &mut Vec<LuaType>) {
    let has_generic_userdata_ref = types.iter().any(is_generic_userdata_ref_type);
    if !has_generic_userdata_ref {
        return;
    }

    for ty in types.iter_mut() {
        if let LuaType::Class(name) = ty {
            if matches!(name.as_str(), "UserDataRef" | "UserDataRefMut") {
                continue;
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
    }

    let mut deduped = Vec::new();
    for ty in std::mem::take(types) {
        if !deduped.contains(&ty) {
            deduped.push(ty);
        }
    }

    if deduped
        .iter()
        .any(|ty| !is_generic_userdata_ref_type(ty) && !matches!(ty, LuaType::Nil))
    {
        deduped.retain(|ty| !is_generic_userdata_ref_type(ty));
    }

    *types = deduped;
}

fn is_generic_userdata_ref_type(ty: &LuaType) -> bool {
    matches!(ty, LuaType::Class(name) if matches!(name.as_str(), "UserDataRef" | "UserDataRefMut"))
}

/// A discovered event emission: event name → callback argument types.
/// Used to infer callback signatures for event registration functions like `wezterm.on`.
#[derive(Debug, Clone)]
pub struct EventEmission {
    pub event_name: String,
    pub arg_types: Vec<LuaType>,
}

/// Mutable rustc extraction facts before semantic assembly.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct ExtractedApi {
    pub provenance: Provenance,
    pub classes: Vec<LuaClass>,
    pub enums: Vec<LuaEnum>,
    pub modules: Vec<LuaModule>,
    pub global_fields: Vec<LuaField>,
    pub global_functions: Vec<LuaFunction>,
    /// Event emissions found in this crate (event_name → arg types).
    pub event_emissions: Vec<EventEmission>,
    /// Exact directional conversion behavior, kept separate from rendered aliases.
    pub conversions: Vec<ConversionContract>,
}

impl ExtractedApi {
    #[must_use]
    pub fn new(provenance: Provenance) -> Self {
        Self {
            provenance,
            classes: Vec::new(),
            enums: Vec::new(),
            modules: Vec::new(),
            global_fields: Vec::new(),
            global_functions: Vec::new(),
            event_emissions: Vec::new(),
            conversions: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_union_prefers_specific_userdata_ref_alias() {
        assert_eq!(
            make_union(vec![
                LuaType::Class("UserDataRef".to_string()),
                LuaType::Class("FileRef".to_string()),
                LuaType::Integer,
            ]),
            LuaType::Union(vec![LuaType::Class("File".to_string()), LuaType::Integer])
        );
    }

    #[test]
    fn display_embedded_class_union_normalizes_userdata_refs() {
        assert_eq!(
            LuaType::Class("UserDataRef | FileRef | integer".to_string()).to_string(),
            "File | integer"
        );
    }

    #[test]
    fn display_embedded_class_map_normalizes_userdata_refs() {
        assert_eq!(
            LuaType::Class("table<string, UserDataRef | FileRef | integer>".to_string())
                .to_string(),
            "table<string, File | integer>"
        );
    }
}
