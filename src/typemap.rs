use crate::{LuaType, make_union};

/// Whether `path` is a Rust or `mlua` definition with a native Lua contract.
#[must_use]
pub fn has_native_mapping(path: &str) -> bool {
    matches!(
        path,
        "std::string::String"
            | "alloc::string::String"
            | "std::vec::Vec"
            | "alloc::vec::Vec"
            | "std::option::Option"
            | "core::option::Option"
            | "std::result::Result"
            | "core::result::Result"
            | "std::collections::HashMap"
            | "std::collections::hash_map::HashMap"
            | "std::collections::BTreeMap"
            | "std::collections::btree_map::BTreeMap"
            | "std::collections::HashSet"
            | "std::collections::hash_set::HashSet"
            | "std::collections::BTreeSet"
            | "std::collections::btree_set::BTreeSet"
            | "std::boxed::Box"
            | "alloc::boxed::Box"
            | "std::ffi::CString"
            | "std::ffi::CStr"
            | "core::ffi::CStr"
            | "std::ffi::OsString"
            | "std::ffi::OsStr"
            | "std::path::PathBuf"
            | "std::path::Path"
            | "std::borrow::Cow"
            | "alloc::borrow::Cow"
            | "mlua::table::Table"
            | "mlua::Table"
            | "mlua::prelude::LuaTable"
            | "mlua::function::Function"
            | "mlua::Function"
            | "mlua::prelude::LuaFunction"
            | "mlua::value::Value"
            | "mlua::Value"
            | "mlua::prelude::LuaValue"
            | "mlua::string::String"
            | "mlua::String"
            | "mlua::BString"
            | "mlua::prelude::LuaString"
            | "mlua::thread::Thread"
            | "mlua::Thread"
            | "mlua::prelude::LuaThread"
            | "mlua::types::Integer"
            | "mlua::Integer"
            | "mlua::prelude::LuaInteger"
            | "mlua::types::Number"
            | "mlua::Number"
            | "mlua::prelude::LuaNumber"
            | "mlua::types::LightUserData"
            | "mlua::LightUserData"
            | "mlua::prelude::LuaLightUserData"
            | "mlua::userdata::AnyUserData"
            | "mlua::AnyUserData"
            | "mlua::prelude::LuaAnyUserData"
            | "mlua::userdata::UserDataRef"
            | "mlua::UserDataRef"
            | "mlua::prelude::LuaUserDataRef"
            | "mlua::userdata::UserDataRefMut"
            | "mlua::UserDataRefMut"
            | "mlua::prelude::LuaUserDataRefMut"
            | "mlua::multi::MultiValue"
            | "mlua::MultiValue"
            | "mlua::prelude::LuaMultiValue"
            | "mlua::error::Error"
            | "mlua::Error"
            | "mlua::prelude::LuaError"
            | "mlua::types::Variadic"
            | "mlua::Variadic"
            | "mlua::prelude::LuaVariadic"
    )
}

