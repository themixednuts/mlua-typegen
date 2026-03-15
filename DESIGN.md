# mlua-typegen

A rustc driver tool that extracts fully-inferred Lua type information from `mlua` UserData implementations and generates LuaCATS-compatible type stub files for LuaLS and EmmyLua LSPs.

## How It Works

Uses the same technique as clippy/miri — a custom `rustc` driver that hooks into compilation **after type checking**, when all types are fully resolved. Zero user annotations required.

```
cargo mlua-typegen --output types/
  → sets RUSTC_WRAPPER=mlua-typegen-driver
    → drives rustc as a library
      → normal compilation + type checking runs
      → after_analysis callback walks HIR with full TyCtxt
      → finds all `impl UserData` blocks
      → extracts method names, param types, return types from closures
      → maps Rust types → Lua types
      → writes .lua stub files with LuaCATS annotations
```

## Architecture

```
mlua-typegen/
├── src/
│   ├── lib.rs              # Shared types and Rust→Lua type mapping
│   ├── driver.rs           # rustc driver (Callbacks impl)
│   ├── extract.rs          # HIR walker: find impl UserData blocks, extract type info
│   ├── typemap.rs          # Rust ty::Ty → Lua type string mapping
│   └── codegen.rs          # Emit .lua stub files (LuaCATS format)
```

## Extraction Pipeline

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

---@class Buffer
---@field document_id string
local Buffer = {}

---@return string
function Buffer:get_text() end

---@param start Position
---@param stop Position
---@return string
function Buffer:get_range_text(start, stop) end

---@return Diagnostic[]
function Buffer:diagnostics() end
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
    output_dir: PathBuf,
}

impl Callbacks for LuaTypegenCallbacks {
    fn after_analysis<'tcx>(
        &mut self,
        _compiler: &interface::Compiler,
        tcx: TyCtxt<'tcx>,
    ) -> bool {
        let types = extract::collect_lua_types(tcx);
        codegen::write_stubs(&self.output_dir, &types);
        true // stop after analysis, don't codegen
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

The generated `lua/types/` directory can be added to your LSP's library paths:

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

- **Tool build**: requires nightly (`rustc_private` feature)
- **User projects**: no requirements — works on stable Rust, zero code changes needed
- **Nightly pinning**: like clippy, the tool should pin to a specific nightly version and track updates

## Open Questions

- [ ] How to handle `FromLua` impls (types constructable from Lua tables)?
- [ ] Should we generate `@alias` for Rust enums exposed via string matching?
- [ ] Module-level functions registered via `lua.create_function()` outside of UserData impls — detect via call to `table.set("name", func)`?
- [ ] Support for `#[derive(FromLua)]` if mlua adds derive macros?
- [ ] Should there be a config file for custom type mappings?
- [ ] Cross-crate analysis (UserData types defined in dependency crates)?
