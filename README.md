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
    }
}
```

It generates:

```lua
---@meta

---@class Vec2
---@field x number (readonly)
---@field y number (readonly)
local Vec2 = {}

---@param other_x number
---@param other_y number
---@return number
function Vec2:dot(other_x, other_y) end
```

## Install

Requires a nightly toolchain (the tool uses `rustc_private` internals). Your project can use stable Rust.

```bash
cargo +nightly install --git https://github.com/themixednuts/mlua-typegen
```

## Usage

```bash
# Generate stubs (defaults to ./lua-types/)
cargo mlua-typegen

# Specify output directory
cargo mlua-typegen --output types/

# Target a specific crate in a workspace
cargo mlua-typegen -p my-plugin-crate

# Generate EmmyLua-flavored annotations
cargo mlua-typegen --emmylua
```

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
- **Doc comments** carried through to the generated stubs
- **Overloaded methods** emitted with `@overload` annotations

## Type mapping

| Rust | Lua |
|------|-----|
| `String`, `&str`, `Cow<str>` | `string` |
| `i32`, `u64`, `usize`, ... | `integer` |
| `f32`, `f64` | `number` |
| `bool` | `boolean` |
| `Vec<T>` | `T[]` |
| `Option<T>` | `T?` |
| `HashMap<K, V>` | `table<K, V>` |
| `Result<T, _>` | `T` (unwrapped) |
| `Box<T>`, `Arc<T>`, `Mutex<T>`, ... | `T` (unwrapped) |
| `mlua::Table` | `table` |
| `mlua::Function` | `function` |
| `mlua::Value` | `any` |
| Other `UserData` types | class reference by name |

Also handles types from popular crates: `serde_json`, `uuid`, `url`, `chrono`, `bytes`, `smallvec`, `indexmap`, `dashmap`, and more.

## License

MIT
