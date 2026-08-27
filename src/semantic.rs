//! Canonical Lua API semantics, independent of extraction and annotation syntax.

use std::collections::BTreeMap;
use std::fmt;

use crate::{
    EventEmission, ExtractedApi, LuaClass, LuaEnum, LuaField, LuaFunction, LuaLiteral, LuaMethod,
    LuaName, LuaPath, LuaType, MethodKind,
};

mod lower;
mod validate;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeId(String);

impl TypeId {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TypeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeParameterId(String);

impl TypeParameterId {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TypeParameterId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Provenance {
    RustCrate { crate_name: String },
    Explicit { source: String },
}

impl Provenance {
    #[must_use]
    pub fn rust_crate(crate_name: impl Into<String>) -> Self {
        Self::RustCrate {
            crate_name: crate_name.into(),
        }
    }

    #[must_use]
    pub fn explicit(source: impl Into<String>) -> Self {
        Self::Explicit {
            source: source.into(),
        }
    }
}

impl fmt::Display for Provenance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RustCrate { crate_name } => write!(formatter, "Rust crate `{crate_name}`"),
            Self::Explicit { source } => formatter.write_str(source),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Origins {
    first: Provenance,
    rest: Vec<Provenance>,
}

impl Origins {
    #[must_use]
    pub fn one(origin: Provenance) -> Self {
        Self {
            first: origin,
            rest: Vec::new(),
        }
    }

    pub fn insert(&mut self, origin: Provenance) {
        if self.first != origin && !self.rest.contains(&origin) {
            self.rest.push(origin);
            self.rest.sort();
        }
    }

