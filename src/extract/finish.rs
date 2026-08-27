use std::collections::{BTreeSet, HashMap, HashSet};

use mlua_typegen::{
    ConversionContract, ExtractedApi, LuaField, LuaFunction, LuaMethod, LuaModule, LuaRecordField,
    LuaType, LuaValueDomain, TableOpenness,
    config::{MappingDirection, MappingId, MappingUseError, ResolvedConfig},
    driver_protocol::{EvidenceCatalog, EvidenceDirection, RequiredEvidence},
    make_union,
};

use super::trace::{method_trace_summary, should_trace_method_name, trace};
use super::{lua_type::walk_lua_type_mut, split_top_level};

pub(super) fn finish_api(
    api: &mut ExtractedApi,
    config: &ResolvedConfig,
    evidence: &EvidenceCatalog,
) -> Result<FinishOutcome, Vec<TypeResolutionError>> {
    dedupe_api(api);
    resolve_api_types(api, config, evidence)
}

pub(super) struct FinishOutcome {
    pub(super) used_mappings: BTreeSet<MappingId>,
    pub(super) requirements: BTreeSet<RequiredEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TypeResolutionError {
    MissingEvidence {
        rust_path: String,
        lua_path: String,
        direction: MappingDirection,
    },
    MissingDirection {
        mapping: MappingId,
        rust_path: String,
        lua_path: String,
        direction: MappingDirection,
    },
    InvalidMappingUse {
        rust_path: String,
        lua_path: String,
        direction: MappingDirection,
        source: MappingUseError,
    },
    UnsupportedStatic {
        rust_type: String,
        lua_path: String,
        reason: String,
        direction: MappingDirection,
    },
    ConflictingEvidence {
        mapping: MappingId,
        rust_path: String,
        lua_path: String,
        direction: MappingDirection,
        extracted: &'static str,
    },
}

impl TypeResolutionError {
    pub(super) fn rust_path(&self) -> &str {
        match self {
            Self::MissingEvidence { rust_path, .. }
            | Self::MissingDirection { rust_path, .. }
            | Self::InvalidMappingUse { rust_path, .. } => rust_path,
            Self::UnsupportedStatic { rust_type, .. } => rust_type,
            Self::ConflictingEvidence { rust_path, .. } => rust_path,
        }
    }

    pub(super) fn lua_path(&self) -> &str {
        match self {
            Self::MissingEvidence { lua_path, .. }
            | Self::MissingDirection { lua_path, .. }
            | Self::InvalidMappingUse { lua_path, .. }
            | Self::UnsupportedStatic { lua_path, .. }
            | Self::ConflictingEvidence { lua_path, .. } => lua_path,
        }
    }

    pub(super) const fn remediation(&self) -> &'static str {
        match self {
            Self::MissingEvidence { .. } => {
                "implement a native mlua conversion, make the defining source reachable, or declare an exact mapping"
            }
            Self::MissingDirection { .. } => {
                "declare this direction in the mapping or stop using the Rust type in that Lua direction"
            }
            Self::InvalidMappingUse { .. } => {
                "make the mapping parameters match the resolved Rust definition"
            }
            Self::UnsupportedStatic { .. } => {
                "use a supported static Rust type or add extractor support for this exact type form"
            }
            Self::ConflictingEvidence { .. } => {
                "remove the mapping because native or extracted evidence has precedence"
            }
        }
    }
}

impl std::fmt::Display for TypeResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingEvidence {
                rust_path,
                direction,
                ..
            } => write!(
                formatter,
                "no exact {direction} Lua contract is known for Rust type `{rust_path}`"
            ),
            Self::MissingDirection {
                mapping,
                rust_path,
                direction,
                ..
            } => write!(
                formatter,
                "mapping `{mapping}` does not declare {direction} for Rust type `{rust_path}`"
            ),
            Self::InvalidMappingUse { source, .. } => std::fmt::Display::fmt(source, formatter),
            Self::UnsupportedStatic {
                rust_type,
                reason,
                direction,
                ..
            } => write!(
                formatter,
                "cannot prove the {direction} Lua contract for static Rust type `{rust_type}`: {reason}"
            ),
            Self::ConflictingEvidence {
                mapping,
                rust_path,
                direction,
                extracted,
                ..
            } => write!(
                formatter,
                "mapping `{mapping}` conflicts with {extracted} {direction} evidence for `{rust_path}`"
            ),
        }
    }
}

