# Collection protocol design trace

Status: **prototype awaiting user verdict**  
Question: Which Cargo and rustc coordination design guarantees complete extraction despite freshness caching and parallel compiler processes, then publishes all-or-nothing output?

This is a throwaway design artifact for [Choose a deterministic collection and atomic publication protocol](https://github.com/themixednuts/mlua-typegen/issues/3). It records the proposed contract precisely enough to implement after validation; it is not production documentation.

## Scope and evidence

The current flow is:

```text
src/main.rs
  -> cargo check with RUSTC_WORKSPACE_WRAPPER
    -> src/driver.rs::LuaTypegenCallbacks::after_analysis for each non-fresh workspace unit
      -> codegen::write_stubs_for(output_dir, api, target, crate_name)
      -> codegen::append_event_emissions(output_dir, events)
  -> codegen::write_events_file(output_dir)
  -> resolve_cross_crate_lookups(output_dir)
```

That flow has four observed or source-proven failures:

1. A fresh Cargo unit does not invoke the wrapper callback, so a successful command can publish nothing or retain stale output.
2. Parallel callbacks mutate shared module files and append registries without a single owner.
3. A compiler failure can occur after another callback has already changed published output.
4. `RUSTC_WORKSPACE_WRAPPER` cannot observe non-workspace path or registry dependencies whose bodies own reachable Lua semantics.

The dependency research established the missing boundary: Cargo JSON artifact messages are the package/unit ledger; external rustc metadata identifies type owners; each reached owner needs its own local HIR compiler session.

## Required behaviors

| Behavior | Entrypoint | Observable result | Terminal failures |
|---|---|---|---|
| Generate exact stubs | `generate(GenerateRequest)` | one complete published snapshot | invalid request, Cargo failure, missing report, incomplete reachable owner, semantic conflict, render failure, publish failure |
| Reuse valid extraction evidence | `ReportStore::load(UnitKey)` | matching `UnitReport` | absent report triggers replay; incompatible report is ignored, never treated as empty |
| Reach dependency-owned types | `Reachability::advance(&ReportSet)` | least fixed point of required owners | owner unavailable, unit invocation unavailable, unsupported target |
| Publish output | `SnapshotPublisher::publish(CompleteSnapshot)` | new snapshot or previous snapshot, never a partial success | output busy, stage write failure, atomic replacement failure |
| Run concurrently | separate CLI processes | different run workspaces and isolated unit reports | one publisher owns a given output path; another receives `OutputBusy` |

Constraints:

- Rust 1.98 nightly is the driver ABI and schema identity.
- Stable `mlua >=0.9, <0.13` is the supported semantic range.
- Cargo decides the exact enabled unit graph, features, target, build-script output, and compiler arguments.
- Only locally available dependency source is eligible for replay.
- The previous complete output remains authoritative until a new complete snapshot commits.
- Compiler callbacks never own user-facing output or run completion.

## Alternatives

### A. Distributed direct writers — reject

Each rustc callback renders and writes its crate's files into the public output directory. The CLI patches shared artifacts after Cargo exits.

- Caller burden: low initially, but callers cannot know which callbacks ran.
- Invariant locality: none; completeness is distributed across processes and Cargo cache state.
- Errors: compiler and filesystem errors are frequently printed and discarded rather than reflected in the CLI result.
- Testability: races and freshness require process-level tests with nondeterministic shared effects.
- Migration: no change, but correctness requirements remain impossible.
- Operational failure: partial, stale, or empty successful output.

### B. Coordinator plus per-unit reports and directory swap — viable but weaker

Callbacks write isolated reports. One coordinator merges and renders a complete directory in a sibling staging path, renames the previous output to a backup, renames staging to output, and rolls back on failure.

- Caller burden: one typed `generate` operation.
- Invariant locality: coordinator owns completeness; publisher owns the directory transaction.
- Errors: typed and reportable.
- Testability: deterministic until publication.
- Migration: retains the current multi-file layout.
- Operational failure: portable filesystems do not provide one standard atomic replacement for a non-empty directory. Rollback protects command outcomes, but a concurrent language-server reader can observe the small rename gap.

### C. Coordinator plus one generated snapshot file — recommend

Callbacks write isolated reports. The coordinator merges the full semantic API, renders one deterministic Lua file for the selected dialect, writes it beside the destination, flushes it, then atomically renames it over the previous generated file.

- Caller burden: one typed `generate` operation.
- Invariant locality: `CompleteApi` is required to render; `CompleteSnapshot` is required to publish.
- Errors: typed through the library and rendered once by the CLI.
- Testability: byte-for-byte deterministic artifact and a small, platform-testable publication seam.
- Migration: changes generated layout from per-crate/module files to one declaration file.
- Operational failure: the prior file remains authoritative until atomic replacement succeeds. The output lock prevents competing publishers.

One file is not a compatibility shim and does not flatten semantic ownership: every declaration retains package, crate, definition, and source provenance in the semantic model and diagnostics. It only makes the final language-server artifact transactional.

Evidence that could reverse the recommendation: a supported language server demonstrably cannot index one file of the largest expected API size, or consumers require independent file-level loading rather than a single library root.

## Code-shaped contracts

The public library surface stays small. Concrete adapters remain crate-private until a second consumer proves a trait is needed.

```rust
pub struct GenerateRequest {
    pub cargo: CargoRequest,
    pub output: StubFile,
    pub dialect: Dialect,
    pub mappings: TypeMappings,
}

pub struct PublishedSnapshot {
    pub path: PathBuf,
    pub declarations: usize,
    pub units: usize,
}

pub fn generate(request: GenerateRequest) -> Result<PublishedSnapshot, GenerateError>;
```

Raw CLI strings do not cross the entrypoint boundary:

```rust
pub struct CargoRequest {
    pub manifest_path: Option<PathBuf>,
    pub packages: PackageSelection,
    pub features: FeatureSelection,
    pub target: Option<TargetTriple>,
    pub locked: bool,
    pub offline: bool,
}

pub struct StubFile(PathBuf);

impl TryFrom<PathBuf> for StubFile {
    type Error = InvalidOutput;
}

pub enum Dialect {
    LuaLs,
    EmmyLua,
}
```

Process-crossing values are owned, serializable, and schema-versioned:

```rust
struct RunId(String);

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
struct UnitKey {
    package: CargoPackageId,
    target: CargoTargetId,
    compile_kind: CompileKind,
    features: BTreeSet<String>,
    rustc: RustcIdentity,
    extractor_schema: SchemaVersion,
}

struct UnitReport {
    key: UnitKey,
    crate_id: StableCrateId,
    api: PartialApi,
    referenced_owners: BTreeSet<TypeOwner>,
    diagnostics: Vec<ExtractionDiagnostic>,
}

struct ReportSet(BTreeMap<UnitKey, UnitReport>);

struct CompleteApi {
    api: SemanticApi,
    units: BTreeSet<UnitKey>,
}

struct CompleteSnapshot {
    destination: StubFile,
    bytes: Vec<u8>,
    digest: SnapshotDigest,
}
```

Illegal publication states stay unrepresentable:

```rust
impl ReportSet {
    fn complete(
        self,
        expected: &BTreeSet<UnitKey>,
        mappings: &TypeMappings,
    ) -> Result<CompleteApi, CompletenessError>;
}

impl CompleteApi {
    fn render(self, dialect: Dialect, destination: StubFile)
        -> Result<CompleteSnapshot, RenderError>;
}

impl SnapshotPublisher {
    fn publish(&self, snapshot: CompleteSnapshot)
        -> Result<PublishedSnapshot, PublishError>;
}
```

`SnapshotPublisher` retains output-lock and filesystem lifecycle, so it is a struct rather than a broad service trait. Tests use real temporary filesystems through the same method.

Error ownership follows the call stack:

```rust
pub enum GenerateError {
    Cargo(CargoRunError),
    Reachability(ReachabilityError),
    Incomplete(CompletenessError),
    Render(RenderError),
    Publish(PublishError),
}

enum CompletenessError {
    MissingReport { unit: UnitKey },
    UnsupportedStaticPattern { diagnostic: ExtractionDiagnostic },
    ConflictingIdentity { identity: LuaIdentity, definitions: Vec<Provenance> },
}

enum PublishError {
    OutputBusy { path: PathBuf },
    WriteStage { path: PathBuf, source: io::Error },
    FlushStage { path: PathBuf, source: io::Error },
    Replace { path: PathBuf, source: io::Error },
}
```

Cargo JSON DTOs, rustc arguments, OS errors, and temporary paths do not appear above their adapters. Diagnostics carry stable provenance, not adapter internals.

## Successful entrypoint-to-effect stack

```text
mlua-typegen CLI: Vec<OsString>
  -> cli::parse: GenerateRequest | CliError
     owner: src/cli.rs; validates untrusted arguments and paths
  -> generate::generate(request): Result<PublishedSnapshot, GenerateError>
     owner: src/generate/mod.rs; synchronous operation; owns cancellation and terminal result
    -> cargo::metadata(&CargoRequest): UnitGraph | CargoMetadataError
       owner: src/generate/cargo.rs; child process; translates Cargo JSON/process failures
    -> RunWorkspace::create(UnitGraph): RunWorkspace | WorkspaceError
       owner: src/generate/run.rs; creates isolated local-durable report/stage paths
    -> CargoRun::check(&RunWorkspace): CargoLedger | CargoRunError
       owner: src/generate/cargo.rs; child process; consumes JSON messages to completion
      -> mlua-typegen-driver wrapper: UnitInvocation
         owner: src/driver.rs; process hop; parses driver-only protocol
        -> extract::collect_lua_api(tcx): UnitReport
           owner: src/extract.rs; pure with respect to public output
        -> ReportStore::commit(UnitReport)
           owner: src/report.rs; isolated atomic per-unit file; local-durable evidence
    -> reachability::collect(CargoLedger, ReportStore): ReportSet | ReachabilityError
       owner: src/generate/reachability.rs; owns least-fixed-point iteration and replay
    -> ReportSet::complete(expected, mappings): CompleteApi | CompletenessError
       owner: src/semantic/merge.rs; pure deterministic merge and invariant gate
    -> CompleteApi::render(dialect, destination): CompleteSnapshot | RenderError
       owner: src/render/mod.rs; pure bytes plus destination
    -> SnapshotPublisher::publish(snapshot): PublishedSnapshot | PublishError
       owner: src/generate/publish.rs; output lock, stage write/flush, atomic replace
       side effect: the only public-output mutation
  <- CLI renders one source-preserving error chain and exits nonzero on failure
```

## Fresh-unit replay path

```text
CargoRun receives compiler-artifact { fresh: true, unit }
  -> CargoLedger records Fresh(unit)
  -> ReportStore::load(UnitKey)
    -> matching schema/tool/rustc/features report: reuse
    -> absent or incompatible report: Reachability schedules Replay(unit)
      -> CargoRun::replay(recorded UnitInvocation)
        -> driver writes isolated UnitReport
  -> completion continues only with a valid report
```

Freshness is Cargo's compilation fact, not proof of extraction evidence.

## Reached dependency path

```text
root UnitReport references TypeOwner(package, target, stable_crate_id, def_path_hash)
  -> Reachability maps owner through CargoLedger
    -> already reported: add report
    -> recorded but unreported: replay exact unit invocation
    -> no local source or matching unit: ReachabilityError::UnavailableOwner
  -> dependency UnitReport can expose more owners
  -> iterate until owner set stops growing
```

No ordinary dependency-graph edge is traversed unless a Lua-visible type reaches it.

## Failure and concurrency paths

### Cargo or compiler failure

```text
CargoRun reports nonzero build-finished or child exit
  -> cancel pending replay work
  -> close and discard RunWorkspace stage
  -> return GenerateError::Cargo
  -> SnapshotPublisher is never called
  -> previous generated file remains unchanged
```

### Missing or conflicting semantics

```text
ReportSet::complete
  -> missing reached owner / unsupported static pattern / Lua identity conflict
  -> return CompletenessError with all relevant provenance
  -> no bytes are rendered and no output effect occurs
```

### Concurrent generators

```text
generate A and generate B use distinct RunId workspaces
  -> both may collect immutable UnitReports
  -> A acquires destination lock and publishes
  -> B fails with OutputBusy or waits only if the CLI explicitly requests waiting
```

The default is fail-fast `OutputBusy`; hidden waiting and indefinite lock acquisition are not part of the API.

### Publish failure

```text
SnapshotPublisher acquires destination lock
  -> write sibling temporary file
  -> flush file and close handle
  -> atomic replace destination
     success: remove lock and report PublishedSnapshot
     failure: remove temporary file, preserve previous destination, return PublishError::Replace
```

The publisher owns cleanup and error translation. The caller does not coordinate rollback.

## File ownership

```text
src/
├── cli.rs                    raw argument parsing and exit projection
├── main.rs                   composition root: parse -> generate -> report
├── driver.rs                 thin rustc protocol adapter
├── report.rs                 shared UnitKey/UnitReport codec and isolated store
├── generate/
│   ├── mod.rs                GenerateRequest, PublishedSnapshot, generate
│   ├── cargo.rs              metadata, JSON message ledger, child process
│   ├── reachability.rs       owner fixed point and exact unit replay
│   ├── run.rs                RunId and private workspace lifecycle
│   └── publish.rs            destination lock and atomic file replacement
├── semantic/
│   ├── mod.rs                SemanticApi and Lua-visible domain types
│   └── merge.rs              ReportSet -> CompleteApi
├── extract/                  rustc HIR -> UnitReport facts
└── render/                   CompleteApi -> CompleteSnapshot bytes
```

No `Manager`, `Helper`, service locator, broad repository trait, shared append file, or public adapter DTO is introduced. `main.rs` invokes one application operation. The driver invokes one extractor operation and one isolated report commit.

## Vertical proof obligations

All process tests enter through the same `generate(GenerateRequest)` operation used by the CLI.

1. A completely fresh workspace publishes exact stubs.
2. A second run with every Cargo unit fresh reuses or replays reports and publishes identical bytes.
3. Deleting the output but retaining Cargo artifacts still regenerates the complete file.
4. Parallel workspace units never share a report path; repeated stress runs are byte-identical.
5. A compiler failure after one valid report leaves the previous snapshot byte-identical.
6. A reached path dependency is selectively replayed and included.
7. A reached locally cached registry dependency is selectively replayed and included offline.
8. An unavailable reached owner fails with symbol, Rust path, package, and source provenance.
9. Competing generators for one destination produce one success and one typed `OutputBusy`, never interleaved bytes.
10. LuaLS and EmmyLua renderers consume the same `CompleteApi` and cannot publish on render diagnostics.
11. The largest real fixture remains indexable as one Lua file; if this fails, revisit directory publication with an explicit weaker observer guarantee.

## Verdict requested

Adopt **coordinator plus one generated snapshot file**. Compiler processes own only isolated extraction facts. The coordinator owns reachability and completeness. `SnapshotPublisher` alone owns the public-output side effect.
