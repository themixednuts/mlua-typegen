pub mod codegen;
pub mod typemap;

use std::fmt;

/// Which language server to target for codegen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodegenTarget {
    /// LuaCATS annotations (used by LuaLS). This is the default.
    #[default]
    LuaLS,
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
    /// A typed array: `T[]`
    Array(Box<LuaType>),
    /// An optional type: `T?`
    Optional(Box<LuaType>),
    /// A typed table: `table<K, V>`
    Map(Box<LuaType>, Box<LuaType>),
    /// A reference to another UserData class or alias by name.
    Class(String),
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
            LuaType::Array(inner) => {
                // Wrap complex inner types in parens to avoid ambiguity:
                // (string?)[] not string?[] (which LuaLS reads as string[]?)
                if matches!(inner.as_ref(), LuaType::Optional(_) | LuaType::Map(_, _)) {
                    write!(f, "({inner})[]")
                } else {
                    write!(f, "{inner}[]")
                }
            }
            LuaType::Optional(inner) => write!(f, "{inner}?"),
            LuaType::Map(k, v) => write!(f, "table<{k}, {v}>"),
            LuaType::Class(name) => write!(f, "{name}"),
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

/// A parameter in a Lua method/function.
#[derive(Debug, Clone)]
pub struct LuaParam {
    pub name: String,
    pub ty: LuaType,
}

/// A return value from a Lua method/function.
#[derive(Debug, Clone)]
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
        Self { ty, name: Some(name.into()) }
    }
}

/// Whether a method takes `self` or is static.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub variants: Vec<String>,
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
/// Deduplicates, collapses `T | nil` → `Optional(T)`, and simplifies single-element unions.
pub fn make_union(types: Vec<LuaType>) -> LuaType {
    // Flatten nested unions
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

    // Deduplicate (preserving order)
    let mut seen = Vec::new();
    for ty in flat {
        if !seen.contains(&ty) {
            seen.push(ty);
        }
    }

    // Remove Any — if we have concrete types, Any is just noise
    let has_concrete = seen.iter().any(|t| !matches!(t, LuaType::Any | LuaType::Nil));
    if has_concrete {
        seen.retain(|t| !matches!(t, LuaType::Any));
    }

    // Extract nil for optional handling
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

/// The complete Lua API surface extracted from a crate.
#[derive(Debug, Clone, Default)]
pub struct LuaApi {
    pub classes: Vec<LuaClass>,
    pub enums: Vec<LuaEnum>,
    pub modules: Vec<LuaModule>,
    pub global_functions: Vec<LuaFunction>,
}
