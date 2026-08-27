# Lua-visible identity and output naming

## Decision

`LuaPath` is the canonical identity of a Lua-visible registration. It stores the
exact Lua string bytes of each path segment. Segment boundaries are semantic, so
the global key `"a.b"` is not the nested path `"a"`, `"b"`.

Source access, annotation identifiers, diagnostics, and storage names are
separate projections of that identity:

- `LuaAccess` emits byte-safe bracket access from `_G`.
- `AnnotationId` encodes every segment as its byte length followed by hex bytes.
- diagnostics may be human-readable but never participate in equality.
- compiler unit reports use the fixed filename `annotations.lua`; Lua and crate
  names never become path components.

The same Rust value registered at two paths has two Lua identities. Compatible
contributions that reopen one module may merge deterministically. Incompatible
contributions at one path are a collision and must retain both provenances.
Dynamic names are incomplete extraction evidence; the extractor must not guess a
static identity.

## Typed call stack

```text
rustc after_analysis
  -> registration extraction
    -> constant Lua bytes -> LuaName
    -> parent.child(name) -> LuaPath
  -> semantic API assembly
    -> canonical-path merge or typed collision
  -> dialect renderer
    -> LuaAccess::for_path
    -> AnnotationId::for_path
  -> UnitReport::write
    -> fixed annotations.lua
  -> deterministic coordinator
    -> fixed mlua-typegen.lua publication
```

This keeps extraction facts independent of Lua source syntax and keeps all
untrusted semantic names outside the filesystem boundary.
