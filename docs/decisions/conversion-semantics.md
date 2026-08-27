# Exact conversion semantics

Status: accepted on 2026-08-27.

`mlua-typegen` treats `IntoLua` and `FromLua` as separate directional
contracts. It derives each contract from resolved MIR operations, evaluated
constants, reachable local helpers, and control-flow joins. Rust type and
variant names are provenance only; they are never evidence of a Lua value.

The compiler-side call stack is:

```text
rustc after_analysis
  -> ExtractionPlan (exact mlua trait DefId + direction)
  -> conversion::analyze (bounded MIR interpretation)
  -> conversion::assemble (directional ConversionContract)
  -> semantic API assembly
  -> isolated UnitReport
  -> locked atomic snapshot publication
```

The abstract value domain distinguishes finite literals from scalar types and
closed produced tables from open accepted tables. A runtime-computed string is
therefore `String`, not an invented literal set and not `Any`. Static analysis
that cannot prove a supported contract returns an incomplete-extraction error;
the coordinator preserves the previous snapshot.

The implementation recognizes native `mlua::Value` construction and
destructuring, `Lua::create_string`, `Lua::create_table`, `Table::set`,
`Table::get`, generated userdata-cloning `FromLua`, constants, result/try
plumbing, and bounded local helper calls by resolved `DefId`. Unrecognized
value-producing calls and dynamic table keys are incomplete rather than
silently widened.

The accepted prototype and its seven-case decision lab remain on branch
`codex/prototype-conversion-semantics` at commit `dcaaa9d`. Locked real-mlua
fixtures for every supported minor line are part of the release-matrix
decision; the initial production fixture uses native mlua 0.12 and covers
renamed strings, numeric values, asymmetric directions, table shapes, local
helpers, generated `FromLua`, runtime-computed strings, and incomplete
publication.
