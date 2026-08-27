# Diagnostic contract and incomplete runs

## Decision

Generation succeeds only when every exported static fact is proven. Unsupported
syntax, unavailable dependency source, semantic conflicts, invalid explicit
mappings, and dialect features that cannot represent the semantic API are
errors. Warnings are reserved for facts that cannot change generated output,
such as an unused mapping or a deprecation notice.

`any` is a proven type, not an escape hatch. It requires an explicit dynamic
reason in the type model, such as runtime-erased `mlua::Value`, opaque light
userdata, or a deliberately heterogeneous variadic value. Missing generic
arguments, unresolved userdata, unavailable source, and unsupported analysis
are errors.

Each extraction error has a stable typed code and stage, a primary source span,
the Rust owner, the Lua-visible path when one is known, an exact reason, and a
remediation. Rendering through rustc is an adapter at the outer boundary; the
internal representation remains structured so another renderer can serialize
it without parsing prose.

An incomplete run collects and deterministically sorts every independent error
that can still be discovered. It exits unsuccessfully, commits no incomplete
unit report, publishes no generated output, preserves the previous snapshot,
and names that preserved snapshot in its final summary.

## Diagnostic call stack

```text
mlua-typegen CLI
  -> cargo check with mlua-typegen-driver
    -> driver::Callbacks::after_analysis
      -> extract_unit(TyCtxt)
        -> ApiCompiler::compile
          -> ExtractionPlan::for_crate
          -> emit every independent work item
            -> ConversionError with typed reason and owner
          -> validate assembled semantic API
          -> sorted CompileDiagnostics
      -> rustc diagnostic adapter
        -> source-linked error for each CompileDiagnostic
      -> commit complete unit report only when no errors exist
  -> generation coordinator
    -> atomically publish only complete reports
  -> failed-run summary names the unchanged prior snapshot
```

## Ownership and failure boundaries

- Extraction owns facts and typed failure reasons; it does not format terminal
  output or decide publication.
- The driver owns the rustc diagnostic adapter because only that boundary has a
  `TyCtxt`, source map, and rustc session.
- The coordinator owns report completeness, deterministic assembly, and atomic
  publication.
- The CLI owns the final process summary, including the preserved snapshot path.
- A failed work item does not prevent independent work items from being
  analyzed. A failed extraction does prevent semantic publication and report
  commit.

## Verification obligations

- Two independent unsupported conversions produce two source-linked diagnostics
  in deterministic order during one run.
- Every diagnostic exposes its stable code, stage, reason, owner, optional Lua
  path, and remediation before terminal rendering.
- A failed run returns nonzero, does not expose an incomplete report, does not
  replace the output, and states which previous snapshot was preserved.
- A successful run still publishes one deterministic snapshot atomically.

## Rejected designs

Fail-fast extraction hides independent defects and makes correction iterative.
Free-form strings prevent stable tooling and force future JSON output to parse
human prose. Treating unsupported static analysis as `any` silently changes the
public Lua contract. Publishing a partial file makes a failed run look
successful and destroys the last known-good artifact.
