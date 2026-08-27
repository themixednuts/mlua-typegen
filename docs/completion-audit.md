# Destination completion audit

This is a working evidence ledger for the Wayfinder destination. A checked item
requires production code plus a vertical test that enters through the normal
CLI or another public boundary. Decision documents describe the contract; they
do not prove implementation.

## Proven

- [x] Rust 1.98 is declared and pinned to `nightly-2026-06-30`.
- [x] `mlua >=0.9.0, <0.13.0` has bounded manifests, exact lockfiles, minimum,
  latest, and behavior-boundary lanes, and reviewed output from real upstream
  crates.
- [x] Configuration has one workspace-root file, one explicit override, a
  stable direct schema URL, structural validation before Cargo, exact Cargo
  package selection, and a generated-schema freshness test.
- [x] One coordinator assembles completion-marked unit reports and atomically
  replaces one fixed output file while preserving the prior snapshot on Cargo
  failure.
- [x] The semantic graph owns deterministic merges and typed collisions. Closed
  LuaLS and EmmyLua adapters consume that graph.
- [x] Lua registration identity uses byte-preserving path segments, safe source
  access, reversible annotation identifiers, and fixed report filenames.

## Not yet proven

- [x] `ExtractionPlan` enumerates crate owners once. It collects generated
  module-wrapper facts during that enumeration, and `BodyCollector` performs
  the sole recursive visit for each emitted body. Field, method, runtime
  registry, module, event, and global registration observations are leaf
  operations in that visit. The ordinary CLI fixture and all nine native
  compatibility lanes preserve their reviewed output through this path.
- [x] Explicit mappings affect type lowering by resolved `DefId`, validate Rust
  generic parameters and direction use, report conflicts, transport used
  mapping IDs, and warn only for unused mappings after successful publication.
- [x] Static type lowering preserves canonical definition identity and rejects
  unknown definitions, unavailable evidence, missing generic arguments, and
  unsupported analysis. Runtime-erased values carry an explicit dynamic reason.
- [x] Conversion analysis proves every accepted MIR form, follows bounded local
  helpers, reports unsupported effects, and preserves exact asymmetric input
  and output semantics through the public API.
- [x] Cargo records every relevant compiled unit, computes the least fixed point
  of dependency owners reachable from Lua exports, replays fresh or newly
  reached units under a requested-definition scope, and fails when required
  local source is unavailable. Vertical fixtures cover path and alternate
  registry dependencies through the global `RUSTC_WRAPPER` boundary.
- [x] Every incomplete static fact yields a stable structured diagnostic with
  source span, Rust owner, Lua path when known, reason, and remediation.
  Independent errors accumulate deterministically, commit no unit report, and
  preserve the previous snapshot with an explicit CLI summary.
- [x] The production CLI fixture covers nested-owner uniqueness, simultaneous
  body observations, reachable path and registry dependency types, explicit
  directional generic mappings, both independent dialect validators, multiple
  diagnostic accumulation, and failure-preserving publication.

## Completion gates

- [x] No accepted decision lacks production evidence.
- [x] No compatibility shim, guessed type rule, duplicate recursive traversal,
  renderer-owned semantic merge, or Lua-controlled filesystem path remains.
- [x] Formatting, Clippy with warnings denied, all ordinary tests, both dialect
  validators, and the complete locked `mlua` matrix pass from a clean target.
- [x] The thermo-nuclear maintainability review finds no blocking structural
  regression or obvious simplification left undone.
