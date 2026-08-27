# Extractor ownership and traversal

## Decision

`ExtractionPlan` is the only crate-level HIR owner classifier. It enumerates
`tcx.hir_crate_items(()).free_items()` once and never recursively walks module
items. Every free or impl function becomes one `ExtractionWork::Body` containing
its `BodyId`; only free functions retain an optional item reference for module
attributes. Class and conversion work remain owner-level capabilities rather
than body visitors.

`emit::BodyCollector` is the only recursive visitor for a function body. During
one expression traversal it records three independent observations:

- event emissions;
- exports from an annotated Lua module;
- global and table registrations.

The observation functions inspect only the current expression. They cannot
start another traversal. Registration state and annotated-module state are
private to the collector and are finalized together after the body has been
visited.

## Extraction call stack

```text
extract_unit(TyCtxt)
  -> ApiCompiler::compile
    -> ExtractionPlan::for_crate
      -> hir_crate_items().free_items() once
      -> one body-work item per function owner
    -> emit_work
      -> owner-level class or conversion extraction
      -> emit::collect_body
        -> BodyCollector::visit_expr once
          -> record_event_emission(current expression)
          -> record_module_export(current expression)
          -> update registration state(current expression)
    -> finish private extraction facts
    -> ApiContribution
    -> SemanticApi::assemble
```

## Invariants

- Nested Rust modules are discovered by `free_items`; the planner does not
  descend into a module and classify the same owner again.
- A function body has exactly one recursive expression visitor.
- Leaf observations do not recurse and do not own mutable traversal state.
- Closures are entered explicitly by `BodyCollector`, including async closure
  bodies.
- Extractor modules use explicit imports and curated parent APIs. Production
  code has no wildcard imports or wildcard reexports.
- Unsupported conversion and semantic facts remain errors. The collector does
  not retry extraction through a second traversal or erase a failed fact.

## Verification obligations

The CLI regression fixture places a `UserData` impl in a nested Rust module and
places an event emission beside a global function registration in one function
body. It asserts that the nested class appears exactly once and that both body
observations reach the generated annotation document. Unit and CLI tests cover
the remaining class, conversion, module, registration, and event paths.

## Rejected designs

Independent analyzer-owned visitors made traversal count proportional to the
number of extractors and allowed a new analyzer to process nested expressions a
second time. A generic callback fan-out visitor hid ordering and shared-state
requirements behind callbacks. The stateful body collector keeps traversal
ownership singular while leaving each observation as a small, testable leaf.
