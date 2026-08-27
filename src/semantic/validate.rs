use std::collections::BTreeMap;

use super::*;

pub(super) fn graph(api: &SemanticApi, errors: &mut Vec<SemanticError>) {
    let arities = api
        .types
        .iter()
        .map(|(id, definition)| (id, type_parameters(definition)))
        .collect::<BTreeMap<_, _>>();

    for definition in api.types.values() {
        match definition {
            TypeDef::Class(class) => {
                for field in class.fields.values() {
                    type_expr(&field.ty, &field.origins, &arities, errors);
                }
                for overloads in class.methods.values() {
                    signatures(overloads, &arities, errors);
                }
            }
            TypeDef::Alias(alias) => {
                type_expr(&alias.target, &alias.origins, &arities, errors);
                for constructor in alias.constructors.values() {
                    type_expr(constructor, &alias.origins, &arities, errors);
                }
            }
        }
    }

    for value in api.values.values() {
        match value {
            ValueDef::Table(table) => {
                for field in table.fields.values() {
                    type_expr(&field.ty, &field.origins, &arities, errors);
                }
                for overloads in table.functions.values() {
                    signatures(overloads, &arities, errors);
                }
            }
            ValueDef::Function(overloads) => signatures(overloads, &arities, errors),
            ValueDef::Typed(field) => type_expr(&field.ty, &field.origins, &arities, errors),
        }
    }
}

fn type_parameters(definition: &TypeDef) -> usize {
    match definition {
        TypeDef::Class(class) => class.parameters.len(),
        TypeDef::Alias(alias) => alias.parameters.len(),
    }
}

fn signatures(
    overloads: &OverloadSet,
    arities: &BTreeMap<&TypeId, usize>,
    errors: &mut Vec<SemanticError>,
) {
    for signature in overloads.iter() {
        for parameter in &signature.inputs {
            type_expr(&parameter.ty, &signature.origins, arities, errors);
        }
        if let Some(variadic) = &signature.variadic {
            type_expr(variadic, &signature.origins, arities, errors);
        }
        for output in &signature.outputs {
            type_expr(&output.ty, &signature.origins, arities, errors);
        }
    }
}

fn type_expr(
    ty: &TypeExpr,
    origins: &Origins,
    arities: &BTreeMap<&TypeId, usize>,
    errors: &mut Vec<SemanticError>,
) {
    match ty {
        TypeExpr::Named { id, arguments } => {
            if let Some(expected) = arities.get(id)
                && *expected != arguments.len()
            {
                errors.push(SemanticError::GenericArity {
                    referenced: id.clone(),
                    expected: *expected,
                    actual: arguments.len(),
                    origins: origins.clone(),
                });
            }
            for argument in arguments {
                type_expr(argument, origins, arities, errors);
            }
        }
        TypeExpr::Function(signature) => {
            for parameter in &signature.inputs {
                type_expr(&parameter.ty, origins, arities, errors);
            }
            if let Some(variadic) = &signature.variadic {
                type_expr(variadic, origins, arities, errors);
            }
            for output in &signature.outputs {
                type_expr(&output.ty, origins, arities, errors);
            }
        }
        TypeExpr::Array(item) => type_expr(item, origins, arities, errors),
        TypeExpr::Map { key, value } => {
            type_expr(key, origins, arities, errors);
            type_expr(value, origins, arities, errors);
        }
        TypeExpr::Record { fields, .. } => {
            for (_, field) in fields {
                type_expr(field, origins, arities, errors);
            }
        }
        TypeExpr::Union(items) => {
            for item in items {
                type_expr(item, origins, arities, errors);
            }
        }
        TypeExpr::Nil
        | TypeExpr::Boolean
        | TypeExpr::Integer
        | TypeExpr::Number
        | TypeExpr::String
        | TypeExpr::Table
        | TypeExpr::Any
        | TypeExpr::Dynamic { .. }
        | TypeExpr::Thread
        | TypeExpr::Literal(_)
        | TypeExpr::Parameter(_) => {}
    }
}
