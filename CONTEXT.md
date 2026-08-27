# mlua-typegen Context

Shared language for the compatibility and extraction guarantees made by `mlua-typegen`.

## Language

**Supported mlua range**:
The stable `mlua` releases that a particular `mlua-typegen` minor release analyzes through version-native APIs. The range is `>=0.9, <0.13` until a tested `mlua-typegen` release changes it.
_Avoid_: Best-effort compatibility, latest mlua

**Exact Lua type**:
A Lua type derived from resolved Rust and `mlua` semantics. `any` is exact only when the source value is genuinely dynamic; unsupported static patterns are diagnostics.
_Avoid_: Guessed type, compatibility type

**Native mlua form**:
A public `mlua` registration or conversion form analyzed according to the API of its own supported version, including handwritten traits and generated implementations.
_Avoid_: Compatibility shim, normalized facade

**Reachable dependency type**:
A type from the workspace, a path dependency, or locally available registry source that is exposed through the generated Lua API.
_Avoid_: Every dependency type, transitive scan

**Complete generation**:
A generation run that atomically replaces its output only after every statically typed exported symbol is extracted successfully. Unsupported static patterns fail the run instead of leaving partial or stale stubs.
_Avoid_: Best-effort output, partial generation

**Incomplete run**:
A generation run in which one or more exported static facts cannot be proven. It publishes nothing, reports every independently discoverable error, and leaves any previously generated snapshot unchanged.
_Avoid_: Partial success, degraded generation

**Extraction diagnostic**:
A typed, source-linked explanation of why a static API fact could not be proven. Its stable code, stage, Rust owner, Lua-visible path when known, reason, and remediation are part of the diagnostic contract.
_Avoid_: Compiler failure, error string

**Dynamic type reason**:
Explicit evidence that `any` is the exact Lua type because the runtime value is intentionally erased or heterogeneous. Missing evidence, unsupported analysis, or an unavailable definition is not a dynamic type reason.
_Avoid_: Fallback `any`, unknown type

**Unit report**:
The private, isolated facts committed by one successful rustc invocation. A report is visible to the coordinator only after its completion marker is durable; it is never public generated output.
_Avoid_: Shared compiler output, partial report

**Generated snapshot**:
The single `mlua-typegen.lua` file assembled deterministically from all complete unit reports and atomically published by the coordinator.
_Avoid_: Per-crate output, incrementally patched stubs

**Lua-visible identity**:
The name and structure registered into Lua, which is authoritative for generated declarations. Rust paths are provenance for diagnostics, not generated API identity.
_Avoid_: Rust item identity

**Explicit type mapping**:
A validated project declaration of the exact Lua meaning of a resolved external or domain Rust type. It supplies missing proof and cannot silently replace conflicting native evidence.
_Avoid_: Fallback mapping, implicit override

**Mapping selector**:
The unambiguous identity of a Rust type definition in the resolved build, including its Cargo package instance and canonical definition path. Dependency aliases and reexports do not create new identities.
_Avoid_: Import path, type-name match

**Directional type mapping**:
An explicit type mapping for values entering Lua, leaving Lua, or both. A symmetric mapping asserts that both conversion directions have the same exact Lua type.
_Avoid_: Universal override, undirected mapping

**Mapping configuration**:
The single workspace-wide set of explicit type mappings used by one generation run.
_Avoid_: Package overlay, cascading configuration

**Mapping schema**:
The JSON Schema generated from the canonical mapping-configuration types and referenced directly by a mapping file. Its link remains stable through ordinary iteration, commits, and crate releases; it is not a numeric development counter.
_Avoid_: Schema version number, development counter

**Generic type mapping**:
An explicit type mapping whose declared Rust generic parameters may appear in its Lua type expression. The driver verifies those parameter names and their order against the resolved Rust definition before substituting concrete arguments.
_Avoid_: Arity guess, positional wildcard

**External Lua type**:
A named Lua type and generic arity declared as an available reference without inventing a class body. It permits exact references to a type owned by another generator or handwritten declaration.
_Avoid_: Synthetic class, unknown named type

**Semantic API**:
The language-server-independent model extracted from Rust and `mlua`; LuaLS and EmmyLua are separately validated renderings of this same model.
_Avoid_: Shared syntax approximation
