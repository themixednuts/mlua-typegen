# Explicit type mappings

## Status

Accepted for issue #7.

## Decision

`mlua-typegen` has one workspace-wide mapping file. By default it is
`mlua-typegen.toml` at the Cargo workspace root; `--config PATH` is the only
override. Configuration does not cascade through packages and environment
variables do not alter precedence.

The file links directly to the checked-in generated JSON Schema. It has no
numeric schema field:

```toml
#:schema https://raw.githubusercontent.com/themixednuts/mlua-typegen/main/schemas/mlua-typegen.schema.json

[[mappings]]
id = "uuid"
rust = { package = "uuid", version = "1", path = "Uuid" }
lua = "string"
```

The schema URL remains stable through ordinary implementation iteration,
commits, and crate releases. The schema artifact changes only when the
canonical configuration types change; a test checks that the checked-in file
matches those types. There is no schema development counter to pre-bump.

## Rust identity

A selector identifies a Cargo package instance and the canonical path of a
definition within that package:

```toml
rust = { package = "uuid", version = "1", path = "Uuid" }
```

`package` and `path` are required. `version` is a SemVer requirement used only
when the package name alone does not select one package instance. `source` is
available only when name and version remain ambiguous. Dependency aliases and
reexports do not affect identity. The coordinator rejects selectors that match
zero or multiple package instances; the driver resolves the selected crate and
canonical definition path to the actual rustc `DefId`. Selection never uses a
first match.

Generic mappings declare the definition parameters they use:

```toml
[[mappings]]
id = "small-vector"
rust = { package = "smallvec", path = "SmallVec" }
params = ["A"]
lua = "$A[]"
```

The driver checks parameter names, order, and arity against the resolved Rust
definition. Concrete specializations and overlapping mappings are rejected;
there is no specificity or precedence system.

## Direction

`lua` is a symmetric assertion for `FromLua` input and `IntoLua` output. A
directional mapping instead declares one or both directions:

```toml
[[mappings]]
id = "wire-id"
rust = { package = "domain", path = "WireId" }
from_lua = "string"
into_lua = "integer"
```

A mapping cannot combine `lua` with directional fields, and a directional
mapping must contain at least one direction. Using an undeclared direction is
an extraction error.

## Lua type expressions

Mapping expressions are target-neutral semantic types, not raw LuaLS or
EmmyLua annotations. The language contains:

- `nil`, `boolean`, `integer`, `number`, `string`, `table`, `function`,
  `thread`, and literals;
- unions (`A | B`), optionals (`T?`), arrays (`T[]`), and maps
  (`map<K, V>`);
- typed functions (`fun(A, B): R`) and named types (`Page<T>`);
- declared Rust generic parameters (`$T`);
- intentional dynamic values as `dynamic("reason")`.

Bare `any` is invalid. `dynamic("reason")` carries evidence explaining why the
runtime value is genuinely erased or heterogeneous. It is never a substitute
for unsupported analysis.

Named references must be built-ins, extracted Lua types, or declared external
Lua types:

```toml
[[external_types]]
name = "Promise"
params = ["T"]
```

External declarations validate name and generic arity. They do not emit class
bodies.

## Authority and conflicts

Mappings fill semantics that native Rust, native `mlua`, or extracted evidence
cannot prove. Core Rust and `mlua` conversion rules are not overridable. If a
mapping conflicts with native or extracted evidence, generation fails. Duplicate
mapping IDs, duplicate selectors, ambiguous selectors, overlapping generic
mappings, and conflicting declarations are errors.

An exported static type with no native, extracted, or configured evidence is an
actionable extraction error. The old public string mapper, suffix heuristics,
class-name fallback, and guessed third-party mappings are removed without a
compatibility shim.

## Call stack and ownership

```text
CLI
  -> locate workspace config / --config
  -> parse and structurally validate once
  -> resolve Cargo package selectors exactly
  -> snapshot validated configuration in the private run workspace
  -> cargo check with the snapshot path
       -> rustc driver loads the snapshot
       -> resolve selectors to DefIds and validate Rust generics
       -> lower FromLua / IntoLua sites with one typed resolver
       -> write used mapping IDs in the unit report
  -> coordinator unions usage and warns for genuinely unused mappings
  -> assemble and atomically publish only after every unit succeeds
```

The configuration module exclusively owns parsing, expression validation,
package resolution, and the private snapshot representation. Type lowering owns
DefId resolution and directional substitution. The unit protocol transports
mapping IDs, not configuration semantics. Renderers only consume the semantic
API and never parse mapping syntax.

## Validation and failure paths

Parsing and structural validation occur before Cargo is started. Rust identity,
generic parameters, direction use, and conflicts with native evidence are
validated in the driver. Each independently discoverable error is reported in
the existing structured diagnostic pipeline, and any such error prevents
publication. Only mappings absent from all complete unit usage reports produce
unused warnings.

Tests cover schema freshness, malformed and contradictory configuration,
package ambiguity, expression parsing, generic substitution, directional use,
DefId resolution, unused aggregation, and a complete CLI-to-driver fixture.

## Rejected alternatives

- Numeric schema versions tied to development iterations or crate releases.
- Cascading per-package files or environment-variable precedence.
- Rust-path strings, dependency aliases, reexports, suffixes, or first-match
  selection as type identity.
- Raw renderer annotation strings.
- Bare `any`, implicit dynamic fallbacks, or guessed third-party semantics.
- Compatibility shims around the old string mapper.
- Synthesizing external class declarations merely to satisfy a reference.