fn dedupe_api(api: &mut ExtractedApi) {
    api.classes.sort_by(|a, b| a.name.cmp(&b.name));
    for class in &mut api.classes {
        dedupe_fields(&mut class.fields);
        dedupe_methods(&mut class.methods);
    }

    api.enums.sort_by(|a, b| a.name.cmp(&b.name));

    api.modules.sort_by(|a, b| a.name.cmp(&b.name));
    for module in &mut api.modules {
        dedupe_fields(&mut module.fields);
        dedupe_functions(&mut module.functions);
    }
    api.modules
        .retain(|module| !is_synthetic_helper_module(module));

    api.global_fields.sort_by(|a, b| a.name.cmp(&b.name));

    api.global_functions.sort_by(|a, b| a.name.cmp(&b.name));
}

fn resolve_api_types(
    api: &mut ExtractedApi,
    config: &ResolvedConfig,
    evidence: &EvidenceCatalog,
) -> Result<FinishOutcome, Vec<TypeResolutionError>> {
    let known_names: HashSet<String> = api
        .classes
        .iter()
        .map(|class| class.name.clone())
        .chain(api.enums.iter().map(|item| item.name.clone()))
        .chain(api.modules.iter().map(|module| module.name.clone()))
        .collect();
    let conversions = api
        .conversions
        .iter()
        .cloned()
        .map(|contract| (contract.rust_path.clone(), contract))
        .collect::<HashMap<_, _>>();
    let mut resolver = TypeResolver {
        known_names: &known_names,
        conversions: &conversions,
        evidence,
        config,
        used: BTreeSet::new(),
        requirements: BTreeSet::new(),
        errors: Vec::new(),
    };

    for class in &mut api.classes {
        for field in &mut class.fields {
            resolver.resolve(
                &mut field.ty,
                MappingDirection::IntoLua,
                &format!("{}.{}", class.name, field.name),
            );
        }
        for method in &mut class.methods {
            let lua_path = format!("{}.{}", class.name, method.name);
            for param in &mut method.params {
                resolver.resolve(&mut param.ty, MappingDirection::FromLua, &lua_path);
            }
            for ret in &mut method.returns {
                resolver.resolve(&mut ret.ty, MappingDirection::IntoLua, &lua_path);
            }
        }
    }

    for module in &mut api.modules {
        for field in &mut module.fields {
            resolver.resolve(
                &mut field.ty,
                MappingDirection::IntoLua,
                &format!("{}.{}", module.name, field.name),
            );
        }
        for func in &mut module.functions {
            let lua_path = format!("{}.{}", module.name, func.name);
            for param in &mut func.params {
                resolver.resolve(&mut param.ty, MappingDirection::FromLua, &lua_path);
            }
            for ret in &mut func.returns {
                resolver.resolve(&mut ret.ty, MappingDirection::IntoLua, &lua_path);
            }
        }
    }

    for field in &mut api.global_fields {
        resolver.resolve(&mut field.ty, MappingDirection::IntoLua, &field.name);
    }

    for func in &mut api.global_functions {
        for param in &mut func.params {
            resolver.resolve(&mut param.ty, MappingDirection::FromLua, &func.name);
        }
        for ret in &mut func.returns {
            resolver.resolve(&mut ret.ty, MappingDirection::IntoLua, &func.name);
        }
    }

    for event in &mut api.event_emissions {
        for argument in &mut event.arg_types {
            resolver.resolve(argument, MappingDirection::IntoLua, &event.event_name);
        }
    }

    if resolver.errors.is_empty() {
        Ok(FinishOutcome {
            used_mappings: resolver.used,
            requirements: resolver.requirements,
        })
    } else {
        Err(resolver.errors)
    }
}

struct TypeResolver<'a> {
    known_names: &'a HashSet<String>,
    conversions: &'a HashMap<String, ConversionContract>,
    evidence: &'a EvidenceCatalog,
    config: &'a ResolvedConfig,
    used: BTreeSet<MappingId>,
    requirements: BTreeSet<RequiredEvidence>,
    errors: Vec<TypeResolutionError>,
}

