# Locked mlua compatibility and release matrix

## Status

Accepted for "Specify the locked mlua compatibility and release matrix."

## Supported range

Every `mlua-typegen 0.1.x` release supports stable `mlua >=0.9.0, <0.13.0`.
The extractor analyzes each release line through that line's native API. It does
not compile projects through a facade and does not normalize old APIs with
compatibility shims.

Changing either end of the supported `mlua` range requires an
`mlua-typegen` minor release. An `mlua-typegen` patch release may refresh the
tested patch within an already-supported `mlua` minor line, add coverage, or
fix extraction without narrowing the range.

## Required lanes

The canonical lane list lives in
`tests/fixtures/mlua/matrix.toml`.

| Lane | Locked mlua | Why it exists |
|---|---:|---|
| `0.9-min` | 0.9.0 | lower bound and lifetime-bearing conversion/registration APIs |
| `0.9-latest` | 0.9.9 | final 0.9 patch |
| `0.10-min` | 0.10.0 | lifetime-free API and `UserData::register` lower bound |
| `0.10-latest` | 0.10.5 | final 0.10 patch |
| `0.11-min` | 0.11.0 | 0.11 lower bound before owned callbacks |
| `0.11-pre-once` | 0.11.4 | last patch without `add_method_once` |
| `0.11-once` | 0.11.5 | first patch with owned callback registration |
| `0.11-latest` | 0.11.6 | final 0.11 patch |
| `0.12-min-latest` | 0.12.0 | current 0.12 lower and upper tested patch, including generated userdata registration |

When a new 0.12 patch appears, `0.12-min-latest` splits into an immutable
`0.12-min` lane locked to 0.12.0 and a moving `0.12-latest` lane. The same rule
applies to a future supported minor line.

## Fixture and lockfile policy

Each lane is a standalone Cargo package with a committed `Cargo.lock`.
Its manifest uses the full compatible minor range, for example
`>=0.10.0, <0.11.0`. The lockfile selects the lane's exact patch. This proves
that normal project requirements may use a range while the compatibility test
is reproducible.

Minimum lanes never float. Latest lanes accept patch-only lockfile updates.
Boundary lanes never float because their exact position is the behavior under
test. Verification rejects a lane when Cargo resolves a different `mlua`
version than the matrix declares.

The fixtures compile upstream `mlua` itself. They cover handwritten
`UserData`, runtime `register_userdata_type`, fields, representative
sync/mut/async/meta registration, directional conversions, default and renamed
module entrypoints, the 0.11 owned-callback boundary, and the 0.12 `UserData`
derive and `userdata_impl` expansions. Focused extractor tests own exhaustive
registrar-vocabulary coverage; this matrix owns native API and release-boundary
coverage.

## Call stack

```text
compatibility-matrix integration entrypoint
  -> parse tests/fixtures/mlua/matrix.toml into CompatibilityMatrix
  -> validate unique lane names, bounded requirements, and expected lock version
  -> for each selected CompatibilityLane
       -> cargo metadata --locked --manifest-path <fixture>
       -> assert resolved mlua package == lane.locked
       -> cargo mlua-typegen --manifest-path <fixture>/Cargo.toml --locked
            -> normal CLI/config snapshot/coordinator path
            -> real rustc driver over that lane's upstream mlua HIR
            -> semantic assembly and dialect validation
            -> atomic generated snapshot
       -> compare the complete snapshot with the fixture's reviewed golden file
  -> one typed lane result or a failure naming the lane and violated invariant
```

The verifier owns matrix parsing and subprocess failure translation. Cargo owns
dependency resolution. The normal `cargo mlua-typegen` entrypoint owns extraction and
publication. Fixtures do not contain adapter code that changes `mlua` source
forms.

## CI and updates

Pull requests run formatting, Clippy with warnings denied, the normal test
suite, and every compatibility lane on the pinned Rust 1.98 nightly. The
compatibility job invokes both LuaLS and EmmyLua rendering through the same
semantic extraction result.

Dependabot checks the root crate and latest-lane fixture directories weekly.
It does not update minimum or behavior-boundary lockfiles. A latest-lane update
is accepted only when the complete compatibility job passes and its resolved
version remains inside the lane's declared minor range.

Before releasing, run:

```text
cargo +nightly-2026-06-30 fmt --all -- --check
cargo +nightly-2026-06-30 clippy --all-targets -- -D warnings
cargo +nightly-2026-06-30 test --all-targets
cargo +nightly-2026-06-30 test --test mlua_compatibility -- --ignored --test-threads=1
```

No release may claim the range from compilation alone. Every lane must produce
the reviewed semantic output. A failing minimum lane blocks a release because
it would silently narrow the advertised range. A new stable `mlua` minor is
unsupported until a deliberate `mlua-typegen` minor release adds its native
forms, fixtures, and reviewed output.
