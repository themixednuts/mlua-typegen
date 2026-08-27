# Lua-visible identity decision trace

## Question

How should exact registration names, aliases, nested modules, collisions, Lua
escaping, and filesystem containment become stable generated identities?

## Alternatives

1. **Raw names everywhere.** Reuse a dotted name as runtime access, annotation
   name, merge key, and filename. This is readable but collapses a literal dot
   into nesting, emits invalid Lua for keywords and punctuation, and permits
   report-directory traversal.
2. **Sanitized identifiers everywhere.** Rewrite each name to a Lua identifier.
   This contains files but changes runtime names and creates collisions such as
   `foo-bar` versus `foo_bar`.
3. **Canonical path segments with projections.** Store every exact Lua string
   byte sequence as one segment. Derive separate renderer-safe access syntax,
   reversible annotation identifiers, and a fixed report filename. This is the
   recommended model.

## Code-shaped contract

```rust
struct LuaName(Box<[u8]>);

struct LuaPath {
    root: LuaRoot,
    segments: Box<[LuaName]>,
}

enum LuaRoot {
    Globals,
}

struct LuaIdentity {
    path: LuaPath,
    provenance: RegistrationSite,
}

enum IdentityContribution {
    Module(ModuleContribution),
    Class(ClassContribution),
    Function(FunctionContribution),
    Field(FieldContribution),
}

enum IdentityMergeError {
    DynamicName { provenance: RegistrationSite },
    ConflictingKinds { path: LuaPath, first: RegistrationSite, second: RegistrationSite },
    ConflictingDefinitions { path: LuaPath, first: RegistrationSite, second: RegistrationSite },
}
```

`LuaPath(["a.b"])` and `LuaPath(["a", "b"])` are distinct. Aliases register
the same Rust shape under separate `LuaPath` values. Modules can reopen and add
compatible members; an incompatible contribution to one canonical path is a
typed error with both registration sites. Dynamic registration names are an
incomplete extraction, never a guessed identifier.

## Entrypoint-to-effect stack

```text
rustc after_analysis
  -> registration extraction
    -> StaticLuaName::from_constant(bytes) | DynamicName
    -> LuaPath::child(exact segment)
    -> IdentityContribution { path, shape, provenance }
  -> semantic API assembly
    -> IdentityIndex::insert(contribution)
      -> new identity | compatible module reopen | IdentityMergeError
  -> dialect renderer
    -> LuaAccess::for_path(path)       // bracket syntax with byte-safe literals
    -> AnnotationId::for_path(path)    // reversible, collision-free internal id
  -> UnitReport::write
    -> fixed report filenames only; Lua names never reach Path::join
  -> deterministic coordinator
    -> OutputLock::publish
      -> fixed mlua-typegen.lua destination
```

The semantic API owns exact identity and collision rules. Dialect renderers own
annotation spelling and Lua source escaping. The report protocol owns
filesystem containment. Rust paths remain provenance and never substitute for
Lua registration paths.

## Proof obligations

- Keywords, quotes, slashes, dots, empty names, NUL, non-UTF-8 bytes, and Unicode
  round-trip to exact bracket access without changing the identity.
- A literal dotted key cannot collide with nested segments.
- Annotation ids are reversible and collision-free without hashing.
- No Lua-controlled or crate-controlled string becomes a report or public
  output path; every compiler unit uses fixed filenames.
- Module reopening is deterministic and order-independent for compatible
  members; every incompatible collision reports both provenances.
- The only public output path is the already-decided fixed
  `mlua-typegen.lua` snapshot under the locked output directory.