impl TypeResolver<'_> {
    fn resolve(&mut self, ty: &mut LuaType, direction: MappingDirection, lua_path: &str) {
        walk_lua_type_mut(ty, &mut |nested| {
            if matches!(nested, LuaType::Any) {
                self.errors.push(TypeResolutionError::UnsupportedStatic {
                    rust_type: "any".to_owned(),
                    lua_path: lua_path.to_owned(),
                    reason: "the extractor produced an unqualified dynamic fallback".to_owned(),
                    direction,
                });
                return;
            }
            if let LuaType::UnsupportedStatic { rust_type, reason } = nested {
                self.errors.push(TypeResolutionError::UnsupportedStatic {
                    rust_type: rust_type.clone(),
                    lua_path: lua_path.to_owned(),
                    reason: reason.clone(),
                    direction,
                });
                return;
            }
            if let LuaType::ExternalRust {
                path,
                arguments,
                stable_crate_id,
            } = nested
            {
                for argument in arguments.iter_mut() {
                    self.resolve(argument, direction, lua_path);
                }
                let rust_path = path.clone();
                let configured = self.config.mapping_for_rust_path(&rust_path);
                if let Some(class_name) = self.evidence.classes.get(&rust_path) {
                    if let Some(mapping) = configured {
                        self.errors.push(TypeResolutionError::ConflictingEvidence {
                            mapping: mapping.id().clone(),
                            rust_path,
                            lua_path: lua_path.to_owned(),
                            direction,
                            extracted: "replayed UserData",
                        });
                    } else {
                        *nested = LuaType::Class(class_name.clone());
                    }
                    return;
                }
                if let Some(contract) = self.evidence.conversions.get(&rust_path)
                    && let Some(domain) = conversion_domain(contract, direction)
                {
                    if let Some(mapping) = configured {
                        self.errors.push(TypeResolutionError::ConflictingEvidence {
                            mapping: mapping.id().clone(),
                            rust_path,
                            lua_path: lua_path.to_owned(),
                            direction,
                            extracted: "replayed conversion",
                        });
                    } else {
                        *nested = domain_to_lua_type(domain);
                    }
                    return;
                }
                if let Some(mapping) = configured {
                    match mapping.instantiate(direction, arguments) {
                        Ok(Some(mapped)) => {
                            self.used.insert(mapping.id().clone());
                            *nested = mapped;
                        }
                        Ok(None) => self.errors.push(TypeResolutionError::MissingDirection {
                            mapping: mapping.id().clone(),
                            rust_path,
                            lua_path: lua_path.to_owned(),
                            direction,
                        }),
                        Err(source) => self.errors.push(TypeResolutionError::InvalidMappingUse {
                            rust_path,
                            lua_path: lua_path.to_owned(),
                            direction,
                            source,
                        }),
                    }
                    return;
                }
                self.requirements.insert(RequiredEvidence {
                    stable_crate_id: *stable_crate_id,
                    rust_path: rust_path.clone(),
                    lua_path: lua_path.to_owned(),
                    direction: match direction {
                        MappingDirection::FromLua => EvidenceDirection::FromLua,
                        MappingDirection::IntoLua => EvidenceDirection::IntoLua,
                    },
                });
                *nested = LuaType::Class(
                    rust_path
                        .rsplit("::")
                        .next()
                        .unwrap_or(&rust_path)
                        .to_owned(),
                );
                return;
            }
            let LuaType::UnresolvedRust { path, arguments } = nested else {
                return;
            };
            for argument in arguments.iter_mut() {
                self.resolve(argument, direction, lua_path);
            }
            let rust_path = path.clone();
            let short_name = rust_path
                .rsplit("::")
                .next()
                .unwrap_or(&rust_path)
                .to_owned();
            let configured = self.config.mapping_for_rust_path(&rust_path);
            if self.known_names.contains(&short_name) {
                if let Some(mapping) = configured {
                    self.errors.push(TypeResolutionError::ConflictingEvidence {
                        mapping: mapping.id().clone(),
                        rust_path,
                        lua_path: lua_path.to_owned(),
                        direction,
                        extracted: "UserData",
                    });
                    return;
                }
                *nested = LuaType::Class(if arguments.is_empty() {
                    short_name
                } else {
                    format!(
                        "{}<{}>",
                        short_name,
                        arguments
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                });
                return;
            }
            if let Some(contract) = self.conversions.get(&rust_path)
                && let Some(domain) = conversion_domain(contract, direction)
            {
                if let Some(mapping) = configured {
                    self.errors.push(TypeResolutionError::ConflictingEvidence {
                        mapping: mapping.id().clone(),
                        rust_path,
                        lua_path: lua_path.to_owned(),
                        direction,
                        extracted: "conversion",
                    });
                    return;
                }
                *nested = domain_to_lua_type(domain);
                return;
            }
            let Some(mapping) = configured else {
                self.errors.push(TypeResolutionError::MissingEvidence {
                    rust_path,
                    lua_path: lua_path.to_owned(),
                    direction,
                });
                return;
            };
            match mapping.instantiate(direction, arguments) {
                Ok(Some(mapped)) => {
                    self.used.insert(mapping.id().clone());
                    *nested = mapped;
                }
                Ok(None) => self.errors.push(TypeResolutionError::MissingDirection {
                    mapping: mapping.id().clone(),
                    rust_path: rust_path.clone(),
                    lua_path: lua_path.to_owned(),
                    direction,
                }),
                Err(source) => self.errors.push(TypeResolutionError::InvalidMappingUse {
                    rust_path,
                    lua_path: lua_path.to_owned(),
                    direction,
                    source,
                }),
            }
        });

        if let LuaType::Class(name) = ty {
            if let Some(parsed) = parse_embedded_lua_type_text(name) {
                *ty = parsed;
            } else if let Some(base) = ref_alias_base_name(name, self.known_names) {
                *name = base;
            }
        }

        walk_lua_type_mut(ty, &mut |nested| {
            if let LuaType::Union(items) = nested {
                *nested = make_union(std::mem::take(items));
            }
        });

        if let LuaType::Class(name) = ty
            && let Some(parsed) = parse_embedded_lua_type_text(name)
        {
            *ty = parsed;
            self.resolve(ty, direction, lua_path);
        }
    }
}

