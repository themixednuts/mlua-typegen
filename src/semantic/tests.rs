use super::*;

fn extracted() -> ExtractedApi {
    ExtractedApi::new(Provenance::explicit("test fixture"))
}

fn api(input: ExtractedApi) -> Result<SemanticApi, SemanticErrors> {
    ExtractedUnit::assemble(input).map(|unit| unit.api)
}

#[test]
fn overload_sets_are_non_empty_and_merge_equivalent_origins() {
    let first = lower::empty_signature();
    let mut overloads = OverloadSet::one(first.clone());
    let mut second = first;
    second.origins = Origins::one(Provenance::explicit("second"));
    overloads.insert(second);
    assert_eq!(overloads.len(), 1);
    assert_eq!(overloads.primary().origins.iter().count(), 2);
}

#[test]
fn same_type_can_be_bound_at_two_lua_paths_without_duplication() {
    let mut input = extracted();
    input.classes.push(LuaClass {
        name: "Box<T>".into(),
        doc: None,
        fields: Vec::new(),
        methods: Vec::new(),
    });
    for name in ["primary", "legacy"] {
        input.global_fields.push(LuaField {
            name: name.into(),
            ty: LuaType::Class("Box<string>".into()),
            writable: false,
            doc: None,
        });
    }
    let api = api(input).unwrap();
    assert_eq!(api.types().count(), 1);
    assert_eq!(api.values().count(), 2);
    assert!(api.type_definition(&TypeId::new("Box")).is_some());
}

#[test]
fn compatible_module_reopening_is_order_independent() {
    let mut first = extracted();
    for (name, ty) in [("a", LuaType::String), ("b", LuaType::Integer)] {
        first.modules.push(crate::LuaModule {
            name: "toolkit".into(),
            doc: None,
            functions: Vec::new(),
            fields: vec![LuaField {
                name: name.into(),
                ty,
                writable: false,
                doc: None,
            }],
        });
    }
    let mut reversed = first.clone();
    reversed.modules.reverse();
    assert_eq!(api(first).unwrap(), api(reversed).unwrap());
}

#[test]
fn incompatible_values_fail_instead_of_widening() {
    let mut input = extracted();
    input.global_fields.push(LuaField {
        name: "value".into(),
        ty: LuaType::String,
        writable: false,
        doc: None,
    });
    input.global_functions.push(LuaFunction {
        name: "value".into(),
        is_async: false,
        params: Vec::new(),
        returns: Vec::new(),
        doc: None,
    });
    let errors = api(input).unwrap_err();
    assert!(matches!(
        errors.iter().next(),
        Some(SemanticError::ConflictingValue { .. })
    ));
}

#[test]
fn incompatible_members_in_one_extracted_class_are_not_deduplicated() {
    let mut input = extracted();
    input.classes.push(LuaClass {
        name: "Conflicted".into(),
        doc: None,
        fields: [LuaType::String, LuaType::Integer]
            .into_iter()
            .map(|ty| LuaField {
                name: "value".into(),
                ty,
                writable: false,
                doc: None,
            })
            .collect(),
        methods: Vec::new(),
    });

    let errors = api(input).unwrap_err();

    assert!(matches!(
        errors.iter().next(),
        Some(SemanticError::ConflictingType { .. })
    ));
}

#[test]
fn known_generic_references_must_have_the_declared_arity() {
    let mut input = extracted();
    input.classes.push(LuaClass {
        name: "Box<T>".into(),
        doc: None,
        fields: Vec::new(),
        methods: Vec::new(),
    });
    input.global_fields.push(LuaField {
        name: "box".into(),
        ty: LuaType::Class("Box".into()),
        writable: false,
        doc: None,
    });

    let errors = api(input).unwrap_err();

    assert!(matches!(
        errors.iter().next(),
        Some(SemanticError::GenericArity {
            expected: 1,
            actual: 0,
            ..
        })
    ));
}
