use crate::{
    Dialect, ExtractedApi, LuaClass, LuaField, LuaFunction, LuaMethod, LuaModule, LuaParam,
    LuaReturn, LuaType, MethodKind, Provenance, SemanticApi,
};

use super::render;

fn fixture() -> SemanticApi {
    let mut extracted = ExtractedApi::new(Provenance::explicit("test fixture"));
    extracted.classes.push(LuaClass {
        name: "Box<T>".into(),
        doc: Some("A box.".into()),
        fields: vec![LuaField {
            name: "value".into(),
            ty: LuaType::Class("T".into()),
            writable: false,
            doc: None,
        }],
        methods: vec![LuaMethod {
            name: "map".into(),
            kind: MethodKind::Method,
            is_async: false,
            params: vec![LuaParam {
                name: "f".into(),
                ty: LuaType::Function,
            }],
            returns: vec![LuaReturn::from(LuaType::Class("T".into()))],
            doc: None,
        }],
    });
    extracted.modules.push(LuaModule {
        name: "odd.module".into(),
        doc: None,
        fields: Vec::new(),
        functions: vec![LuaFunction {
            name: "do-work".into(),
            is_async: false,
            params: Vec::new(),
            returns: vec![LuaReturn::from(LuaType::Boolean)],
            doc: Some("Does work.".into()),
        }],
    });
    crate::ExtractedUnit::assemble(extracted).unwrap().api
}

#[test]
fn both_dialects_render_the_same_complete_semantics() {
    let api = fixture();
    for dialect in [Dialect::LuaLs, Dialect::EmmyLua] {
        let document = render(&api, dialect).unwrap();
        assert_eq!(document.dialect(), dialect);
        assert!(document.as_str().contains("---@class (exact) Box<T>"));
        assert!(document.as_str().contains("---@field value T (readonly)"));
        assert!(document.as_str().contains("_G[\"odd.module\"]"));
        assert!(document.as_str().contains("[\"do-work\"]"));
        assert!(document.as_str().contains("--- Does work."));
    }
}

#[test]
fn function_report_keys_match_extractor_lookup_names() {
    let directory = tempfile::tempdir().unwrap();
    let api = fixture();
    super::append_function_types(directory.path(), &api).unwrap();
    let rows = super::try_read_function_types(directory.path()).unwrap();
    assert_eq!(
        rows.get("odd.module.do-work"),
        Some(&vec!["boolean".into()])
    );
}

#[test]
fn type_rendering_preserves_structure() {
    let ty = crate::TypeExpr::Array(Box::new(crate::TypeExpr::Union(vec![
        crate::TypeExpr::Integer,
        crate::TypeExpr::Nil,
    ])));
    assert_eq!(super::render::type_expr(&ty), "(integer | nil)[]");
}
