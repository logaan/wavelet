# Step 5 — Resolve a `macros: true` import to a `.wasm` and instantiate it

- [x] Done

> **Read first:** `dev-notes/macro-components.md`, `dev-notes/design.md` §6.3 and
> §6.5 (composition / `wkg` / deps), and `dev-notes/decouple-wasi.md` for how
> `wit/deps` and `wkg` already work. Base your worktree on the latest
> `origin/macro-components` (after Steps 1–4).

## Context you need

A `macros: true` import names a *package* (e.g. `acme:html/dsl`). To run its
macros we need the **compiled `.wasm` component** that exports
`wavelet:meta/macros`, not just its WIT. The project already knows how to fetch
dependency *WIT* into `wit/deps` via `wkg` (`src/tools.rs::wkg_wit_fetch`, wired
in `src/build.rs`), and how to resolve a parsed WIT package from `wit/deps`
(`src/witdep.rs`, per `decouple-wasi-todo.md` Step 1). But a macro component is an
executable artifact, so this step is about locating that **binary**.

`src/tools.rs` already wraps `wkg`/`wac`. `Cargo.toml` is at version 0.6.0 with
`wac-graph`/`wit-component`/`wit-parser`/`wasm-encoder`.

## Goal

Given a `macros: true` `ImportInfo`, produce an **instantiated `MacroComponent`**
(Step 3) for it: resolve the package id to a `.wasm` on disk, load it, and
instantiate via the Step 2 runtime. Cache the instance so a package imported once
is instantiated once per build.

## Scope

- **Decide the resolution strategy and document it.** Recommended MVP ordering,
  simplest first:
  1. **Explicit local path** — allow `Import {pkg: "…" macros: true from:
     "path/to/macros.wasm"}` (or a conventional location like
     `wit/macros/<ns>-<name>.wasm`) so a project can point at a locally built
     macro component with no registry. This unblocks Steps 6–10 without network.
  2. **`wkg`-fetched component** — if/when `wkg` can fetch a *component* (not just
     WIT) for the package, use `src/tools.rs` to obtain it. If `wkg` only fetches
     WIT today, note that and leave registry-fetch of macro components as a
     follow-up; the local-path route is sufficient for the feature to land.
  Pick the smallest thing that lets a real macro component be loaded; record the
  choice and the deferred bits in the handoff.
- **A resolver module** (e.g. extend `src/build.rs` or a new `src/macrodep.rs`,
  native-only) mapping `ImportInfo { package, path, macros: true, .. }` → a loaded
  `MacroComponent`, with a per-build cache keyed by package id.
