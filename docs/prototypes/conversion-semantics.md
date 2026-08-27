# Conversion semantics decision trace

## Question

How can `mlua-typegen` derive the actual Lua values and shapes accepted or
produced by `IntoLua` and `FromLua` implementations without guessing from Rust
variant names?

The supported range is native `mlua >=0.9, <0.13`. `IntoLua` and `FromLua` are
unconstrained: implementations may use strings, numbers, tables, userdata, or
runtime-computed values. `#[derive(FromLua)]` clones userdata in every supported
line; it is not evidence of a string enum.

## Required behavior

- Preserve `IntoLua` and `FromLua` as separate directional contracts. A type may
  produce fewer values than it accepts.
- Recover finite literals and table keys only from resolved program semantics.
- Represent runtime-computed values by their exact Lua scalar/shape type, not by
  guessed literals and not automatically as `any`.
- Treat unavailable or unsupported static semantics as an actionable incomplete
  extraction, not as a dynamic value.
- Recognize the generated userdata-cloning `FromLua` implementation as userdata.
- Keep conversion analysis pure with respect to public output. Publication still
  occurs only after the complete semantic API succeeds.

## Alternatives

### A. Rust-name convention

Turn every enum variant into snake_case when any conversion impl exists.

- Lowest implementation cost.
- Incorrect for renamed strings, numbers, tables, and generated userdata clones.
- Cannot preserve different input/output sets.
- Rejected because it invents Lua values.

### B. HIR spelling patterns

Scan conversion bodies for `Value::String`, string literals, `Table::set`, and
match arms.

- Handles simple handwritten examples.
- Loses constants and local helper calls, depends on source spelling, and
  confuses unreachable literals with produced values.
- Still needs a separate model for direction, branches, and table mutation.
- Rejected as the semantic authority; HIR remains useful only for provenance.

### C. Resolved MIR abstract interpretation

Interpret each conversion impl over a deliberately small abstract Lua-value
domain. Follow local reachable helpers, evaluate constants through rustc, and
recognize resolved `mlua` operations by `DefId`. Join branch outcomes into
finite literal sets or structural types. Stop with a typed unsupported outcome
when a static effect cannot be modeled.

- Preserves actual control flow and type-checked call identity.
- Handles constants, local helpers, asymmetric directions, and table writes.
- Clearly separates a runtime-computed `string` from an unanalysed conversion.
- Requires a bounded interpreter, but the supported operation vocabulary is
  explicit and testable against locked real-mlua fixtures.
- Recommended.

Runtime execution of user conversions was also considered and rejected: it
requires constructing arbitrary Rust values, can run user side effects, and
still cannot enumerate accepted `FromLua` inputs.

## Code-shaped contract

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConversionDirection {
    IntoLua,
    FromLua,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConversionSubject {
    rust_type: RustTypeId,
    impl_id: RustImplId,
    direction: ConversionDirection,
    source: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LuaValueDomain {
    Nil,
    Boolean,
    Integer,
    Number,
    String,
    Literal(LuaLiteral),
    Table(TableShape),
    UserData(RustTypeId),
    Function(FunctionShape),
    Union(Vec<LuaValueDomain>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TableShape {
    fields: Vec<TableField>,
    openness: TableOpenness,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConversionAnalysis {
    Exact {
        subject: ConversionSubject,
        values: LuaValueDomain,
        evidence: Vec<SourceSpan>,
    },
    Unsupported(UnsupportedConversion),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConversionContract {
    rust_type: RustTypeId,
    into_lua: Option<LuaValueDomain>,
    from_lua: Option<LuaValueDomain>,
}
```

`LuaValueDomain::String` means the runtime value is not a finite literal set but
its Lua type is exactly `string`. `Unsupported` means the analyzer lacks static
evidence and must prevent complete publication. Those states cannot collapse
into one fallback.

## Entrypoint-to-effect stack

```text
rustc after_analysis(tcx)
  -> ExtractionPlan::for_crate(tcx)
    -> resolve exact mlua IntoLua/FromLua trait DefId
    -> ConversionSubject { rust_type, impl_id, direction, source }
  -> conversion::analyze(tcx, subject)
    -> rustc MIR body for the impl method
    -> ConversionInterpreter::run(body)
      -> resolved call adapter (mlua operation DefId -> semantic operation)
      -> rustc constant evaluation adapter
      -> local-call recursion with cycle/depth detection
      -> control-flow joins over LuaValueDomain
    <- ConversionAnalysis::Exact | ConversionAnalysis::Unsupported
  -> conversion::assemble(direction-specific analyses)
    <- ConversionContract | ConversionConflict
  -> semantic API assembly (owned by the later semantic-API decision)
    <- complete API | source-linked incomplete diagnostic
  -> isolated UnitReport
    -> deterministic coordinator assembly
      -> OutputLock::publish(GeneratedSnapshot)
        -> atomic filesystem replacement
```

The interpreter knows rustc/MIR and resolved `mlua` operations. The semantic API
knows only `ConversionContract`; rustc `DefId`, MIR locals, and upstream helper
names cannot leak into renderers.

## Failure paths

- Unknown external helper body: `UnsupportedConversion::UnavailableBody`; no
  snapshot replacement.
- Dynamic scalar computation with known return type: exact scalar domain such as
  `String`; generation may continue.
- Dynamic table key or shape-changing loop: unsupported unless an explicit open
  map domain is statically proven.
- Contradictory impl analyses for one direction: `ConversionConflict` with both
  source spans.
- Interpreter cycle, budget exhaustion, unsupported unsafe/FFI effect, or
  unrecognized resolved mlua operation: typed incomplete diagnostic.
- `IntoLua`/`FromLua` asymmetry: preserved, not treated as a conflict.

## Code ownership

- `src/extract/plan.rs`: resolve native mlua trait ownership and create
  directional `ConversionSubject` work; no suffix matching.
- `src/extract/conversion/`: MIR interpreter, resolved-operation adapter,
  abstract domain, joins, and typed failures.
- semantic API module chosen by the dependent decision: owns assembled
  `ConversionContract` and use-site projection.
- renderers: receive only semantic Lua domains and never inspect Rust variants.

## Proof obligations

Locked real-mlua fixtures for 0.9, 0.10, 0.11, and 0.12 must cover renamed and
numeric enum values, asymmetric input/output sets, table conversion, local
constants/helpers, generated `FromLua` userdata cloning, runtime-computed exact
scalars, and unsupported external helpers. Negative tests must prove that Rust
variant names never appear unless the conversion body actually uses them as Lua
values.
