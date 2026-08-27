//! Explicit, workspace-wide Rust-to-Lua type mapping configuration.

mod expression;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use cargo_metadata::{Metadata, Package};
use schemars::JsonSchema;
use semver::VersionReq;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use expression::{ExpressionError, InstantiateError, TypeTemplate};

pub const DEFAULT_CONFIG_FILE: &str = "mlua-typegen.toml";
pub const CONFIG_SNAPSHOT_ENV: &str = "MLUA_TYPEGEN_CONFIG_SNAPSHOT";
pub const SCHEMA_URL: &str = "https://raw.githubusercontent.com/themixednuts/mlua-typegen/main/schemas/mlua-typegen.schema.json";

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default)]
    pub mappings: Vec<ConfigMapping>,
    #[serde(default)]
    pub external_types: Vec<ExternalType>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ConfigMapping {
    pub id: String,
    pub rust: RustSelector,
    #[serde(default)]
    pub params: Vec<String>,
    pub lua: Option<String>,
    pub from_lua: Option<String>,
    pub into_lua: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RustSelector {
    pub package: String,
    pub version: Option<String>,
    pub source: Option<String>,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExternalType {
    pub name: String,
    #[serde(default)]
    pub params: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MappingId(String);

impl MappingId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MappingId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingDirection {
    FromLua,
    IntoLua,
}

#[derive(Debug, Clone)]
pub struct ParsedConfig {
    mappings: Vec<ParsedMapping>,
    external_types: Vec<ResolvedExternalType>,
}

#[derive(Debug, Clone)]
struct ParsedMapping {
    id: MappingId,
    selector: RustSelector,
    parameters: Vec<String>,
    contract: MappingContract,
}

impl ParsedConfig {
    pub fn parse(source: &str) -> Result<Self, ConfigErrors> {
        let raw: ConfigFile = toml::from_str(source).map_err(ConfigErrors::toml)?;
        validate_structure(raw)
    }

    pub fn resolve(self, metadata: &Metadata) -> Result<ResolvedConfig, ConfigErrors> {
        let mut problems = Vec::new();
        let mut selector_owners = BTreeMap::<(String, String), String>::new();
        let mut mappings = Vec::new();

        for mapping in self.mappings {
            let id = mapping.id.as_str();
            let Some(selector) = resolve_selector(id, &mapping.selector, metadata, &mut problems)
            else {
                continue;
            };
            let key = (selector.package_id.clone(), selector.path.clone());
            if let Some(first) = selector_owners.insert(key, id.to_owned()) {
                problems.push(ConfigProblem::DuplicateSelector {
                    first,
                    second: id.to_owned(),
                });
                continue;
            }
            mappings.push(ResolvedMapping {
                id: mapping.id,
                selector,
                parameters: mapping.parameters,
                contract: mapping.contract,
            });
        }

        if problems.is_empty() {
            mappings.sort_by(|left, right| left.id.cmp(&right.id));
            Ok(ResolvedConfig {
                mappings,
                external_types: self.external_types,
            })
        } else {
            Err(ConfigErrors(problems))
        }
    }
}

impl fmt::Display for MappingDirection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::FromLua => "FromLua",
            Self::IntoLua => "IntoLua",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedConfig {
    mappings: Vec<ResolvedMapping>,
    external_types: Vec<ResolvedExternalType>,
}

impl ResolvedConfig {
    pub fn empty() -> Self {
        Self {
            mappings: Vec::new(),
            external_types: Vec::new(),
        }
    }

    pub fn parse(source: &str, metadata: &Metadata) -> Result<Self, ConfigErrors> {
        ParsedConfig::parse(source)?.resolve(metadata)
    }

    pub fn from_snapshot(path: &Path) -> Result<Self, SnapshotError> {
        let bytes = std::fs::read(path).map_err(|source| SnapshotError::Read {
            path: path.to_owned(),
            source,
        })?;
        serde_json::from_slice(&bytes).map_err(|source| SnapshotError::Decode {
            path: path.to_owned(),
            source,
        })
    }

    pub fn snapshot_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    pub fn mappings(&self) -> &[ResolvedMapping] {
        &self.mappings
    }

    pub fn external_types(&self) -> &[ResolvedExternalType] {
        &self.external_types
    }

    pub fn mapping_ids(&self) -> impl Iterator<Item = &MappingId> {
        self.mappings.iter().map(|mapping| &mapping.id)
    }

    pub fn mapping_for_rust_path(&self, canonical_path: &str) -> Option<&ResolvedMapping> {
        self.mappings
            .iter()
            .find(|mapping| mapping.selector.canonical_rust_path() == canonical_path)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedMapping {
    id: MappingId,
    selector: ResolvedSelector,
    parameters: Vec<String>,
    contract: MappingContract,
}

impl ResolvedMapping {
    pub fn id(&self) -> &MappingId {
        &self.id
    }

    pub fn selector(&self) -> &ResolvedSelector {
        &self.selector
    }

    pub fn parameters(&self) -> &[String] {
        &self.parameters
    }

    pub fn template(&self, direction: MappingDirection) -> Option<&TypeTemplate> {
        match (&self.contract, direction) {
            (MappingContract::Symmetric(template), _) => Some(template),
            (MappingContract::Directional { from_lua, .. }, MappingDirection::FromLua) => {
                from_lua.as_ref()
            }
            (MappingContract::Directional { into_lua, .. }, MappingDirection::IntoLua) => {
                into_lua.as_ref()
            }
        }
    }

    pub fn instantiate(
        &self,
        direction: MappingDirection,
        arguments: &[crate::LuaType],
    ) -> Result<Option<crate::LuaType>, MappingUseError> {
        if arguments.len() != self.parameters.len() {
            return Err(MappingUseError::GenericArity {
                mapping: self.id.clone(),
                expected: self.parameters.len(),
                actual: arguments.len(),
            });
        }
        let parameters = self
            .parameters
            .iter()
            .zip(arguments)
            .map(|(name, argument)| (name.as_str(), argument))
            .collect::<BTreeMap<_, _>>();
        self.template(direction)
            .map(|template| instantiate_template(template, &parameters))
            .transpose()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MappingUseError {
    #[error("mapping `{mapping}` expects {expected} Rust type arguments, found {actual}")]
    GenericArity {
        mapping: MappingId,
        expected: usize,
        actual: usize,
    },
}

fn instantiate_template(
    template: &TypeTemplate,
    parameters: &BTreeMap<&str, &crate::LuaType>,
) -> Result<crate::LuaType, MappingUseError> {
    use crate::{LuaLiteral, LuaType, make_union};

    Ok(match template {
        TypeTemplate::Nil => LuaType::Nil,
        TypeTemplate::Boolean => LuaType::Boolean,
        TypeTemplate::Integer => LuaType::Integer,
        TypeTemplate::Number => LuaType::Number,
        TypeTemplate::String => LuaType::String,
        TypeTemplate::Table => LuaType::Table,
        TypeTemplate::Function => LuaType::Function,
        TypeTemplate::Thread => LuaType::Thread,
        TypeTemplate::Literal(value) => LuaType::Literal(match value {
            expression::LuaLiteralTemplate::Boolean(value) => LuaLiteral::Boolean(*value),
            expression::LuaLiteralTemplate::Integer(value) => LuaLiteral::Integer(*value),
            expression::LuaLiteralTemplate::Number(value) => LuaLiteral::Number(*value),
            expression::LuaLiteralTemplate::String(value) => LuaLiteral::String(value.clone()),
        }),
        TypeTemplate::Named { name, arguments } => {
            let arguments = arguments
                .iter()
                .map(|argument| instantiate_template(argument, parameters))
                .collect::<Result<Vec<_>, _>>()?;
            if arguments.is_empty() {
                LuaType::Class(name.clone())
            } else {
                LuaType::Class(format!(
                    "{}<{}>",
                    name,
                    arguments
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        }
        TypeTemplate::Parameter(name) => (*parameters
            .get(name.as_str())
            .expect("mapping parameter names were structurally validated"))
        .clone(),
        TypeTemplate::Array(inner) => {
            LuaType::Array(Box::new(instantiate_template(inner, parameters)?))
        }
        TypeTemplate::Optional(inner) => {
            LuaType::Optional(Box::new(instantiate_template(inner, parameters)?))
        }
        TypeTemplate::Map { key, value } => LuaType::Map(
            Box::new(instantiate_template(key, parameters)?),
            Box::new(instantiate_template(value, parameters)?),
        ),
        TypeTemplate::Union(items) => make_union(
            items
                .iter()
                .map(|item| instantiate_template(item, parameters))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        TypeTemplate::FunctionSignature {
            parameters: inputs,
            returns,
        } => LuaType::FunctionSig {
            params: inputs
                .iter()
                .map(|input| instantiate_template(input, parameters))
                .collect::<Result<Vec<_>, _>>()?,
            returns: returns
                .iter()
                .map(|output| instantiate_template(output, parameters))
                .collect::<Result<Vec<_>, _>>()?,
        },
        TypeTemplate::Dynamic(reason) => LuaType::Dynamic(reason.clone()),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedSelector {
    package_id: String,
    package: String,
    version: String,
    source: Option<String>,
    crate_name: String,
    path: String,
}

impl ResolvedSelector {
    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn package(&self) -> &str {
        &self.package
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    pub fn crate_name(&self) -> &str {
        &self.crate_name
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn canonical_rust_path(&self) -> String {
        format!("{}::{}", self.crate_name, self.path)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum MappingContract {
    Symmetric(TypeTemplate),
    Directional {
        from_lua: Option<TypeTemplate>,
        into_lua: Option<TypeTemplate>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedExternalType {
    name: String,
    parameters: Vec<String>,
}

impl ResolvedExternalType {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn arity(&self) -> usize {
        self.parameters.len()
    }
}

#[derive(Debug)]
pub struct ConfigErrors(Vec<ConfigProblem>);

impl ConfigErrors {
    fn toml(error: toml::de::Error) -> Self {
        Self(vec![ConfigProblem::Toml(error.to_string())])
    }
}

impl fmt::Display for ConfigErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, problem) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }
            fmt::Display::fmt(problem, formatter)?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigErrors {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigProblem {
    Toml(String),
    EmptyMappingId {
        index: usize,
    },
    DuplicateMappingId(String),
    EmptySelectorField {
        id: String,
        field: &'static str,
    },
    InvalidVersion {
        id: String,
        value: String,
    },
    PackageNotFound {
        id: String,
        selector: String,
    },
    AmbiguousPackage {
        id: String,
        matches: Vec<String>,
    },
    PackageHasNoLibrary {
        id: String,
        package: String,
    },
    DuplicateSelector {
        first: String,
        second: String,
    },
    EmptyDirection {
        id: String,
    },
    MixedDirection {
        id: String,
    },
    InvalidExpression {
        id: String,
        direction: &'static str,
        error: String,
    },
    EmptyParameter {
        owner: String,
    },
    DuplicateParameter {
        owner: String,
        parameter: String,
    },
    UndeclaredParameter {
        id: String,
        parameter: String,
    },
    InvalidTypeName {
        owner: String,
        name: String,
    },
    DuplicateExternalType(String),
}

impl fmt::Display for ConfigProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Toml(error) => write!(formatter, "invalid TOML configuration: {error}"),
            Self::EmptyMappingId { index } => write!(formatter, "mapping #{index} has an empty id"),
            Self::DuplicateMappingId(id) => write!(formatter, "mapping id `{id}` is duplicated"),
            Self::EmptySelectorField { id, field } => {
                write!(
                    formatter,
                    "mapping `{id}` has an empty Rust selector `{field}`"
                )
            }
            Self::InvalidVersion { id, value } => {
                write!(
                    formatter,
                    "mapping `{id}` has invalid version requirement `{value}`"
                )
            }
            Self::PackageNotFound { id, selector } => {
                write!(
                    formatter,
                    "mapping `{id}` matches no resolved Cargo package: {selector}"
                )
            }
            Self::AmbiguousPackage { id, matches } => write!(
                formatter,
                "mapping `{id}` matches multiple Cargo packages: {}",
                matches.join(", ")
            ),
            Self::PackageHasNoLibrary { id, package } => write!(
                formatter,
                "mapping `{id}` selects package `{package}`, which has no library target"
            ),
            Self::DuplicateSelector { first, second } => write!(
                formatter,
                "mappings `{first}` and `{second}` select the same Rust definition"
            ),
            Self::EmptyDirection { id } => write!(
                formatter,
                "directional mapping `{id}` must declare `from_lua`, `into_lua`, or both"
            ),
            Self::MixedDirection { id } => write!(
                formatter,
                "mapping `{id}` cannot combine `lua` with `from_lua` or `into_lua`"
            ),
            Self::InvalidExpression {
                id,
                direction,
                error,
            } => write!(
                formatter,
                "mapping `{id}` has an invalid {direction} expression: {error}"
            ),
            Self::EmptyParameter { owner } => {
                write!(formatter, "{owner} declares an empty type parameter")
            }
            Self::DuplicateParameter { owner, parameter } => {
                write!(
                    formatter,
                    "{owner} declares type parameter `{parameter}` more than once"
                )
            }
            Self::UndeclaredParameter { id, parameter } => write!(
                formatter,
                "mapping `{id}` references undeclared Rust parameter `${parameter}`"
            ),
            Self::InvalidTypeName { owner, name } => {
                write!(formatter, "{owner} has invalid Lua type name `{name}`")
            }
            Self::DuplicateExternalType(name) => {
                write!(formatter, "external Lua type `{name}` is duplicated")
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("failed to read configuration snapshot `{}`", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("configuration snapshot `{}` is invalid", path.display())]
    Decode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

pub fn schema_json() -> Result<String, serde_json::Error> {
    let mut schema = schemars::schema_for!(ConfigFile);
    schema.insert(
        "$id".to_owned(),
        serde_json::Value::String(SCHEMA_URL.to_owned()),
    );
    serde_json::to_string_pretty(&schema).map(|mut json| {
        json.push('\n');
        json
    })
}

fn validate_structure(raw: ConfigFile) -> Result<ParsedConfig, ConfigErrors> {
    let mut problems = Vec::new();
    let mut mapping_ids = BTreeSet::new();
    let mut mappings = Vec::new();

    for (index, mapping) in raw.mappings.into_iter().enumerate() {
        let id_text = mapping.id.trim().to_owned();
        if id_text.is_empty() {
            problems.push(ConfigProblem::EmptyMappingId { index: index + 1 });
            continue;
        }
        if !mapping_ids.insert(id_text.clone()) {
            problems.push(ConfigProblem::DuplicateMappingId(id_text));
            continue;
        }

        validate_selector_fields(&id_text, &mapping.rust, &mut problems);
        validate_parameters(
            &format!("mapping `{id_text}`"),
            &mapping.params,
            &mut problems,
        );

        let contract = parse_contract(
            &id_text,
            mapping.lua,
            mapping.from_lua,
            mapping.into_lua,
            &mapping.params,
            &mut problems,
        );
        let Some(contract) = contract else {
            continue;
        };

        mappings.push(ParsedMapping {
            id: MappingId(id_text),
            selector: mapping.rust,
            parameters: mapping.params,
            contract,
        });
    }

    let mut external_names = BTreeSet::new();
    let mut external_types = Vec::new();
    for external in raw.external_types {
        let name = external.name.trim().to_owned();
        if !valid_type_name(&name) {
            problems.push(ConfigProblem::InvalidTypeName {
                owner: "external type".to_owned(),
                name,
            });
            continue;
        }
        if !external_names.insert(name.clone()) {
            problems.push(ConfigProblem::DuplicateExternalType(name));
            continue;
        }
        validate_parameters(
            &format!("external type `{name}`"),
            &external.params,
            &mut problems,
        );
        external_types.push(ResolvedExternalType {
            name,
            parameters: external.params,
        });
    }

    if problems.is_empty() {
        external_types.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(ParsedConfig {
            mappings,
            external_types,
        })
    } else {
        Err(ConfigErrors(problems))
    }
}

fn validate_selector_fields(id: &str, selector: &RustSelector, problems: &mut Vec<ConfigProblem>) {
    for (field, value) in [("package", &selector.package), ("path", &selector.path)] {
        if value.trim().is_empty() {
            problems.push(ConfigProblem::EmptySelectorField {
                id: id.to_owned(),
                field,
            });
        }
    }
}

fn parse_contract(
    id: &str,
    lua: Option<String>,
    from_lua: Option<String>,
    into_lua: Option<String>,
    parameters: &[String],
    problems: &mut Vec<ConfigProblem>,
) -> Option<MappingContract> {
    if lua.is_some() && (from_lua.is_some() || into_lua.is_some()) {
        problems.push(ConfigProblem::MixedDirection { id: id.to_owned() });
        return None;
    }
    let parsed = if let Some(lua) = lua {
        parse_expression(id, "symmetric", &lua, problems).map(MappingContract::Symmetric)
    } else if from_lua.is_some() || into_lua.is_some() {
        let from_lua = from_lua
            .as_deref()
            .and_then(|value| parse_expression(id, "from_lua", value, problems));
        let into_lua = into_lua
            .as_deref()
            .and_then(|value| parse_expression(id, "into_lua", value, problems));
        Some(MappingContract::Directional { from_lua, into_lua })
    } else {
        problems.push(ConfigProblem::EmptyDirection { id: id.to_owned() });
        return None;
    };

    if let Some(contract) = &parsed {
        let mut referenced = Vec::new();
        match contract {
            MappingContract::Symmetric(template) => template.referenced_parameters(&mut referenced),
            MappingContract::Directional { from_lua, into_lua } => {
                for template in from_lua.iter().chain(into_lua) {
                    template.referenced_parameters(&mut referenced);
                }
            }
        }
        for parameter in referenced {
            if !parameters.contains(&parameter) {
                problems.push(ConfigProblem::UndeclaredParameter {
                    id: id.to_owned(),
                    parameter,
                });
            }
        }
    }
    parsed
}

fn parse_expression(
    id: &str,
    direction: &'static str,
    source: &str,
    problems: &mut Vec<ConfigProblem>,
) -> Option<TypeTemplate> {
    match TypeTemplate::parse(source) {
        Ok(template) => Some(template),
        Err(error) => {
            problems.push(ConfigProblem::InvalidExpression {
                id: id.to_owned(),
                direction,
                error: error.to_string(),
            });
            None
        }
    }
}

fn resolve_selector(
    id: &str,
    selector: &RustSelector,
    metadata: &Metadata,
    problems: &mut Vec<ConfigProblem>,
) -> Option<ResolvedSelector> {
    let requirement = match selector.version.as_deref() {
        Some(value) => match VersionReq::parse(value) {
            Ok(requirement) => Some(requirement),
            Err(_) => {
                problems.push(ConfigProblem::InvalidVersion {
                    id: id.to_owned(),
                    value: value.to_owned(),
                });
                return None;
            }
        },
        None => None,
    };

    let candidates = metadata
        .packages
        .iter()
        .filter(|package| package.name.as_str() == selector.package)
        .filter(|package| {
            requirement
                .as_ref()
                .is_none_or(|requirement| requirement.matches(&package.version))
        })
        .filter(|package| {
            selector.source.as_ref().is_none_or(|source| {
                package
                    .source
                    .as_ref()
                    .is_some_and(|candidate| candidate.to_string() == *source)
            })
        })
        .collect::<Vec<_>>();

    if candidates.is_empty() {
        problems.push(ConfigProblem::PackageNotFound {
            id: id.to_owned(),
            selector: selector_description(selector),
        });
        return None;
    }
    if candidates.len() > 1 {
        problems.push(ConfigProblem::AmbiguousPackage {
            id: id.to_owned(),
            matches: candidates
                .iter()
                .map(|package| package.id.to_string())
                .collect(),
        });
        return None;
    }
    package_selector(id, selector, candidates[0], problems)
}

fn package_selector(
    id: &str,
    selector: &RustSelector,
    package: &Package,
    problems: &mut Vec<ConfigProblem>,
) -> Option<ResolvedSelector> {
    let Some(target) = package.targets.iter().find(|target| target.is_lib()) else {
        problems.push(ConfigProblem::PackageHasNoLibrary {
            id: id.to_owned(),
            package: package.id.to_string(),
        });
        return None;
    };
    let crate_name = target.name.replace('-', "_");
    let mut path = selector.path.trim().trim_start_matches("::").to_owned();
    if let Some(relative) = path.strip_prefix(&format!("{crate_name}::")) {
        path = relative.to_owned();
    }
    Some(ResolvedSelector {
        package_id: package.id.to_string(),
        package: package.name.to_string(),
        version: package.version.to_string(),
        source: package.source.as_ref().map(ToString::to_string),
        crate_name,
        path,
    })
}

fn selector_description(selector: &RustSelector) -> String {
    let mut output = selector.package.clone();
    if let Some(version) = &selector.version {
        output.push('@');
        output.push_str(version);
    }
    if let Some(source) = &selector.source {
        output.push_str(" from ");
        output.push_str(source);
    }
    output
}

fn validate_parameters(owner: &str, parameters: &[String], problems: &mut Vec<ConfigProblem>) {
    let mut seen = BTreeSet::new();
    for parameter in parameters {
        if parameter.trim().is_empty() {
            problems.push(ConfigProblem::EmptyParameter {
                owner: owner.to_owned(),
            });
        } else if !valid_identifier(parameter) {
            problems.push(ConfigProblem::InvalidTypeName {
                owner: owner.to_owned(),
                name: parameter.clone(),
            });
        } else if !seen.insert(parameter) {
            problems.push(ConfigProblem::DuplicateParameter {
                owner: owner.to_owned(),
                parameter: parameter.clone(),
            });
        }
    }
}

fn valid_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_alphabetic())
        && characters.all(|character| character == '_' || character.is_alphanumeric())
}

fn valid_type_name(value: &str) -> bool {
    !value.is_empty() && value.split('.').all(valid_identifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_has_stable_id_and_no_numeric_version_field() {
        let schema = schema_json().unwrap();
        assert!(schema.contains(SCHEMA_URL));
        assert!(!schema.contains("schema_version"));
    }

    #[test]
    fn checked_in_schema_matches_canonical_types() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/mlua-typegen.schema.json");
        let checked_in = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "failed to read generated schema `{}`: {error}",
                path.display()
            )
        });
        assert_eq!(
            checked_in,
            schema_json().unwrap(),
            "run `cargo run --example generate-config-schema`"
        );
    }

    #[test]
    fn contract_shape_rejects_symmetric_and_directional_mix() {
        let error = ParsedConfig::parse(
            r#"
            [[mappings]]
            id = "bad"
            rust = { package = "pkg", path = "Thing" }
            lua = "string"
            into_lua = "integer"
            "#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("cannot combine"));
    }

    #[test]
    fn resolves_package_identity_and_instantiates_generic_template() {
        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .exec()
            .unwrap();
        let config = ResolvedConfig::parse(
            r#"
            [[mappings]]
            id = "local-box"
            rust = { package = "mlua-typegen", path = "DomainBox" }
            params = ["T"]
            lua = "$T[]"
            "#,
            &metadata,
        )
        .unwrap();

        let mapping = &config.mappings()[0];
        assert_eq!(mapping.selector().crate_name(), "mlua_typegen");
        assert_eq!(mapping.selector().path(), "DomainBox");
        let parameters = BTreeMap::from([("T".to_owned(), TypeTemplate::String)]);
        assert_eq!(
            mapping
                .template(MappingDirection::IntoLua)
                .unwrap()
                .instantiate(&parameters)
                .unwrap(),
            TypeTemplate::Array(Box::new(TypeTemplate::String))
        );
    }
}