fn conversion_domain(
    contract: &ConversionContract,
    direction: MappingDirection,
) -> Option<&LuaValueDomain> {
    match direction {
        MappingDirection::FromLua => contract.from_lua.as_ref(),
        MappingDirection::IntoLua => contract.into_lua.as_ref(),
    }
}

fn domain_to_lua_type(domain: &LuaValueDomain) -> LuaType {
    match domain {
        LuaValueDomain::Nil => LuaType::Nil,
        LuaValueDomain::Boolean => LuaType::Boolean,
        LuaValueDomain::Integer => LuaType::Integer,
        LuaValueDomain::Number => LuaType::Number,
        LuaValueDomain::String => LuaType::String,
        LuaValueDomain::Literal(value) => LuaType::Literal(value.clone()),
        LuaValueDomain::Table(shape) => LuaType::Record {
            fields: shape
                .fields
                .iter()
                .map(|field| LuaRecordField {
                    name: field.name.clone(),
                    ty: domain_to_lua_type(&field.value),
                })
                .collect(),
            open: shape.openness == TableOpenness::Open,
        },
        LuaValueDomain::UserData(name) => {
            LuaType::Class(name.rsplit("::").next().unwrap_or(name).to_owned())
        }
        LuaValueDomain::Function => LuaType::Function,
        LuaValueDomain::Thread => LuaType::Thread,
        LuaValueDomain::Union(items) => make_union(items.iter().map(domain_to_lua_type).collect()),
    }
}

fn parse_embedded_lua_type_text(text: &str) -> Option<LuaType> {
    let text = text.trim();
    let union_parts = split_top_level(text, '|');
    if union_parts.len() > 1 {
        return Some(make_union(
            union_parts
                .into_iter()
                .map(parse_embedded_lua_type_atom)
                .collect(),
        ));
    }

    None
}

fn parse_embedded_lua_type_atom(text: &str) -> LuaType {
    match text.trim() {
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
    }
}

fn ref_alias_base_name(name: &str, known_names: &HashSet<String>) -> Option<String> {
    if known_names.contains(name) || matches!(name, "UserDataRef" | "UserDataRefMut") {
        return None;
    }

    for suffix in ["RefMut", "Ref"] {
        if let Some(base) = name.strip_suffix(suffix)
            && !base.is_empty()
            && base
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_uppercase())
        {
            return Some(base.to_string());
        }
    }

    None
}

fn dedupe_fields(fields: &mut Vec<LuaField>) {
    let mut seen = HashSet::new();
    fields.retain(|field| seen.insert(field_key(field)));
}

fn dedupe_methods(methods: &mut Vec<LuaMethod>) {
    let mut seen = HashSet::new();
    methods.retain(|method| {
        let inserted = seen.insert(method_key(method));
        if !inserted && should_trace_method_name(&method.name) {
            trace(format!(
                "dedupe_methods dropped method={}",
                method_trace_summary(method)
            ));
        }
        inserted
    });
}

fn dedupe_functions(functions: &mut Vec<LuaFunction>) {
    let mut seen = HashSet::new();
    functions.retain(|function| seen.insert(function_key(function)));
}

fn is_synthetic_helper_module(module: &LuaModule) -> bool {
    let short_lowercase_name = !module.name.is_empty()
        && module.name.len() <= 2
        && module.name.chars().all(|c| c.is_ascii_lowercase());

    short_lowercase_name
        && module.functions.is_empty()
        && module
            .fields
            .iter()
            .all(|field| field.name.starts_with('_'))
}