- **Synthesis interaction.** Decide whether a `macros: true` import still
  contributes to the synthesized runtime world. A *pure* macro import (used only
  at compile time) should **not** appear as a runtime import of the consumer
  component. Make `src/wit.rs`/`src/build.rs` skip macro-only imports in
  world synthesis and sibling-edge composition, and test that a file which
  imports a macro library but uses none of its *runtime* exports builds a
  component with no spurious import. (If an import is *both* a runtime dep and a
  macro library, that's an edge case — note it; the common case is macro-only.)
- **Tests:** resolve a fixture macro component from a local path and get a working
  `MacroComponent` (call `manifest()` through it as the assertion). A missing /
  non-macro `.wasm` gives an actionable error.

## Watch out for

- **Build-time only.** Resolution + instantiation happen during read/expand, not
  at runtime of the produced component. Make sure none of this lands in the
  `wasm32` playground build (native-only gating, like Steps 2–3).
- **Don't regress `decouple-wasi`'s import resolution.** The existing `wit/deps`
  and sibling-`.wvl` resolution (`src/build.rs`) must keep working unchanged for
  non-macro imports.

## Done when

`cargo test` passes; a `macros: true` import pointing at a fixture macro
component resolves to an instantiated `MacroComponent`; a macro-only import does
not show up as a runtime import in the synthesized world; actionable errors for
the missing-binary case.

## Handoff notes

### Resolution strategy chosen (and what was deferred)

**Explicit local path, MVP.** `wkg` only fetches dependency *WIT* into
`wit/deps` (`src/tools.rs::wkg_wit_fetch` runs `wkg wit fetch --type wit`); it
does **not** fetch executable *components*. So registry-fetch of a macro
component is **deferred** as a follow-up. For now a macro component is located on
disk, in this order (see `src/macrodep.rs`):

1. **Explicit `from:` path** — `Import {pkg: "acme:html/dsl" macros: true from:
   "path/to/macros.wasm"}`. Relative paths resolve against the **project root**
   (the parent of `src/`, mirroring `build.rs::project_root`); absolute paths are
   taken as-is.
2. **Conventional location** — `wit/macros/<ns>-<name>.wasm` under the project
   root, where `<ns>-<name>` is the import's *package path* with `:` and `/`
   mapped to `-` (e.g. `acme:html/dsl` → `wit/macros/acme-html.wasm`). Note the
   key uses `ImportInfo.package` (the package part, version-stripped — currently
   `acme:html`, i.e. it drops the `/dsl` interface segment), so the convention
   file is `acme-html.wasm`.

A `from:` field was added: `pub from: Option<String>` on `ImportInfo`, parsed in
the `import-MACRO` record arm as `("from", Node::Str(s))`, mirroring how Step 4
added `macros:`. It is an **ordinary record field** — no lexer or
syntax-highlighting change is needed (confirmed: the lexer doesn't special-case
record keys; `pkg`/`as`/`macros`/`from` are all just identifiers).

### The resolver's public surface + caching

New native-only module `src/macrodep.rs` (gated `#[cfg(not(target_arch =
"wasm32"))]` in `lib.rs`, like `host`/`macros`/`emit`/`build`):

- `MacroResolver::new(root: impl Into<PathBuf>) -> Self` — `root` is the project
  root for resolving relative `from:` paths and the conventional location.
- `MacroResolver::resolve(&mut self, import: &ImportInfo) -> Result<&mut
  MacroComponent, String>` — locates + instantiates the macro component
  (`MacroComponent::from_file`, which already verifies the `wavelet:meta/macros`
  export) and returns a mutable handle.

**Caching:** a `HashMap<String, MacroComponent>` keyed by `ImportInfo.package`
(version-stripped package path). A package imported once is instantiated once per
build; two imports of the same package — even under different aliases — share one
instance. (Tested: resolving the same package twice leaves `cache.len() == 1`.)
`MacroResolver` is `#[derive(Default)]`-able and otherwise has no global state.

This step builds and **caches** the resolver but does not yet *call* it from the
build pipeline — wiring `resolve()` into read/expand is Steps 6 (arity
registration from `manifest()`) and 7 (route expansion through `expand`). It is
fully unit-tested standalone against the Step 3 fixture (`tests/fixtures/
macros.wasm`): resolve-from-`from:`, resolve-from-convention, cache-once,
missing-binary error, non-macro-component (`add.wasm`) rejection.

### How world synthesis now treats macro-only imports

Added `wit::is_macro_only(imp) -> bool` (currently just `imp.macros`). A pure
macro import is **compile-time only** and contributes **no runtime import** to
the consumer component. It is now skipped in three places:

- `src/wit.rs::synthesize_info` — `if is_macro_only(imp) { continue }` before the
  `wasi:`/sibling import-emission branches, so the `world { … }` block has no
  `import` line for it. (Tested: a file importing a macro library but using none
  of its runtime exports synthesizes a world with **no** `import acme:html/dsl`,
  while the same import *without* `macros: true` still appears.)
- `src/wit.rs::has_host_deps` — a macro-only import is not counted as a host dep,
  so it doesn't trigger a `wkg wit fetch` (it has no registry WIT to fetch).
- `src/build.rs` — both the per-unit dependency-resolution loop (it adds no
  `Dep`, so the import is **not** required to be satisfied by the build set or
  `wit/deps`) and `compose_units`' sibling-edge collection (it is never a wiring
  edge in the composed `app.wasm`).

**Both-runtime-and-macro edge case:** an import that is *both* a runtime
dependency *and* a macro library is **unsupported for now**. `is_macro_only`
treats any `macros: true` import as macro-only, so such an import would be
dropped from the runtime world entirely. Supporting it would mean emitting both a
runtime import (in the world / composition) *and* a compile-time macro instance.
This is noted as a follow-up; the common case (a pure macro library) is the one
implemented. The single chokepoint `wit::is_macro_only` is where that distinction
would later be refined (e.g. a separate `runtime: true` flag).

### How a project points at a macro component for now

Build the macro library to a Component-Model component that exports
`wavelet:meta/macros@0.1.0` (built for `wasm32-unknown-unknown`, no WASI, so it
instantiates under the empty sandbox linker — see Step 3), then either:

- set `from:` on the import — `Import {pkg: "acme:html/dsl" macros: true from:
  "build/dsl.wasm"}` (path relative to the project root), **or**
- drop the component at `wit/macros/<ns>-<name>.wasm` (e.g.
  `wit/macros/acme-html.wasm`) and import it with just `Import {pkg:
  "acme:html/dsl" macros: true}`.

A `macros: true` import with no resolvable binary fails with an actionable error
naming both the `from:` path tried and the conventional location.

### Notes for Steps 6/7

- `resolve()` returns `&mut MacroComponent`; call `.manifest()` (Step 6) and
  `.expand(name, args)` (Step 7) on it. The borrow is `&mut`, so hold the
  resolver and re-`resolve()` (cheap cache hit) per use site if you need
  non-overlapping borrows.
- Where the resolver gets constructed in the pipeline is open: the natural home
  is read/expand, which currently has no project-root handle the way `build.rs`
  does. `build.rs::project_root(paths)` is the existing way to derive the root;
  expansion may need the same path threaded in (or default to `.`).
- The cache key is `ImportInfo.package` (the version-stripped *package* part,
  which presently drops the `/dsl` interface segment). If Step 8's qualified
  refs need per-*interface-path* resolution, revisit the key (use `import.path`).
