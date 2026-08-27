# Prototype: semantic API and dialect boundary

## Question

What typed intermediate representation preserves classes, aliases, overloads,
modules, generics, exact Lua paths, and provenance while allowing LuaLS and
EmmyLua to be rendered and validated independently?

## Grounded constraints

- Extraction runs inside rustc and initially owns `DefId` and `Span`; compiler
  unit reports cross a process boundary, so only stable owned provenance may
  leave the driver.
- `LuaPath` is the accepted byte-preserving identity of a Lua-visible binding.
- IntoLua and FromLua behavior remain distinct directional extraction facts.
- Unit reports and final publication are already isolated, deterministic, and
  fixed-name.
- LuaLS explicitly warns that modern LuaCATS is not generally cross-compatible
  with EmmyLua. Both tools currently support classes, exact classes, generics,
  fields, aliases, and overloads, but support can evolve independently.
- Unsupported semantic facts must not silently become `any` or disappear.

Primary references:

- [LuaLS annotations](https://luals.github.io/wiki/annotations/)
- [EmmyLua class syntax](https://github.com/EmmyLuaLs/emmylua-analyzer-rust/blob/main/docs/emmylua_doc/annotations_EN/class.md)
- [EmmyLua generic syntax](https://github.com/EmmyLuaLs/emmylua-analyzer-rust/blob/main/docs/emmylua_doc/annotations_EN/generic.md)

## Alternatives exercised

### A. Renderer-shaped bags

Keep `LuaApi { classes, enums, modules, global_fields, global_functions }` and
add fields as needs appear. Renderers infer overloads by duplicate names and
reconstruct identity from strings.

This has low migration cost, but the container cannot express canonical merge
rules, scoped generic parameters, stable type identity versus Lua binding
identity, or provenance-rich collisions. Every renderer repeats semantic work.

### B. Common annotation AST

Lower extraction directly into target-neutral annotation nodes such as
`ClassAnnotation`, `ParamAnnotation`, and `OverloadAnnotation`, then print the
same tree differently for each target.

This centralizes formatting but makes the least-common annotation syntax the
domain model. A dialect capability gap is encountered after semantic
information has already been flattened or dropped. Strings stand in for type
references and generic scope.

### C. Semantic graph with dialect adapters (recommended)

Extract owned contributions, assemble one immutable semantic graph, and lower
that graph separately through closed LuaLS and EmmyLua adapters. Each adapter
must either preserve the requested semantic feature or return a typed
`UnsupportedFeature`; it may not weaken a type silently.

This costs the largest migration, but invariants have one owner and every
dialect receives the same complete input.

## Code-shaped contract

```rust
struct UnitFacts {
    contributions: Vec<ApiContribution>,
    conversions: Vec<ConversionContract>,
}

enum ApiContribution {
    DefineType(TypeContribution),
    BindValue(ValueContribution),
    ExtendTable(TableContribution),
}

struct Provenance {
    owner: RustDefinition,
    location: SourceLocation,
    form: NativeMluaForm,
}

struct RustDefinition {
    cargo_unit: CargoUnitId,
    crate_name: String,
    def_path_hash: u128,
    display_path: String,
}

struct SemanticApi {
    types: BTreeMap<TypeId, TypeDef>,
    values: BTreeMap<LuaPath, ValueDef>,
}

enum TypeDef {
    Class(ClassDef),
    Alias(AliasDef),
}

struct ClassDef {
    parameters: Vec<TypeParameter>,
    fields: BTreeMap<LuaName, FieldDef>,
    methods: BTreeMap<LuaName, OverloadSet>,
    provenance: NonEmpty<Provenance>,
}

struct AliasDef {
    parameters: Vec<TypeParameter>,
    target: TypeExpr,
    provenance: NonEmpty<Provenance>,
}

enum ValueDef {
    Table(TableDef),
    Function(OverloadSet),
    Typed(TypeExpr),
}

struct TableDef {
    fields: BTreeMap<LuaName, FieldDef>,
    functions: BTreeMap<LuaName, OverloadSet>,
    provenance: NonEmpty<Provenance>,
}

struct OverloadSet(NonEmpty<Signature>);

struct Signature {
    parameters: Vec<TypeParameter>,
    inputs: Vec<Parameter>,
    variadic: Option<TypeExpr>,
    outputs: Vec<ReturnValue>,
    is_async: bool,
    provenance: Provenance,
}

enum TypeExpr {
    Nil,
    Boolean,
    Integer,
    Number,
    String,
    Table,
    Function(Box<Signature>),
    Literal(LuaLiteral),
    Named { id: TypeId, arguments: Vec<TypeExpr> },
    Parameter(TypeParameterId),
    Array(Box<TypeExpr>),
    Map { key: Box<TypeExpr>, value: Box<TypeExpr> },
    Union(NonEmpty<TypeExpr>),
}

enum Dialect {
    LuaLs,
    EmmyLua,
}

fn assemble(
    units: impl IntoIterator<Item = UnitFacts>,
) -> Result<SemanticApi, NonEmpty<SemanticError>>;

fn render(
    api: &SemanticApi,
    dialect: Dialect,
) -> Result<RenderedDocument, NonEmpty<RenderError>>;

fn validate(
    document: &RenderedDocument,
    validator: &Validator,
) -> Result<ValidatedDocument, ValidationError>;
```

`TypeId`, `TypeParameterId`, `CargoUnitId`, and `SourceLocation` are validated
owned values with private representation. `NonEmpty` is schematic: production
may use a small project-owned wrapper rather than a dependency. Optional values
are canonical unions with `Nil`; there is no second optional representation.

## Entrypoint-to-effect call stacks

### Extract one compiler unit

```text
rustc_driver::Callbacks::after_analysis(TyCtxt)
  [driver process; rustc values trusted only here]
  -> ExtractionPlan::for_crate(TyCtxt): ExtractionPlan
  -> extract_unit(TyCtxt, ExtractionPlan): Result<UnitFacts, ExtractError>
       -> provenance::own(DefId, Span): Provenance
       -> registration emitters: Vec<ApiContribution>
       -> conversion analyzer: Vec<ConversionContract>
  -> UnitReport::commit(UnitFacts): Result<UnitReport, ReportError>
       [codec rejects non-owned/compiler-local values]
       -> fixed report files + completion marker
```

### Assemble one semantic API

```text
generation coordinator
  -> UnitReport::read_complete: Vec<UnitFacts> | ReportError
  -> semantic::assemble(units): SemanticApi | NonEmpty<SemanticError>
       -> TypeIndex::insert(TypeContribution)
          [type id, generic arity, and duplicate-definition owner]
       -> ValueIndex::insert(ValueContribution)
          [LuaPath identity and kind-collision owner]
       -> TableDef::merge(TableContribution)
          [compatible reopening or provenance-rich conflict]
       -> TypeExpr::resolve(TypeIndex)
          [reference existence and generic arity owner]
  <- immutable, deterministically ordered SemanticApi
```

### Render and validate a selected dialect

```text
generate(Dialect, &SemanticApi)
  -> render::render: RenderedDocument | NonEmpty<RenderError>
       -> render::luals::lower OR render::emmylua::lower
          [dialect syntax and capability owner]
          -> identity projections for names and safe bindings
          -> dialect document bytes
  -> validate::validate(RenderedDocument, Validator)
       -> lua-language-server --check OR emmylua_check
          [external process adapter; tool diagnostics translated here]
  -> deterministic publication
       [only a validated document may become mlua-typegen.lua]
```

### Failure path

```text
dynamic name / unresolved type / incompatible reopening
  -> typed ExtractError or SemanticError with all Provenance
  -> coordinator rejects this run
  -> previous published snapshot remains intact

dialect cannot represent a proven semantic feature
  -> RenderError::UnsupportedFeature { dialect, feature, provenance }
  -> no approximation and no publication

validator rejects generated syntax or semantics
  -> ValidationError { dialect, version, diagnostics }
  -> no publication; previous snapshot remains intact
```

## Ownership and leak rules

- Extractors know rustc and native mlua source forms; `SemanticApi` does not.
- The semantic assembler knows merge, reference, overload, and generic-scope
  invariants; renderers do not repair malformed facts.
- Dialect renderers know annotation syntax and capability versions; extractors
  and semantic types do not contain target flags.
- Validator adapters know executable paths, process output, and diagnostic
  codecs; those details do not cross into rendering.
- The coordinator owns fail-closed publication and deterministic ordering.
- No broad renderer trait is needed for the closed two-dialect set. An enum
  dispatcher keeps the public surface finite; each adapter remains a module.

## Vertical proof obligations

1. One generic overload set renders equivalent parameter/return relationships
   and passes both real validators.
2. One Rust type bound at two `LuaPath`s yields one type definition and two
   independently addressable values.
3. Literal-dot and segmented paths remain distinct through extraction,
   assembly, both renderers, and validation.
4. Compatible module reopening is order-independent; incompatible kinds or
   fields return a diagnostic naming both provenances.
5. Missing type references and wrong generic arity cannot construct a
   publishable `SemanticApi`.
6. A target capability gap returns `UnsupportedFeature`; it never emits `any`,
   drops a field, or reuses another dialect's unchecked syntax.
7. Unit ordering does not affect serialized semantic API or rendered bytes.
8. Only output accepted by the selected real analyzer reaches atomic
   publication.

## Prototype execution evidence

- The interactive lab exercised 3 architectures × 2 dialects, 42 free-play
  transitions, four guided walkthroughs, and both capability-failure paths
  without a browser or console error.
- A common exact-class, generic-overload, and dual-binding document passed
  `lua-language-server 3.19.1 --check` and `emmylua_check 0.25.1`.
- An EmmyLua-only `@return_overload` document passed EmmyLua validation as
  expected. LuaLS also reported no problem because it ignores the unknown tag;
  this proves that parse/diagnostic success cannot establish semantic
  preservation. The renderer's versioned capability matrix must reject an
  unsupported semantic feature before invoking the external validator.

## Verdict requested

Accept the semantic graph with independent, fail-closed dialect adapters, or
select one of the other approaches and identify which lost invariant is
acceptable.
