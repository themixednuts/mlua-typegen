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

---@class Vec2
---@field x number (readonly)
---@field y number (readonly)
local Vec2 = {}

---@param other_x number
---@param other_y number
---@return number
function Vec2:dot(other_x, other_y) end

---@return number x
---@return number y
function Vec2:components() end
```

## Install

Requires a nightly toolchain with `rustc-dev` (the driver links against rustc internals). Your project can use stable Rust — the tool auto-selects nightly at runtime.

```bash
cargo +nightly install --git https://github.com/themixednuts/mlua-typegen
```

Make sure the nightly `rustc-dev` component is installed:

```bash
rustup component add rustc-dev --toolchain nightly
```

## Usage

```bash
# Generate stubs (defaults to ./lua-types/)
mlua-typegen

# Specify output directory
mlua-typegen --output types/

# Target a specific crate in a workspace
mlua-typegen -p my-plugin-crate

# Generate EmmyLua-flavored annotations
mlua-typegen --emmylua
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
- **Modules** from `create_table` / scope-based function registration
- **Doc comments** carried through to the generated stubs
- **Overloaded methods** emitted with `@overload` annotations
- **Named return values** extracted from tuple multi-returns (e.g. `---@return number x`)
- **Union types** from match/if branches returning different types (e.g. `string | integer`)
- **`AnyUserData` resolution** — traces through `create_any_userdata(expr)` to resolve the concrete class
- **`into_lua_multi` decomposition** — expands tuple returns into multiple `@return` annotations

## Type mapping

| Rust | Lua |
|------|-----|
| `String`, `&str`, `Cow<str>` | `string` |
| `i8`..`i128`, `u8`..`u128`, `usize`, `isize` | `integer` |
| `f32`, `f64` | `number` |
| `bool` | `boolean` |
| `Vec<T>`, `VecDeque<T>`, `SmallVec<T>` | `T[]` |
| `Option<T>` | `T?` |
| `HashMap<K, V>`, `BTreeMap<K, V>`, `IndexMap<K, V>` | `table<K, V>` |
| `Result<T, _>` | `T` (unwrapped) |
| `Box<T>`, `Arc<T>`, `Rc<T>`, `Mutex<T>`, `RwLock<T>` | `T` (unwrapped) |
| `mlua::Table` | `table` |
| `mlua::Function` | `function` |
| `mlua::Thread` | `thread` |
| `mlua::Value` | `any` |
| `mlua::Error` | `string` |
| `mlua::AnyUserData` | resolved class or `any` |
| `serde_json::Value` | `any` |
| `uuid::Uuid`, `url::Url` | `string` |
| Other `UserData` types | class reference by name |

## Real-world examples

The [`examples/`](examples/) directory contains generated stubs for:

- **[Lune](examples/lune/)** — Roblox datatypes (CFrame, Vector3, Color3, etc.) and standard library modules
- **[Yazi](examples/yazi/)** — file manager plugin API with 200+ class methods

## License

MIT
