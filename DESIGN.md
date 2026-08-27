# mlua-typegen

Exact `IntoLua` and `FromLua` behavior is modeled as separate directional
contracts. See [the accepted conversion-semantics decision](docs/decisions/conversion-semantics.md).

Lua-visible registrations use exact segmented identities, with source,
annotation, diagnostic, and filesystem projections kept separate. See
[the accepted Lua identity decision](docs/decisions/lua-identity.md).

Extraction facts cross into code generation through one immutable semantic
graph. LuaLS and EmmyLua capability checks run before their renderers. See
[the semantic API decision](docs/decisions/semantic-api.md).

A rustc driver tool that extracts fully-inferred Lua type information from `mlua` UserData implementations and generates LuaCATS-compatible type stub files for LuaLS and EmmyLua LSPs.

## How It Works

Uses the same technique as clippy/miri — a custom `rustc` driver that hooks into compilation **after type checking**, when all types are fully resolved. Zero user annotations required.

```
cargo mlua-typegen --output types/
  → acquires the output-directory lock
  → creates a private run workspace and fresh Cargo target directory
  → sets RUSTC_WRAPPER=mlua-typegen-driver
    → records every Cargo rustc invocation and extracts primary crates
  → coordinator replays only reachable dependency owners under a typed scope
  → accepts only completion-marked reports and resolves facts to a fixed point
  → one atomic rename publishes types/mlua-typegen.lua
```

## Architecture

```
mlua-typegen/
├── src/
│   ├── lib.rs              # Shared types and Rust→Lua type mapping
│   ├── driver.rs           # rustc driver (Callbacks impl)
│   ├── driver_protocol.rs  # Isolated compiler-unit report protocol
│   ├── generation.rs       # Deterministic assembly and atomic publication
│   ├── extract.rs          # Thin orchestration entry point
│   ├── extract/
│   │   ├── plan.rs         # Build a crate extraction plan from HIR items
│   │   ├── pipeline.rs     # ApiCompiler: plan -> emit -> assemble
│   │   ├── emit/           # Classes, modules, registrations, events, callbacks
│   │   ├── lower.rs        # rustc ty::Ty -> Lua type lowering helpers
│   │   ├── returns/        # Return-value inference and naming
│   │   ├── table/          # Table, converter, and param-use inference
│   │   └── finish.rs       # Dedupe and normalize private extraction facts
│   ├── semantic.rs         # Contributions, immutable graph, canonical merge
│   ├── typemap.rs          # Rust ty::Ty → Lua type string mapping
│   ├── codegen.rs          # Semantic rendering entry point and report I/O
│   └── codegen/
│       ├── dialect.rs      # Closed capability matrix and fail-closed dispatch
│       └── render.rs       # Annotation document rendering
```

## Extraction Pipeline

The extractor is structured like a small compiler:

1. `plan` walks the crate HIR once and records the work items worth extracting.
2. `emit` dispatches each work item to the module that owns that source pattern.
3. `lower`, `table`, `returns`, `fields`, and `signature` perform local analysis and type lowering.
4. `finish` deduplicates extraction facts and normalizes type references.
5. The extractor turns those facts into owned `ApiContribution` values. `SemanticApi::assemble` is the only merge and collision boundary.
6. The selected dialect rejects unsupported semantic features before rendering.

### 1. Find UserData impls

Walk all `ItemKind::Impl` in the crate's HIR. Check if the trait ref resolves to `mlua::UserData` (or `mlua::prelude::LuaUserData`).

### 2. Extract from `add_methods`

For each `methods.add_method("name", closure)` call:

- **Method name**: first arg, a string literal
- **Closure params**: from the closure signature's 3rd parameter (the user params tuple)
  - `()` → no params
  - `(String,)` → one param
  - `(String, i32)` → two params
- **Return type**: `tcx.type_of(closure_def_id)` gives the full closure type including inferred return. Unwrap `Result<T, mlua::Error>` to get `T`.

Also handle:
- `add_method` vs `add_function` (method with self vs static)
- `add_async_method` / `add_async_function`
- `add_meta_method` (map MetaMethod variants to `__tostring`, `__add`, etc.)

### 3. Extract from `add_fields`

For each `fields.add_field_method_get("name", closure)`:
- **Field name**: string literal
- **Field type**: closure return type (unwrap LuaResult)
- **Writable**: true if a matching `add_field_method_set` exists

### 4. Map Rust types to Lua types

| Rust Type | Lua Type |
|---|---|
| `String`, `&str`, `Cow<str>` | `string` |
| `i8`..`i128`, `u8`..`u128`, `isize`, `usize` | `integer` |
| `f32`, `f64` | `number` |
| `bool` | `boolean` |
| `()` | _(void / nil)_ |
| `Vec<T>` | `T[]` |
| `Option<T>` | `T?` |
| `HashMap<K, V>`, `BTreeMap<K, V>` | `table<K, V>` |
| `mlua::Table` | `table` |
| `mlua::Function` | `function` |
| `mlua::Value` | `any` |
| `Result<T, _>` | unwrap to `T` |
| Any type with `impl UserData` | resolved class name |

