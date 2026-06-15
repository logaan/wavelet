# Step 5 — Resolve a `macros: true` import to a `.wasm` and instantiate it

- [ ] Done

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

_(fill in: the resolution strategy chosen and what was deferred, the resolver's
public surface + caching, exactly how world synthesis now treats macro-only
imports, and how a project is expected to point at a macro component for now.)_