fn method_key(method: &LuaMethod) -> String {
    format!(
        "{}|{:?}|{}|{:?}|{:?}",
        method.name, method.kind, method.is_async, method.params, method.returns
    )
}

fn function_key(function: &LuaFunction) -> String {
    format!(
        "{}|{}|{:?}|{:?}",
        function.name, function.is_async, function.params, function.returns
    )
}

fn field_key(field: &LuaField) -> String {
    format!(
        "{}|{:?}|{}|{:?}",
        field.name, field.ty, field.writable, field.doc
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use mlua_typegen::{
        LuaField, LuaFunction, LuaModule, LuaParam, LuaReturn, LuaType, MethodKind, make_union,
    };

    use super::*;

    fn normalize(ty: &mut LuaType, known: &HashSet<String>) {
        let conversions = HashMap::new();
        let evidence = EvidenceCatalog::default();
        let config = ResolvedConfig::empty();
        let mut resolver = TypeResolver {
            known_names: known,
            conversions: &conversions,
            evidence: &evidence,
            config: &config,
            used: BTreeSet::new(),
            requirements: BTreeSet::new(),
            errors: Vec::new(),
        };
        resolver.resolve(ty, MappingDirection::IntoLua, "<test>");
        assert!(resolver.errors.is_empty(), "{:?}", resolver.errors);
    }

    #[test]
    fn synthetic_helper_module_detection_handles_internal_wrapper_tables() {
        let internal = LuaModule {
            name: "ts".to_string(),
            doc: None,
            functions: Vec::new(),
            fields: vec![LuaField {
                name: "__mod".to_string(),
                ty: LuaType::Table,
                writable: true,
                doc: None,
            }],
        };

        let exported = LuaModule {
            name: "fs".to_string(),
            doc: None,
            functions: vec![LuaFunction {
                name: "cwd".to_string(),
                is_async: false,
                params: Vec::new(),
                returns: Vec::new(),
                doc: None,
            }],
            fields: Vec::new(),
        };

        assert!(is_synthetic_helper_module(&internal));
        assert!(!is_synthetic_helper_module(&exported));
    }

    #[test]
    fn api_type_normalization_rewrites_known_ref_aliases() {
        let mut ty = LuaType::Union(vec![
            LuaType::Class("UserDataRef".to_string()),
            LuaType::Class("FileRef".to_string()),
            LuaType::Integer,
        ]);
        let known = HashSet::from(["File".to_string()]);

        normalize(&mut ty, &known);

        assert_eq!(
            ty,
            make_union(vec![LuaType::Class("File".to_string()), LuaType::Integer])
        );
    }

    #[test]
    fn api_type_normalization_parses_embedded_union_text() {
        let mut ty = LuaType::Class("UserDataRef | FileRef | integer".to_string());
        let known = HashSet::new();

        normalize(&mut ty, &known);

        assert_eq!(
            ty,
            make_union(vec![LuaType::Class("File".to_string()), LuaType::Integer])
        );
    }

    #[test]
    fn finish_api_dedupes_nested_members() {
        let mut api = ExtractedApi::new(mlua_typegen::Provenance::explicit("test"));
        api.classes = vec![mlua_typegen::LuaClass {
            name: "Vec2".to_string(),
            doc: None,
            fields: vec![
                LuaField {
                    name: "x".to_string(),
                    ty: LuaType::Number,
                    writable: true,
                    doc: None,
                },
                LuaField {
                    name: "x".to_string(),
                    ty: LuaType::Number,
                    writable: true,
                    doc: None,
                },
            ],
            methods: vec![
                mlua_typegen::LuaMethod {
                    name: "dot".to_string(),
                    kind: MethodKind::Method,
                    is_async: false,
                    params: vec![LuaParam {
                        name: "other".to_string(),
                        ty: LuaType::Class("Vec2".to_string()),
                    }],
                    returns: vec![LuaReturn::from(LuaType::Number)],
                    doc: None,
                },
                mlua_typegen::LuaMethod {
                    name: "dot".to_string(),
                    kind: MethodKind::Method,
                    is_async: false,
                    params: vec![LuaParam {
                        name: "other".to_string(),
                        ty: LuaType::Class("Vec2".to_string()),
                    }],
                    returns: vec![LuaReturn::from(LuaType::Number)],
                    doc: None,
                },
            ],
        }];

        finish_api(
            &mut api,
            &ResolvedConfig::empty(),
            &EvidenceCatalog::default(),
        )
        .unwrap();

        assert_eq!(api.classes[0].fields.len(), 1);
        assert_eq!(api.classes[0].methods.len(), 1);
    }
}
