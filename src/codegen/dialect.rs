use std::fmt;

use crate::{Dialect, Origins, OverloadSet, SemanticApi, TypeDef, ValueDef};

use super::render as rendering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialectFeature {
    ExactClass,
    GenericOverload,
}

impl fmt::Display for DialectFeature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ExactClass => "exact class",
            Self::GenericOverload => "generic overload",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    #[error("{dialect:?} cannot represent {feature} from {origins:?}")]
    Unsupported {
        dialect: Dialect,
        feature: DialectFeature,
        origins: Origins,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedDocument {
    dialect: Dialect,
    text: String,
}

impl RenderedDocument {
    pub(super) fn new(dialect: Dialect, text: String) -> Self {
        Self { dialect, text }
    }
    #[must_use]
    pub fn dialect(&self) -> Dialect {
        self.dialect
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

pub(super) fn render(api: &SemanticApi, dialect: Dialect) -> Result<RenderedDocument, RenderError> {
    let capabilities = match dialect {
        Dialect::LuaLs => &[DialectFeature::ExactClass, DialectFeature::GenericOverload][..],
        Dialect::EmmyLua => &[DialectFeature::ExactClass, DialectFeature::GenericOverload][..],
    };
    ensure_supported(api, dialect, capabilities)?;
    Ok(RenderedDocument::new(dialect, rendering::document(api)))
}

fn ensure_supported(
    api: &SemanticApi,
    dialect: Dialect,
    capabilities: &[DialectFeature],
) -> Result<(), RenderError> {
    for (feature, origins) in required_features(api) {
        if !capabilities.contains(&feature) {
            return Err(RenderError::Unsupported {
                dialect,
                feature,
                origins,
            });
        }
    }
    Ok(())
}

fn required_features(api: &SemanticApi) -> Vec<(DialectFeature, Origins)> {
    let mut required = Vec::new();
    for (_, definition) in api.types() {
        match definition {
            TypeDef::Class(class) => {
                required.push((DialectFeature::ExactClass, class.origins.clone()));
                require_generic_overloads(&mut required, class.methods.values(), &class.origins);
            }
            TypeDef::Alias(alias) if !alias.constructors.is_empty() => {
                required.push((DialectFeature::ExactClass, alias.origins.clone()));
            }
            TypeDef::Alias(_) => {}
        }
    }
    for (_, value) in api.values() {
        match value {
            ValueDef::Table(table) => {
                required.push((DialectFeature::ExactClass, table.origins.clone()));
                require_generic_overloads(&mut required, table.functions.values(), &table.origins);
            }
            ValueDef::Function(overloads) => {
                require_generic_overloads(
                    &mut required,
                    std::iter::once(overloads),
                    &overloads.primary().origins,
                );
            }
            ValueDef::Typed(_) => {}
        }
    }
    required
}

fn require_generic_overloads<'a>(
    required: &mut Vec<(DialectFeature, Origins)>,
    overloads: impl IntoIterator<Item = &'a OverloadSet>,
    origins: &Origins,
) {
    if overloads.into_iter().any(|overloads| {
        overloads
            .iter()
            .any(|signature| !signature.type_parameters.is_empty())
    }) {
        required.push((DialectFeature::GenericOverload, origins.clone()));
    }
}

#[cfg(test)]
mod tests {
    use crate::{ExtractedApi, ExtractedUnit, LuaClass, Provenance};

    use super::*;

    #[test]
    fn unsupported_semantics_fail_before_rendering() {
        let mut extracted = ExtractedApi::new(Provenance::explicit("fixture"));
        extracted.classes.push(LuaClass {
            name: "Exact".into(),
            doc: None,
            fields: Vec::new(),
            methods: Vec::new(),
        });
        let api = ExtractedUnit::assemble(extracted).unwrap().api;

        assert!(matches!(
            ensure_supported(&api, Dialect::LuaLs, &[]),
            Err(RenderError::Unsupported {
                feature: DialectFeature::ExactClass,
                ..
            })
        ));
    }
}