    pub fn extend(&mut self, other: &Self) {
        for origin in other.iter() {
            self.insert(origin.clone());
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Provenance> {
        std::iter::once(&self.first).chain(&self.rest)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TypeParameter {
    pub id: TypeParameterId,
    pub constraint: Option<TypeExpr>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum TypeExpr {
    Nil,
    Boolean,
    Integer,
    Number,
    String,
    Table,
    Function(Box<Signature>),
    Any,
    Dynamic {
        reason: String,
    },
    Thread,
    Literal(LuaLiteral),
    Named {
        id: TypeId,
        arguments: Vec<TypeExpr>,
    },
    Parameter(TypeParameterId),
    Array(Box<TypeExpr>),
    Map {
        key: Box<TypeExpr>,
        value: Box<TypeExpr>,
    },
    Record {
        fields: Vec<(LuaName, TypeExpr)>,
        open: bool,
    },
    Union(Vec<TypeExpr>),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Parameter {
    pub name: String,
    pub ty: TypeExpr,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReturnValue {
    pub name: Option<String>,
    pub ty: TypeExpr,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Signature {
    pub docs: Vec<String>,
    pub type_parameters: Vec<TypeParameter>,
    pub inputs: Vec<Parameter>,
    pub variadic: Option<Box<TypeExpr>>,
    pub outputs: Vec<ReturnValue>,
    pub is_async: bool,
    pub receiver: Option<MethodKind>,
    pub origins: Origins,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverloadSet {
    signatures: Vec<Signature>,
}

impl OverloadSet {
    #[must_use]
    pub fn one(signature: Signature) -> Self {
        Self {
            signatures: vec![signature],
        }
    }

    pub fn insert(&mut self, signature: Signature) {
        if let Some(existing) = self
            .signatures
            .iter_mut()
            .find(|existing| same_signature(existing, &signature))
        {
            existing.origins.extend(&signature.origins);
            for doc in &signature.docs {
                if !existing.docs.contains(doc) {
                    existing.docs.push(doc.clone());
                }
            }
            existing.docs.sort();
        } else {
            self.signatures.push(signature);
            self.signatures.sort();
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Signature> {
        self.signatures.iter()
    }

    #[must_use]
    pub fn primary(&self) -> &Signature {
        self.signatures
            .first()
            .expect("an overload set is non-empty by construction")
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.signatures.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }
}

fn same_signature(left: &Signature, right: &Signature) -> bool {
    left.type_parameters == right.type_parameters
        && left.inputs == right.inputs
        && left.variadic == right.variadic
        && left.outputs == right.outputs
        && left.is_async == right.is_async
        && left.receiver == right.receiver
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDef {
    pub ty: TypeExpr,
    pub writable: bool,
    pub doc: Option<String>,
    pub origins: Origins,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassDef {
    pub parameters: Vec<TypeParameter>,
    pub fields: BTreeMap<LuaName, FieldDef>,
    pub methods: BTreeMap<LuaName, OverloadSet>,
    pub doc: Option<String>,
    pub origins: Origins,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasDef {
    pub parameters: Vec<TypeParameter>,
    pub target: TypeExpr,
    pub constructors: BTreeMap<LuaName, TypeExpr>,
    pub doc: Option<String>,
    pub origins: Origins,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeDef {
    Class(ClassDef),
    Alias(AliasDef),
}

impl TypeDef {
    #[must_use]
    pub fn origins(&self) -> &Origins {
        match self {
            Self::Class(definition) => &definition.origins,
            Self::Alias(definition) => &definition.origins,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDef {
    pub fields: BTreeMap<LuaName, FieldDef>,
    pub functions: BTreeMap<LuaName, OverloadSet>,
    pub doc: Option<String>,
    pub origins: Origins,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueDef {
    Table(TableDef),
    Function(OverloadSet),
    Typed(FieldDef),
}

/// One owned fact submitted to the canonical semantic assembler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiContribution {
    Type { id: TypeId, definition: TypeDef },
    Value { path: LuaPath, definition: ValueDef },
}

impl ValueDef {
    #[must_use]
    pub fn origins(&self) -> &Origins {
        match self {
            Self::Table(definition) => &definition.origins,
            Self::Function(overloads) => &overloads.primary().origins,
            Self::Typed(definition) => &definition.origins,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticApi {
    types: BTreeMap<TypeId, TypeDef>,
    values: BTreeMap<LuaPath, ValueDef>,
}

impl SemanticApi {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            types: BTreeMap::new(),
            values: BTreeMap::new(),
        }
    }

    pub fn assemble(
        contributions: impl IntoIterator<Item = ApiContribution>,
    ) -> Result<Self, SemanticErrors> {
        let mut api = Self::empty();
        let mut errors = Vec::new();

        for contribution in contributions {
            match contribution {
                ApiContribution::Type { id, definition } => {
                    api.insert_type(id, definition, &mut errors);
                }
                ApiContribution::Value { path, definition } => {
                    api.insert_value(path, definition, &mut errors);
                }
            }
        }

        validate::graph(&api, &mut errors);

        if errors.is_empty() {
            Ok(api)
        } else {
            Err(SemanticErrors(errors))
        }
    }

    pub fn types(&self) -> impl Iterator<Item = (&TypeId, &TypeDef)> {
        self.types.iter()
    }

    pub fn values(&self) -> impl Iterator<Item = (&LuaPath, &ValueDef)> {
        self.values.iter()
    }

    #[must_use]
    pub fn type_definition(&self, id: &TypeId) -> Option<&TypeDef> {
        self.types.get(id)
    }

    #[must_use]
    pub fn value(&self, path: &LuaPath) -> Option<&ValueDef> {
        self.values.get(path)
    }

    #[must_use]
    pub fn item_count(&self) -> usize {
        self.types.len() + self.values.len()
    }

    fn insert_type(&mut self, id: TypeId, incoming: TypeDef, errors: &mut Vec<SemanticError>) {
        let second = incoming.origins().clone();
        let Some(existing) = self.types.get_mut(&id) else {
            self.types.insert(id, incoming);
            return;
        };

        if !merge_type(existing, incoming) {
            errors.push(SemanticError::ConflictingType {
                id,
                first: existing.origins().clone(),
                second,
            });
        }
    }

    fn insert_value(&mut self, path: LuaPath, incoming: ValueDef, errors: &mut Vec<SemanticError>) {
        let second = incoming.origins().clone();
        let Some(existing) = self.values.get_mut(&path) else {
            self.values.insert(path, incoming);
            return;
        };

        if !merge_value(existing, incoming) {
            errors.push(SemanticError::ConflictingValue {
                path,
                first: existing.origins().clone(),
                second,
            });
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticError {
    ConflictingType {
        id: TypeId,
        first: Origins,
        second: Origins,
    },
    ConflictingValue {
        path: LuaPath,
        first: Origins,
        second: Origins,
    },
    GenericArity {
        referenced: TypeId,
        expected: usize,
        actual: usize,
        origins: Origins,
    },
}

impl fmt::Display for SemanticError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConflictingType { id, .. } => {
                write!(formatter, "conflicting definitions for type `{id}`")
            }
            Self::ConflictingValue { path, .. } => {
                write!(formatter, "conflicting definitions for Lua path {path}")
            }
            Self::GenericArity {
                referenced,
                expected,
                actual,
                ..
            } => write!(
                formatter,
                "type `{referenced}` expects {expected} generic arguments, found {actual}"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticErrors(Vec<SemanticError>);

impl SemanticErrors {
    pub fn iter(&self) -> impl Iterator<Item = &SemanticError> {
        self.0.iter()
    }
}

impl fmt::Display for SemanticErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.0.iter().enumerate() {
            if index != 0 {
                formatter.write_str("; ")?;
            }
            fmt::Display::fmt(error, formatter)?;
        }
        Ok(())
    }
}

impl std::error::Error for SemanticErrors {}

#[derive(Debug, Clone)]
pub struct ExtractedUnit {
    pub api: SemanticApi,
    pub events: Vec<EventEmission>,
    pub conversions: Vec<crate::ConversionContract>,
}

impl ExtractedUnit {
    pub fn assemble(extracted: ExtractedApi) -> Result<Self, SemanticErrors> {
        let api = SemanticApi::assemble(lower::contributions(&extracted))?;
        Ok(Self {
            api,
            events: extracted.event_emissions,
            conversions: extracted.conversions,
        })
    }
}

fn merge_type(existing: &mut TypeDef, incoming: TypeDef) -> bool {
    match (existing, incoming) {
        (TypeDef::Class(existing), TypeDef::Class(incoming)) => merge_class(existing, incoming),
        (TypeDef::Alias(existing), TypeDef::Alias(incoming))
            if existing.target == incoming.target =>
        {
            existing.origins.extend(&incoming.origins);
            for (name, constructor) in incoming.constructors {
                existing.constructors.entry(name).or_insert(constructor);
            }
            true
        }
        _ => false,
    }
}

fn merge_class(existing: &mut ClassDef, incoming: ClassDef) -> bool {
    if existing.parameters != incoming.parameters || !compatible_doc(&existing.doc, &incoming.doc) {
        return false;
    }
    if !merge_fields(&mut existing.fields, incoming.fields) {
        return false;
    }
    for (name, overloads) in incoming.methods {
        if let Some(existing) = existing.methods.get_mut(&name) {
            for signature in overloads.iter() {
                existing.insert(signature.clone());
            }
        } else {
            existing.methods.insert(name, overloads);
        }
    }
    existing.origins.extend(&incoming.origins);
    if existing.doc.is_none() {
        existing.doc = incoming.doc;
    }
    true
}

fn merge_value(existing: &mut ValueDef, incoming: ValueDef) -> bool {
    match (existing, incoming) {
        (ValueDef::Table(existing), ValueDef::Table(incoming)) => merge_table(existing, incoming),
        (ValueDef::Function(existing), ValueDef::Function(incoming)) => {
            for signature in incoming.iter() {
                existing.insert(signature.clone());
            }
            true
        }
        (ValueDef::Typed(existing), ValueDef::Typed(incoming))
            if same_field(existing, &incoming) =>
        {
            existing.origins.extend(&incoming.origins);
            true
        }
        _ => false,
    }
}

fn merge_table(existing: &mut TableDef, incoming: TableDef) -> bool {
    if !compatible_doc(&existing.doc, &incoming.doc)
        || !merge_fields(&mut existing.fields, incoming.fields)
    {
        return false;
    }
    for (name, overloads) in incoming.functions {
        if let Some(existing) = existing.functions.get_mut(&name) {
            for signature in overloads.iter() {
                existing.insert(signature.clone());
            }
        } else {
            existing.functions.insert(name, overloads);
        }
    }
    existing.origins.extend(&incoming.origins);
    if existing.doc.is_none() {
        existing.doc = incoming.doc;
    }
    true
}

fn merge_fields(
    existing: &mut BTreeMap<LuaName, FieldDef>,
    incoming: BTreeMap<LuaName, FieldDef>,
) -> bool {
    for (name, field) in incoming {
        if let Some(current) = existing.get_mut(&name) {
            if !same_field(current, &field) {
                return false;
            }
            current.origins.extend(&field.origins);
        } else {
            existing.insert(name, field);
        }
    }
    true
}

fn same_field(left: &FieldDef, right: &FieldDef) -> bool {
    left.ty == right.ty && left.writable == right.writable && compatible_doc(&left.doc, &right.doc)
}

fn compatible_doc(left: &Option<String>, right: &Option<String>) -> bool {
    left.is_none() || right.is_none() || left == right
}

fn lower_class(class: &LuaClass, origin: Provenance) -> ClassDef {
    let (_, parameter_names) = split_type_declaration(&class.name);
    let parameters = parameter_names
        .iter()
        .cloned()
        .map(|id| TypeParameter {
            id,
            constraint: None,
        })
        .collect::<Vec<_>>();
    let fields = class
        .fields
        .iter()
        .map(|field| {
            (
                LuaName::from(field.name.clone()),
                lower_field(field, &parameter_names, origin.clone()),
            )
        })
        .collect();
    let mut methods = BTreeMap::new();
    for method in &class.methods {
        insert_method(&mut methods, method, &parameter_names, origin.clone());
    }
    ClassDef {
        parameters,
        fields,
        methods,
        doc: class.doc.clone(),
        origins: Origins::one(origin),
    }
}

fn lower_alias(alias: &LuaEnum, origin: Provenance) -> AliasDef {
    let literals = alias
        .variants
        .iter()
        .cloned()
        .map(LuaLiteral::String)
        .map(TypeExpr::Literal)
        .collect::<Vec<_>>();
    let target = canonical_union(literals);
    let constructors = alias
        .pascal_variants
        .iter()
        .cloned()
        .map(|name| {
            (
                LuaName::from(name),
                TypeExpr::Named {
                    id: TypeId::new(alias.name.clone()),
                    arguments: Vec::new(),
                },
            )
        })
        .collect();
    AliasDef {
        parameters: Vec::new(),
        target,
        constructors,
        doc: alias.doc.clone(),
        origins: Origins::one(origin),
    }
}

fn lower_table(module: &crate::LuaModule, origin: Provenance) -> TableDef {
    let fields = module
        .fields
        .iter()
        .map(|field| {
            (
                LuaName::from(field.name.clone()),
                lower_field(field, &[], origin.clone()),
            )
        })
        .collect();
    let mut functions = BTreeMap::new();
    for function in &module.functions {
        insert_function(&mut functions, function, &[], origin.clone());
    }
    TableDef {
        fields,
        functions,
        doc: module.doc.clone(),
        origins: Origins::one(origin),
    }
}

fn insert_method(
    methods: &mut BTreeMap<LuaName, OverloadSet>,
    method: &LuaMethod,
    parameters: &[TypeParameterId],
    origin: Provenance,
) {
    let name = LuaName::from(method.name.clone());
    let signature = lower_method(method, parameters, origin);
    methods
        .entry(name)
        .and_modify(|overloads| overloads.insert(signature.clone()))
        .or_insert_with(|| OverloadSet::one(signature));
}

fn insert_function(
    functions: &mut BTreeMap<LuaName, OverloadSet>,
    function: &LuaFunction,
    parameters: &[TypeParameterId],
    origin: Provenance,
) {
    let name = LuaName::from(function.name.clone());
    let signature = lower_function(function, None, parameters, origin);
    functions
        .entry(name)
        .and_modify(|overloads| overloads.insert(signature.clone()))
        .or_insert_with(|| OverloadSet::one(signature));
}

fn lower_method(
    method: &LuaMethod,
    parameters: &[TypeParameterId],
    origin: Provenance,
) -> Signature {
    lower_signature(
        method.doc.as_ref(),
        &method.params,
        &method.returns,
        method.is_async,
        Some(method.kind),
        parameters,
        origin,
    )
}

fn lower_function(
    function: &LuaFunction,
    receiver: Option<MethodKind>,
    parameters: &[TypeParameterId],
    origin: Provenance,
) -> Signature {
    lower_signature(
        function.doc.as_ref(),
        &function.params,
        &function.returns,
        function.is_async,
        receiver,
        parameters,
        origin,
    )
}

fn lower_signature(
    doc: Option<&String>,
    params: &[crate::LuaParam],
    returns: &[crate::LuaReturn],
    is_async: bool,
    receiver: Option<MethodKind>,
    type_parameters: &[TypeParameterId],
    origin: Provenance,
) -> Signature {
    let mut inputs = Vec::new();
    let mut variadic = None;
    for param in params {
        if param.ty == LuaType::Nil {
            continue;
        }
        match &param.ty {
            LuaType::Variadic(inner) => {
                variadic = Some(Box::new(lower_type(inner, type_parameters)));
            }
            ty => inputs.push(Parameter {
                name: param.name.clone(),
                ty: lower_type(ty, type_parameters),
            }),
        }
    }
    Signature {
        docs: doc.into_iter().cloned().collect(),
        type_parameters: Vec::new(),
        inputs,
        variadic,
        outputs: returns
            .iter()
            .filter(|ret| !is_error_type(&ret.ty))
            .map(|ret| ReturnValue {
                name: ret.name.clone(),
                ty: lower_type(&ret.ty, type_parameters),
            })
            .collect(),
        is_async,
        receiver,
        origins: Origins::one(origin),
    }
}

fn lower_field(field: &LuaField, parameters: &[TypeParameterId], origin: Provenance) -> FieldDef {
    FieldDef {
        ty: lower_type(&field.ty, parameters),
        writable: field.writable,
        doc: field.doc.clone(),
        origins: Origins::one(origin),
    }
}

fn lower_type(ty: &LuaType, parameters: &[TypeParameterId]) -> TypeExpr {
    match ty {
        LuaType::Nil => TypeExpr::Nil,
        LuaType::Boolean => TypeExpr::Boolean,
        LuaType::Integer => TypeExpr::Integer,
        LuaType::Number => TypeExpr::Number,
        LuaType::String => TypeExpr::String,
        LuaType::Table => TypeExpr::Table,
        LuaType::Function => TypeExpr::Function(Box::new(lower::empty_signature())),
        LuaType::Any => TypeExpr::Any,
        LuaType::Dynamic(reason) => TypeExpr::Dynamic {
            reason: reason.clone(),
        },
        LuaType::Literal(value) => TypeExpr::Literal(value.clone()),
        LuaType::Array(item) => TypeExpr::Array(Box::new(lower_type(item, parameters))),
        LuaType::Optional(item) => {
            canonical_union(vec![lower_type(item, parameters), TypeExpr::Nil])
        }
        LuaType::Map(key, value) => TypeExpr::Map {
            key: Box::new(lower_type(key, parameters)),
            value: Box::new(lower_type(value, parameters)),
        },
        LuaType::Record { fields, open } => TypeExpr::Record {
            fields: fields
                .iter()
                .map(|field| {
                    (
                        LuaName::from_bytes(field.name.as_bytes().to_vec()),
                        lower_type(&field.ty, parameters),
                    )
                })
                .collect(),
            open: *open,
        },
        LuaType::Class(name) => lower_named_type(name, parameters),
        LuaType::UnresolvedRust { path, arguments } => TypeExpr::Named {
            id: TypeId::new(path.rsplit("::").next().unwrap_or(path)),
            arguments: arguments
                .iter()
                .map(|argument| lower_type(argument, parameters))
                .collect(),
        },
        LuaType::ExternalRust {
            path, arguments, ..
        } => TypeExpr::Named {
            id: TypeId::new(path.rsplit("::").next().unwrap_or(path)),
            arguments: arguments
                .iter()
                .map(|argument| lower_type(argument, parameters))
                .collect(),
        },
        LuaType::UnsupportedStatic { rust_type, reason } => unreachable!(
            "unsupported static Rust type reached semantic assembly: {rust_type}: {reason}"
        ),
        LuaType::StringLiteral(values) => canonical_union(
            values
                .iter()
                .cloned()
                .map(LuaLiteral::String)
                .map(TypeExpr::Literal)
                .collect(),
        ),
        LuaType::Union(items) => canonical_union(
            items
                .iter()
                .map(|item| lower_type(item, parameters))
                .collect(),
        ),
        LuaType::Thread => TypeExpr::Thread,
        LuaType::Variadic(item) => lower_type(item, parameters),
        LuaType::FunctionSig { params, returns } => TypeExpr::Function(Box::new(Signature {
            docs: Vec::new(),
            type_parameters: Vec::new(),
            inputs: params
                .iter()
                .enumerate()
                .map(|(index, ty)| Parameter {
                    name: format!("p{}", index + 1),
                    ty: lower_type(ty, parameters),
                })
                .collect(),
            variadic: None,
            outputs: returns
                .iter()
                .map(|ty| ReturnValue {
                    name: None,
                    ty: lower_type(ty, parameters),
                })
                .collect(),
            is_async: false,
            receiver: None,
            origins: Origins::one(Provenance::explicit("inferred function type")),
        })),
    }
}

fn lower_named_type(name: &str, parameters: &[TypeParameterId]) -> TypeExpr {
    let name = name.trim();
    if let Some(parameter) = parameters
        .iter()
        .find(|parameter| parameter.as_str() == name)
    {
        return TypeExpr::Parameter(parameter.clone());
    }
    if let Some((base, arguments)) = split_generic_application(name) {
        return TypeExpr::Named {
            id: TypeId::new(base),
            arguments: split_top_level(arguments, ',')
                .into_iter()
                .map(|argument| lower_named_type(argument, parameters))
                .collect(),
        };
    }
    TypeExpr::Named {
        id: TypeId::new(name),
        arguments: Vec::new(),
    }
}

fn canonical_union(items: Vec<TypeExpr>) -> TypeExpr {
    let mut flattened = Vec::new();
    for item in items {
        if let TypeExpr::Union(nested) = item {
            flattened.extend(nested);
        } else {
            flattened.push(item);
        }
    }
    flattened.sort();
    flattened.dedup();
    match flattened.len() {
        0 => TypeExpr::Any,
        1 => flattened.pop().unwrap_or(TypeExpr::Any),
        _ => TypeExpr::Union(flattened),
    }
}

fn class_type_id(declaration: &str) -> TypeId {
    TypeId::new(split_type_declaration(declaration).0)
}

fn split_type_declaration(declaration: &str) -> (String, Vec<TypeParameterId>) {
    let declaration = declaration.trim();
    let Some((base, parameters)) = split_generic_application(declaration) else {
        return (declaration.to_string(), Vec::new());
    };
    (
        base.to_string(),
        split_top_level(parameters, ',')
            .into_iter()
            .map(str::trim)
            .filter(|parameter| !parameter.is_empty())
            .map(TypeParameterId::new)
            .collect(),
    )
}

fn split_generic_application(value: &str) -> Option<(&str, &str)> {
    let open = value.find('<')?;
    value
        .ends_with('>')
        .then(|| (&value[..open], &value[open + 1..value.len() - 1]))
}

fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    for (index, character) in text.char_indices() {
        match character {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if character == separator && depth == 0 {
            parts.push(text[start..index].trim());
            start = index + character.len_utf8();
        }
    }
    parts.push(text[start..].trim());
    parts
}

fn is_error_type(ty: &LuaType) -> bool {
    matches!(ty, LuaType::Class(name) if name == "Error" || name == "LuaError"
        || name.ends_with("::Error") || name.ends_with("::LuaError"))
}

#[cfg(test)]
mod tests;