### 5. Generate LuaCATS stubs

Output format (compatible with both LuaLS and EmmyLua):

```lua
---@meta

---@class (exact) Buffer
---@field document_id string
local Buffer = {}

---@return string
Buffer["get_text"] = function(self) end

---@param start Position
---@param stop Position
---@return string
Buffer["get_range_text"] = function(self, start, stop) end

---@return Diagnostic[]
Buffer["diagnostics"] = function(self) end
```

## Rustc Driver Details

### Entry point

```rust
#![feature(rustc_private)]

extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_span;

use rustc_driver::Callbacks;
use rustc_middle::ty::TyCtxt;

struct LuaTypegenCallbacks {
    report_root: PathBuf,
}

impl Callbacks for LuaTypegenCallbacks {
    fn after_analysis<'tcx>(
        &mut self,
        _compiler: &interface::Compiler,
        tcx: TyCtxt<'tcx>,
    ) -> rustc_driver::Compilation {
        let crate_name = tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE);
        let unit = extract::extract_unit(tcx)
            .unwrap_or_else(|error| tcx.dcx().fatal(error.to_string()));
        driver_protocol::write_unit_report(
            &self.report_root,
            &unit,
            self.dialect,
            crate_name.as_str(),
        )
        .unwrap_or_else(|error| tcx.dcx().fatal(error.to_string()));
        rustc_driver::Compilation::Continue
    }
}
```

### Key rustc APIs used

- `tcx.hir().items()` — iterate all items in the crate
- `tcx.type_of(def_id)` — get the type of any definition (including closure return types)
- `tcx.def_path_str(def_id)` — get the fully qualified name of a type/trait
- `impl_block.of_trait` — check which trait an impl is for
- `args.as_closure().sig()` — get closure signature with resolved types
- `sig.output()` — the return type
- `sig.inputs()` — parameter types

### Closure type extraction flow

```
1. Find call expr: methods.add_method("name", <closure_expr>)
2. Get closure's HirId → DefId
3. tcx.type_of(closure_def_id) → ty::Closure(def_id, args)
4. args.as_closure().sig() → FnSig { inputs: [&Lua, &Self, (P1, P2, ...)], output: Result<R, Error> }
5. inputs[2] = param tuple → destructure for Lua param types
6. output = Result<R, _> → unwrap to R → map to Lua type
7. Recurse into R: Vec<T> → T[], Option<T> → T?, UserData → class name
```

## Target LSPs

### LuaLS (lua-language-server)
- https://github.com/LuaLS/lua-language-server
- Uses LuaCATS annotation format
- `---@meta` marks file as type-definitions-only
- Annotations: `@class`, `@field`, `@param`, `@return`, `@alias`, `@enum`, `@type`, `@generic`, `@overload`

### EmmyLua Analyzer (emmylua-analyzer-rust)
- https://github.com/EmmyLuaLs/emmylua-analyzer-rust
- Supports both EmmyLua and LuaCATS formats
- Shared annotation syntax with LuaLS for core features
- Extra: `@class (exact)`, `@class (partial)`, `@cast`, `@operator`, intersection types

### Compatibility strategy

Target the shared LuaCATS subset — it works in both LSPs. This covers: `@meta`, `@class`, `@field`, `@param`, `@return`, `@alias`, `@enum`, `@generic`, `@overload`, `@type`.

EmmyLua-specific features (`(exact)`, `@operator`) can be opt-in via CLI flags later.

## Usage

```bash
# Install (requires nightly to build the tool itself)
cargo +nightly install mlua-typegen

# Run against your project (your project can be on stable)
cargo mlua-typegen --output lua/types/

# Or specify which crate in a workspace
cargo mlua-typegen -p my-plugin-crate --output lua/types/
```

The `rust-toolchain.toml` and build script handle `rustc-dev` plus the Unix LLVM runtime; users should not need a separate component setup step.

The generated `lua/types/mlua-typegen.lua` snapshot lives in a directory that can
be added to your LSP's library paths:

```json
// .luarc.json (LuaLS)
{
    "workspace.library": ["lua/types"]
}
```

```json
// .emmyrc.json (EmmyLua)
{
    "workspace": {
        "library": ["lua/types"]
    }
}
```

## Build Requirements

- **Tool build**: requires nightly (`rustc_private` feature); `rustc-dev` and the Unix LLVM runtime are installed automatically when missing
- **User projects**: no requirements — works on stable Rust, zero code changes needed
- **Rust version**: requires Rust 1.98 and pins `nightly-2026-06-30`; newer nightlies are best-effort and older compilers fail fast in `build.rs`

## Open Questions

- [ ] How to handle `FromLua` impls (types constructable from Lua tables)?
- [ ] Should we generate `@alias` for Rust enums exposed via string matching?
- [ ] Module-level functions registered via `lua.create_function()` outside of UserData impls — detect via call to `table.set("name", func)`?
- [ ] Support for `#[derive(FromLua)]` if mlua adds derive macros?
- [ ] Should there be a config file for custom type mappings?
- [ ] Cross-crate analysis (UserData types defined in dependency crates)?
