# Semantic API and dialect boundary

## Decision

`SemanticApi` is the sole input to annotation rendering. Extraction produces
owned `ApiContribution` values with provenance. `SemanticApi::assemble`
canonicalizes overloads, merges compatible definitions, and rejects type or
Lua-path collisions.

The graph separates type identity from Lua value identity:

- `TypeId` keys classes and aliases.
- segmented, byte-preserving `LuaPath` values key registered Lua values.
- `TypeExpr` stores structured named applications, arrays, maps, unions,
  functions, literals, and type parameters.
- `Origins` stays non-empty and accumulates when compatible contributions
  merge.

The rustc extractor keeps a private mutable draft because its visitors discover
facts incrementally. That draft does not cross the semantic boundary and code
generation cannot accept it.

## Rendering call stack

```text
rustc callbacks
  -> ApiCompiler::compile
    -> private extraction draft
    -> owned ApiContribution values
    -> SemanticApi::assemble
  -> codegen::render(SemanticApi, Dialect)
    -> dialect capability check
    -> annotation renderer
    -> RenderedDocument
  -> isolated unit report
  -> atomic publication
```

`Dialect` is closed over LuaLS and EmmyLua. Each entry has an explicit feature
set. The adapter checks every semantic requirement before it writes text. An
unsupported fact returns `RenderError::Unsupported`; the renderer never drops
the fact or changes it to `any`.

## Verification

The production CLI fixture exercises extraction, semantic assembly, report
commit, deterministic assembly, and publication. Its generated document passes
LuaLS 3.18.2 and EmmyLua 0.25.1 with warnings treated as errors. A unit test
removes exact-class support from a test capability set and proves that rendering
fails before document construction.

## Rejected designs

Renderer-shaped vectors made code generation responsible for overload grouping
and collision behavior. A shared annotation AST would make the intersection of
two evolving dialects the semantic contract. Both approaches can silently lose
information. The semantic graph keeps extracted meaning independent of either
annotation grammar.
