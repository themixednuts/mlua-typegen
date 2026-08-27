use super::*;
use crate::LuaModule;

pub(super) fn empty_signature() -> Signature {
    Signature {
        docs: Vec::new(),
        type_parameters: Vec::new(),
        inputs: Vec::new(),
        variadic: None,
        outputs: Vec::new(),
        is_async: false,
        receiver: None,
        origins: Origins::one(Provenance::explicit("untyped Lua function")),
    }
}

pub(super) fn contributions(extracted: &ExtractedApi) -> Vec<ApiContribution> {
    let origin = extracted.provenance.clone();
    let mut contributions = Vec::with_capacity(
        extracted.classes.len()
            + extracted.enums.len()
            + extracted.modules.len()
            + extracted.global_fields.len()
            + extracted.global_functions.len(),
    );
    for class in &extracted.classes {
        contributions.extend(class_contributions(class, origin.clone()));
    }
    contributions.extend(extracted.enums.iter().map(|alias| ApiContribution::Type {
        id: TypeId::new(alias.name.clone()),
        definition: TypeDef::Alias(lower_alias(alias, origin.clone())),
    }));
    for module in &extracted.modules {
        contributions.extend(module_contributions(module, origin.clone()));
    }
    contributions.extend(
        extracted
            .global_fields
            .iter()
            .map(|field| ApiContribution::Value {
                path: LuaPath::global(field.name.as_str()),
                definition: ValueDef::Typed(lower_field(field, &[], origin.clone())),
            }),
    );
    contributions.extend(extracted.global_functions.iter().map(|function| {
        ApiContribution::Value {
            path: LuaPath::global(function.name.as_str()),
            definition: ValueDef::Function(OverloadSet::one(lower_function(
                function,
                None,
                &[],
                origin.clone(),
            ))),
        }
    }));
    contributions
}

fn class_contributions(class: &LuaClass, origin: Provenance) -> Vec<ApiContribution> {
    let id = class_type_id(&class.name);
    let mut fragments = Vec::with_capacity(1 + class.fields.len() + class.methods.len());
    let mut base = class.clone();
    base.fields.clear();
    base.methods.clear();
    fragments.push(ApiContribution::Type {
        id: id.clone(),
        definition: TypeDef::Class(lower_class(&base, origin.clone())),
    });
    for field in &class.fields {
        let mut fragment = base.clone();
        fragment.fields.push(field.clone());
        fragments.push(ApiContribution::Type {
            id: id.clone(),
            definition: TypeDef::Class(lower_class(&fragment, origin.clone())),
        });
    }
    for method in &class.methods {
        let mut fragment = base.clone();
        fragment.methods.push(method.clone());
        fragments.push(ApiContribution::Type {
            id: id.clone(),
            definition: TypeDef::Class(lower_class(&fragment, origin.clone())),
        });
    }
    fragments
}

fn module_contributions(module: &LuaModule, origin: Provenance) -> Vec<ApiContribution> {
    let path = LuaPath::global(module.name.as_str());
    let mut fragments = Vec::with_capacity(1 + module.fields.len() + module.functions.len());
    let mut base = module.clone();
    base.fields.clear();
    base.functions.clear();
    fragments.push(ApiContribution::Value {
        path: path.clone(),
        definition: ValueDef::Table(lower_table(&base, origin.clone())),
    });
    for field in &module.fields {
        let mut fragment = base.clone();
        fragment.fields.push(field.clone());
        fragments.push(ApiContribution::Value {
            path: path.clone(),
            definition: ValueDef::Table(lower_table(&fragment, origin.clone())),
        });
    }
    for function in &module.functions {
        let mut fragment = base.clone();
        fragment.functions.push(function.clone());
        fragments.push(ApiContribution::Value {
            path: path.clone(),
            definition: ValueDef::Table(lower_table(&fragment, origin.clone())),
        });
    }
    fragments
}
