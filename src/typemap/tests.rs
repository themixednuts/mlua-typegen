use super::*;

fn unresolved(path: &str, arguments: Vec<LuaType>) -> LuaType {
    LuaType::UnresolvedRust {
        path: path.to_owned(),
        arguments,
    }
}

#[test]
fn maps_owned_string_contract() {
    assert_eq!(map_rust_type("std::string::String", &[]), LuaType::String);
}

#[test]
fn maps_standard_transparent_and_collection_types() {
    assert_eq!(
        map_rust_type("std::vec::Vec", &[LuaType::Integer]),
        LuaType::Array(Box::new(LuaType::Integer))
    );
    assert_eq!(
        map_rust_type("std::option::Option", &[LuaType::String]),
        LuaType::Optional(Box::new(LuaType::String))
    );
    assert_eq!(
        map_rust_type(
            "std::result::Result",
            &[LuaType::String, LuaType::Class("LuaError".into())]
        ),
        LuaType::String,
        "callback errors are thrown and never become Lua values"
    );
    assert_eq!(
        map_rust_type(
            "std::collections::HashMap",
            &[LuaType::String, LuaType::Boolean]
        ),
        LuaType::Map(Box::new(LuaType::String), Box::new(LuaType::Boolean))
    );
    assert_eq!(
        map_rust_type("std::vec::Vec", &[LuaType::Integer]),
        LuaType::Array(Box::new(LuaType::Integer)),
        "Vec<u8> follows mlua's sequence-table conversion, not string coercion"
    );
    assert_eq!(
        map_rust_type("std::collections::HashSet", &[LuaType::String]),
        make_union(vec![
            LuaType::Array(Box::new(LuaType::String)),
            LuaType::Map(Box::new(LuaType::String), Box::new(LuaType::Boolean)),
        ])
    );
}

#[test]
fn maps_native_mlua_types_without_alias_guessing() {
    assert_eq!(
        map_rust_type("mlua::Value", &[]),
        LuaType::Dynamic("mlua::Value is runtime-tagged".to_owned())
    );
    assert_eq!(map_rust_type("mlua::Table", &[]), LuaType::Table);
    assert_eq!(map_rust_type("mlua::Function", &[]), LuaType::Function);
    assert_eq!(map_rust_type("mlua::String", &[]), LuaType::String);
    assert_eq!(map_rust_type("mlua::Thread", &[]), LuaType::Thread);
    assert_eq!(
        map_rust_type("mlua::Variadic", &[LuaType::Boolean]),
        LuaType::Variadic(Box::new(LuaType::Boolean))
    );
    assert_eq!(
        map_rust_type("mlua::UserDataRef", &[LuaType::Class("File".into())]),
        LuaType::Class("File".into())
    );
}

#[test]
fn preserves_unknown_definition_identity_for_evidence_resolution() {
    assert_eq!(
        map_rust_type("domain::UserId", &[]),
        unresolved("domain::UserId", Vec::new())
    );
    assert_eq!(
        map_rust_type("domain::Page", &[LuaType::String]),
        unresolved("domain::Page", vec![LuaType::String])
    );
}

#[test]
fn third_party_names_do_not_invent_conversion_semantics() {
    for path in [
        "uuid::Uuid",
        "bytes::Bytes",
        "serde_json::Value",
        "custom::SecretString",
        "std::sync::Arc",
        "parking_lot::Mutex",
    ] {
        assert_eq!(
            map_rust_type(path, &[]),
            unresolved(path, Vec::new()),
            "{path}"
        );
    }
    assert_eq!(
        map_rust_type("custom::ThingVec", &[LuaType::Integer]),
        unresolved("custom::ThingVec", vec![LuaType::Integer])
    );
}
