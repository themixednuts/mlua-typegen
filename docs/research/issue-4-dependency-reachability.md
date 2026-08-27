# Issue 4 research: dependency source reachability

Date: 2026-08-27

This note answers the compiler and Cargo parts of [issue #4](https://github.com/themixednuts/mlua-typegen/issues/4). Sources are limited to official Rust and Cargo documentation and source documentation.

## Findings

### Cargo metadata is the source index, not the extractor

`cargo metadata --format-version=1` returns every workspace package plus feature-enabled dependencies unless `--no-deps` is used. Each package has an absolute `manifest_path`, and each target has an absolute `src_path`. The resolved graph identifies package dependencies, enabled features, dependency kinds, target predicates, workspace members, and the current root. The `source` field distinguishes registry and git packages, while both path dependencies and workspace members have `source: null`; membership must therefore be decided with `workspace_members`, not `source` alone. Cargo also labels source IDs as opaque, so code must not parse undocumented details from them. [Cargo metadata JSON](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html#json-format)

`--filter-platform` narrows `resolve`, but it does not narrow the `packages` array. Feature flags also change the resolved graph. A reachability pass must use the filtered `resolve.nodes[*].deps` graph rather than treating every returned package as active. [Cargo metadata options](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html#options)

By default, metadata may fetch dependencies. `--no-deps` explicitly avoids fetching them but also removes the graph needed here. `--offline` restricts Cargo to local data and may change resolution; `--locked` prevents lockfile drift, while `--frozen` combines both. The command needs an explicit network policy. For a promise limited to locally available registry sources, prefer locked offline resolution and report unavailable packages rather than silently resolving a different graph. [Cargo manifest options](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html#manifest-options)

Do not derive registry cache paths from `CARGO_HOME`. Use Cargo's absolute `manifest_path` and target `src_path`. Cargo documents the home directory as a configurable cache, and it can redirect registry dependencies to an exact local source. [Cargo environment variables](https://doc.rust-lang.org/cargo/reference/environment-variables.html), [Cargo source replacement](https://doc.rust-lang.org/cargo/reference/source-replacement.html)

Metadata locates package roots and dependency edges. It does not replace compiler-driven discovery of `mod`, `include!`, macro-expanded code, or build-script-generated source. Those belong to the rustc invocation that compiled the target.

### `RUSTC_WORKSPACE_WRAPPER` cannot cover path and registry dependencies

Cargo invokes `RUSTC_WORKSPACE_WRAPPER` only for workspace members. A non-workspace path dependency and a registry dependency bypass it. Cargo passes the actual rustc path as the wrapper's first argument. If `RUSTC_WRAPPER` is also set, Cargo nests them as `RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER RUSTC`. [Cargo environment variables](https://doc.rust-lang.org/cargo/reference/environment-variables.html)

Cargo expressly says the workspace-wrapper value affects filename hashes, which separates its artifacts from ordinary builds. The documentation gives no matching cache-isolation promise for the global `RUSTC_WRAPPER`. Treat the latter as a transparent interception point, not as an invalidation key.

The current comment in [`src/driver.rs`](https://github.com/themixednuts/mlua-typegen/blob/main/src/driver.rs) is accurate only for workspace crates. Cross-package extraction needs one of these designs:

1. Use a global wrapper, cheaply record every rustc unit, and analyze only selected roots plus dependency owners reached from their exported Lua types.
2. Run ordinary Cargo first, then invoke the driver separately for each reached package with the same resolved features, target, cfgs, build-script output, and compiler arguments.

The first design preserves Cargo's exact invocations and is less likely to drift. A tool-owned target directory or cache namespace is needed so global wrapping does not collide with normal build artifacts.

### HIR is local; external metadata is useful but insufficient

`HirId` identifies a node in "the HIR of the current crate," and rustc's HIR visitor documentation describes walking "your crate." The callback's HIR walk therefore sees the crate being compiled, not dependency HIR. [rustc_hir](https://doc.rust-lang.org/nightly/nightly-rustc/rustc_hir/index.html), [HIR visitor](https://doc.rust-lang.org/nightly/nightly-rustc/rustc_hir/intravisit/index.html)

Queries on an external `DefId`, such as `tcx.type_of`, use external providers backed by the dependency's `.rmeta`. That metadata includes exported types, signatures, trait implementations, source-file information, and other facts needed to reference dependency items. [rustc query providers](https://rustc-dev-guide.rust-lang.org/query.html#how-queries-interact-with-external-crate-metadata), [crate metadata contents](https://rustc-dev-guide.rust-lang.org/backend/libs-and-metadata.html#metadata)

This is enough to lower a dependency type mentioned by a root API and to identify its owning crate. It is not enough to run this project's body-pattern analysis over the dependency. Encoded MIR is optional and `cargo check` omits it; HIR bodies are not exposed as the dependency's HIR map. Recovering `UserData` registration calls, closure expressions, string literals, and handwritten conversion bodies requires compiling the owning crate through the driver. [rmeta and optional MIR](https://rustc-dev-guide.rust-lang.org/backend/libs-and-metadata.html#rmeta)

Do not persist raw `DefId` values across compiler runs. Rustc documents `DefPathHash` as stable across crate and compilation-session boundaries and as including a `StableCrateId`. Persist a provenance key containing Cargo package ID, Cargo target identity, compiler/toolchain identity, and the definition's `DefPathHash`. Keep the Lua-visible registered name as the generated API identity. [DefPathHash](https://doc.rust-lang.org/nightly/nightly-rustc/rustc_hir/definitions/struct.DefPathHash.html), [stable IDs in incremental compilation](https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation-in-detail.html#checking-query-results-for-changes-hashstable-and-stablehashingcontext)

### Reachability is a least fixed point over Lua semantics

The root set is the statically named Lua exports extracted from the selected primary package targets. Walk each exported value's resolved Rust type through the supported `mlua` conversion rules. Built-in terminal types and explicit project mappings stop the walk. Transparent containers, tuples, callbacks, and return wrappers contribute the type arguments that remain visible in Lua. A nominal type contributes its defining `DefId` when exact lowering needs that type's `UserData`, `IntoLua`, or `FromLua` semantics.

For every newly reached nominal definition, load only the Cargo library unit that owns its stable crate ID and extract only the requested definitions' Lua-visible members or conversion forms. Those members may expose more nominal definitions, so repeat until the owner set stops growing.

This rule does not follow ordinary Rust fields, every public item, every local `impl`, or every edge in Cargo's dependency graph. A package becomes an extraction owner only because a type from that exact compiled unit appears in a Lua-visible position and has no terminal or explicit mapping. Re-exports and Cargo dependency aliases do not change ownership because the resolved `DefId` still names the defining crate. Ownership is diagnostic provenance; the generated declaration continues to use the Lua-visible registered identity.

### A wrapper callback is not guaranteed to run

Cargo's `compiler-artifact` JSON message has a `fresh` field. When it is true, existing artifacts were up to date and rustc was not executed. No rustc execution means no wrapper or `after_analysis` callback. The message also contains package ID, manifest path, target, features, and artifact filenames, so it should be the orchestration ledger. [Cargo JSON artifact messages](https://doc.rust-lang.org/cargo/reference/external-tools.html#artifact-messages)

Cargo fingerprints compilation units and marks each unit dirty or fresh. Fingerprints include rustc, profile, compile mode, target identity, enabled features, dependency hashes, source mtimes, Rust flags, and other settings. A dependency change propagates dirtiness to its consumers. [Cargo fingerprinting](https://doc.rust-lang.org/stable/nightly-rustc/cargo/core/compiler/fingerprint/index.html)

Extraction must therefore have its own cache contract:

- Key each per-unit report by package ID, target and compile kind, enabled features, rustc version, extraction schema/tool version, relevant compiler flags, and stable crate identity.
- Reuse a report for a fresh Cargo unit only when that complete key matches.
- If Cargo reports a needed unit as fresh but no matching extraction report exists, replay or force that unit through the driver. Never treat the missing callback as an empty API.
- Let parallel wrappers write isolated per-unit temporary reports. Merge only after Cargo emits a successful `build-finished` message and every reachable owner has a valid report.
- Atomically replace generated stubs after the merge. Compiler callbacks should not append directly to shared output.

## Recommended reachability flow

1. Run versioned Cargo metadata with the same manifest, features, target filter, and lock/network policy as generation.
2. Choose root package and library targets by Cargo package ID and target identity.
3. Run Cargo with JSON messages and a global wrapper in a tool-owned target/cache namespace. The wrapper records normalized invocations for all Rust library units but performs full HIR extraction only for selected roots.
4. From each extracted semantic API, collect referenced external definitions by stable owner identity. Map owners back to Cargo package/target units using the invocation ledger and Cargo artifact messages.
5. Replay the recorded driver invocation for each newly reached dependency owner, then repeat until no new owners appear. Do not analyze unrelated graph nodes.
6. Fail if a referenced owner has no local source, no matching Cargo unit, an unsupported target kind, or no valid extraction report. Include the referring Lua symbol, Rust definition path, Cargo package ID, and source location in the diagnostic.
7. Merge reports deterministically and publish output atomically.

The key boundary is simple: Cargo decides which package units exist and how they compile; rustc metadata resolves cross-crate type references; a local HIR session extracts the body-owned Lua semantics for each reached crate.