/// Lowers a definition whose conversion contract is implemented by `mlua`.
/// Unknown definitions retain their canonical Rust identity for evidence
/// resolution in the driver.
pub fn map_rust_type(path: &str, arguments: &[LuaType]) -> LuaType {
    match path {
        "std::string::String" | "alloc::string::String" => LuaType::String,
        "std::vec::Vec" | "alloc::vec::Vec" => {
            unary(path, arguments, |item| LuaType::Array(Box::new(item)))
        }
        "std::option::Option" | "core::option::Option" => {
            unary(path, arguments, |item| match item {
                LuaType::Optional(_) => item,
                item => LuaType::Optional(Box::new(item)),
            })
        }
        "std::result::Result" | "core::result::Result" => unary(path, arguments, |success| success),
        "std::collections::HashMap"
        | "std::collections::hash_map::HashMap"
        | "std::collections::BTreeMap"
        | "std::collections::btree_map::BTreeMap" => binary(path, arguments, |key, value| {
            LuaType::Map(Box::new(key), Box::new(value))
        }),
        "std::collections::HashSet"
        | "std::collections::hash_set::HashSet"
        | "std::collections::BTreeSet"
        | "std::collections::btree_set::BTreeSet" => unary(path, arguments, |item| {
            make_union(vec![
                LuaType::Array(Box::new(item.clone())),
                LuaType::Map(Box::new(item), Box::new(LuaType::Boolean)),
            ])
        }),
        "std::boxed::Box" | "alloc::boxed::Box" => unary(path, arguments, |item| item),
        "std::ffi::CString" | "std::ffi::CStr" | "core::ffi::CStr" | "std::ffi::OsString"
        | "std::ffi::OsStr" | "std::path::PathBuf" | "std::path::Path" => LuaType::String,
        "std::borrow::Cow" | "alloc::borrow::Cow" => unary(path, arguments, |item| item),

        "mlua::table::Table" | "mlua::Table" | "mlua::prelude::LuaTable" => LuaType::Table,
        "mlua::function::Function" | "mlua::Function" | "mlua::prelude::LuaFunction" => {
            LuaType::Function
        }
        "mlua::value::Value" | "mlua::Value" | "mlua::prelude::LuaValue" => {
            LuaType::Dynamic("mlua::Value is runtime-tagged".to_owned())
        }
        "mlua::string::String" | "mlua::String" | "mlua::BString" | "mlua::prelude::LuaString" => {
            LuaType::String
        }
        "mlua::thread::Thread" | "mlua::Thread" | "mlua::prelude::LuaThread" => LuaType::Thread,
        "mlua::types::Integer" | "mlua::Integer" | "mlua::prelude::LuaInteger" => LuaType::Integer,
        "mlua::types::Number" | "mlua::Number" | "mlua::prelude::LuaNumber" => LuaType::Number,
        "mlua::types::LightUserData"
        | "mlua::LightUserData"
        | "mlua::prelude::LuaLightUserData" => {
            LuaType::Dynamic("light userdata has no statically named pointee type".to_owned())
        }
        "mlua::userdata::AnyUserData" | "mlua::AnyUserData" | "mlua::prelude::LuaAnyUserData" => {
            LuaType::Dynamic("AnyUserData erases the registered userdata class".to_owned())
        }
        "mlua::userdata::UserDataRef"
        | "mlua::UserDataRef"
        | "mlua::prelude::LuaUserDataRef"
        | "mlua::userdata::UserDataRefMut"
        | "mlua::UserDataRefMut"
        | "mlua::prelude::LuaUserDataRefMut" => unary(path, arguments, |item| item),
        "mlua::multi::MultiValue" | "mlua::MultiValue" | "mlua::prelude::LuaMultiValue" => {
            LuaType::Variadic(Box::new(LuaType::Dynamic(
                "MultiValue elements are heterogeneous runtime values".to_owned(),
            )))
        }
        "mlua::error::Error" | "mlua::Error" | "mlua::prelude::LuaError" => LuaType::String,
        "mlua::types::Variadic" | "mlua::Variadic" | "mlua::prelude::LuaVariadic" => {
            unary(path, arguments, |item| LuaType::Variadic(Box::new(item)))
        }
        _ => unresolved(path, arguments),
    }
}

fn unary(path: &str, arguments: &[LuaType], lower: impl FnOnce(LuaType) -> LuaType) -> LuaType {
    arguments
        .first()
        .cloned()
        .map(lower)
        .unwrap_or_else(|| unresolved(path, arguments))
}

fn binary(
    path: &str,
    arguments: &[LuaType],
    lower: impl FnOnce(LuaType, LuaType) -> LuaType,
) -> LuaType {
    match arguments {
        [first, second, ..] => lower(first.clone(), second.clone()),
        _ => unresolved(path, arguments),
    }
}

fn unresolved(path: &str, arguments: &[LuaType]) -> LuaType {
    LuaType::UnresolvedRust {
        path: path.to_owned(),
        arguments: arguments.to_vec(),
    }
}

#[cfg(test)]
mod tests;
