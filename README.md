# mlua-typegen

Automatically generate Lua type stubs from your Rust [mlua](https://github.com/mlua-rs/mlua) `UserData` implementations. Get full autocomplete and type checking in LuaLS or EmmyLua with zero manual annotation.

## How it works

`mlua-typegen` is a rustc driver (like clippy or miri) that hooks into compilation after type checking, when all types are fully resolved. It walks the HIR to find `impl UserData` blocks, extracts method signatures and field types using rustc's type inference, and generates [LuaCATS](https://luals.github.io/wiki/annotations/) stub files.

No attributes, no macros, no code changes required.

## Example

Given this Rust code:

```rust
impl UserData for Vec2 {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("x", |_, this| Ok(this.x));
        fields.add_field_method_get("y", |_, this| Ok(this.y));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("dot", |_, this, (other_x, other_y): (f64, f64)| {
            Ok(this.x * other_x + this.y * other_y)
        });
        methods.add_method("components", |_, this, ()| {
            Ok((this.x, this.y))
        });
    }
}
```

It generates:

```lua
---@meta

---@class (exact) Vec2
---@field x number (readonly)
---@field y number (readonly)
local Vec2 = {}

---@param other_x number
---@param other_y number
---@return number
Vec2["dot"] = function(self, other_x, other_y) end

---@return number x
---@return number y
Vec2["components"] = function(self) end
```

## Install

Requires Rust 1.98 on a rustup-managed nightly because the driver links against rustc internals. Your project can still be stable Rust; `mlua-typegen` uses its own pinned nightly toolchain when it runs.

The supported toolchain is `nightly-2026-06-30` (`rustc 1.98.0-nightly`). The repository pins this version so rustc-private API changes in later nightlies cannot break builds without warning.

```bash
cargo +nightly-2026-06-30 install --git https://github.com/themixednuts/mlua-typegen
```

No separate component setup is required. The repository declares `rustc-dev` and the Unix LLVM runtime in `rust-toolchain.toml`, and the build script installs either with rustup if it is missing.

## Usage

```bash
# Generate ./lua-types/mlua-typegen.lua
mlua-typegen

# Specify output directory
mlua-typegen --output types/

# Target a specific crate in a workspace
mlua-typegen -p my-plugin-crate

# Generate EmmyLua-flavored annotations
mlua-typegen --emmylua

# Use a non-default workspace mapping file
mlua-typegen --config config/mlua-typegen.toml
```

Each successful run publishes one complete `mlua-typegen.lua` snapshot. Compiler
processes write only to private per-run report directories; a failed build leaves
the previous public snapshot unchanged. If upgrading from the former multi-file
layout, remove the old generated output directory once before regenerating.

Then point your language server at the output:

```json
// .luarc.json (LuaLS)
{
    "workspace.library": ["lua-types"]
}
```

```json
// .emmyrc.json (EmmyLua)
{
    "workspace": {
        "library": ["lua-types"]
    }
}
```

## What it extracts

- **Classes** from `impl UserData` blocks
- **Fields** from `add_field_method_get` / `add_field_method_set` (tracks readonly vs writable)
- **Methods** from `add_method` / `add_function` (instance vs static)
- **Async methods** from `add_async_method` / `add_async_function`
- **Metamethods** from `add_meta_method` (`__tostring`, `__add`, `__len`, etc.)
- **Enums** from Rust enums with string-based `IntoLua` / `FromLua` impls
- **Modules** from `create_table` / scope-based function registration
- **Doc comments** carried through to the generated stubs
- **Overloaded methods** emitted with `@overload` annotations
- **Named return values** extracted from tuple multi-returns (e.g. `---@return number x`)
- **Union types** from match/if branches returning different types (e.g. `string | integer`)
- **`AnyUserData` resolution** — traces through `create_any_userdata(expr)` to resolve the concrete class
- **`into_lua_multi` decomposition** — expands tuple returns into multiple `@return` annotations

## Explicit type mappings

Place `mlua-typegen.toml` at the Cargo workspace root, or select exactly one
file with `--config PATH`. Start the file with Tombi's schema directive so the
editor validates the generated contract directly:

```toml
#:schema https://raw.githubusercontent.com/themixednuts/mlua-typegen/main/schemas/mlua-typegen.schema.json

[[mappings]]
id = "uuid"
rust = { package = "uuid", version = "1", path = "Uuid" }
lua = "string"

[[mappings]]
id = "wire-id"
rust = { package = "domain", path = "WireId" }
from_lua = "string"
into_lua = "integer"

[[external_types]]
name = "Promise"
params = ["T"]
```

`lua` asserts the same `FromLua` and `IntoLua` type. Directional fields may be
declared separately. Expressions support unions, optionals, arrays, maps,
functions, named types, and declared Rust parameters such as `$T`. A genuinely
runtime-erased value must use `dynamic("reason")`; bare `any` is invalid.

The schema is generated from the Rust configuration types with
`cargo run --example generate-config-schema`. Its URL remains stable; the
project does not use or pre-bump numeric schema versions. See the
[explicit type-mapping decision](docs/decisions/explicit-type-mappings.md).

## Native type mapping

| Rust | Lua |
|------|-----|
| `String`, `&str`, `Cow<str>` | `string` |
| `i8`..`i128`, `u8`..`u128`, `usize`, `isize` | `integer` |
| `f32`, `f64` | `number` |
| `bool` | `boolean` |
| `Vec<T>`, `VecDeque<T>` | `T[]` |
| `Option<T>` | `T?` |
| `HashMap<K, V>`, `BTreeMap<K, V>` | `table<K, V>` |
| `Result<T, _>` | `T` (unwrapped) |
| `Box<T>`, `Arc<T>`, `Rc<T>`, `Mutex<T>`, `RwLock<T>` | `T` (unwrapped) |
| `mlua::Table` | `table` |
| `mlua::Function` | `function` |
| `mlua::Thread` | `thread` |
| `mlua::Value` | `any` with native dynamic evidence |
| `mlua::Error` | `string` |
| `mlua::AnyUserData` | resolved registered userdata identity |

External crate semantics such as `uuid::Uuid` are not guessed from names or
suffixes; declare them explicitly when native or extracted evidence cannot
prove the Lua type.

## Real-world examples

The [`examples/`](examples/) directory contains generated stubs for:

- **[Lune](examples/lune/)** — Roblox datatypes (CFrame, Vector3, Color3, etc.) and standard library modules
- **[Yazi](examples/yazi/)** — file manager plugin API with 200+ class methods

## License

MIT
