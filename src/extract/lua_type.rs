use mlua_typegen::{LuaType, make_union};

/// Sentinel value used to mark "returns Self" during extraction.
/// `extract_class` replaces this with the actual class name.
pub(super) fn self_return_sentinel() -> LuaType {
    LuaType::Class(String::new())
}

fn is_self_return_sentinel(ty: &LuaType) -> bool {
    matches!(ty, LuaType::Class(name) if name.is_empty())
}

pub(super) fn contains_self_return_sentinel(ty: &LuaType) -> bool {
    match ty {
        LuaType::Class(_) => is_self_return_sentinel(ty),
        LuaType::UnresolvedRust { arguments, .. } => {
            arguments.iter().any(contains_self_return_sentinel)
        }
        LuaType::ExternalRust { arguments, .. } => {
            arguments.iter().any(contains_self_return_sentinel)
        }
        LuaType::Array(inner) | LuaType::Optional(inner) | LuaType::Variadic(inner) => {
            contains_self_return_sentinel(inner)
        }
        LuaType::Map(key, value) => {
            contains_self_return_sentinel(key) || contains_self_return_sentinel(value)
        }
        LuaType::Record { fields, .. } => fields
            .iter()
            .any(|field| contains_self_return_sentinel(&field.ty)),
        LuaType::Union(items) => items.iter().any(contains_self_return_sentinel),
        LuaType::FunctionSig { params, returns } => {
            params.iter().any(contains_self_return_sentinel)
                || returns.iter().any(contains_self_return_sentinel)
        }
        _ => false,
    }
}

pub(super) fn walk_lua_type_mut<F>(ty: &mut LuaType, visit: &mut F)
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
        LuaType::Record { fields, .. } => {
            for field in fields {
                walk_lua_type_mut(&mut field.ty, visit);
            }
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
        LuaType::UnresolvedRust { arguments, .. } | LuaType::ExternalRust { arguments, .. } => {
            for argument in arguments {
                walk_lua_type_mut(argument, visit);
            }
        }
        _ => {}
    }
}

pub(super) fn replace_self_return_sentinel(ty: &mut LuaType, class_name: &str) {
    walk_lua_type_mut(ty, &mut |ty| {
        if let LuaType::Class(name) = ty
            && name.is_empty()
        {
            *name = class_name.to_string();
        }
    });
}

pub(super) fn replace_self_class_alias(ty: &mut LuaType, class_name: &str) {
    walk_lua_type_mut(ty, &mut |ty| {
        if let LuaType::Class(name) = ty
            && name == "Self"
        {
            *name = class_name.to_string();
        }
    });
}

pub(super) fn lua_runtime_type() -> LuaType {
    LuaType::Class("Lua".to_string())
}

pub(super) fn normalize_explicit_param_type(mut ty: LuaType) -> LuaType {
    normalize_userdata_ref_alias_type(&mut ty);
    ty
}

pub(super) fn should_prefer_explicit_param_type(existing: &LuaType, explicit: &LuaType) -> bool {
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

pub(super) fn normalize_param_lua_type(ty: LuaType) -> LuaType {
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

pub(super) fn normalize_table_value_types(mut value_types: Vec<LuaType>) -> Vec<LuaType> {
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

pub(super) fn normalize_inferred_lua_type(ty: LuaType) -> LuaType {
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

pub(super) fn is_generic_userdata_ref_type(ty: &LuaType) -> bool {
    matches!(ty, LuaType::Class(name) if matches!(name.as_str(), "UserDataRef" | "UserDataRefMut"))
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

pub(super) fn lua_type_from_extracted_name(name: &str) -> LuaType {
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

    if let Some(inner) = qualified_name
        .strip_prefix("Variadic<")
        .and_then(|inner| inner.strip_suffix('>'))
    {
        return LuaType::Variadic(Box::new(lua_type_from_extracted_name(inner.trim())));
    }

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

pub(super) fn is_better_inferred_type(existing: &LuaType, inferred: &LuaType) -> bool {
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

/// Returns true if a LuaType carries useful type information (not Any, Nil, or Function bare).
pub(super) fn is_informative(ty: &LuaType) -> bool {
    match ty {
        LuaType::Any | LuaType::Nil | LuaType::Function => false,
        LuaType::Optional(inner) => is_informative(inner),
        LuaType::Variadic(inner) => is_informative(inner),
        LuaType::Class(name) if name.contains("{opaque") || name.starts_with('{') => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
