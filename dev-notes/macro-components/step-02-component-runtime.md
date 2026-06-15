# Step 2 — A compile-time component runtime (instantiate + call exports)

- [ ] Done

> **Read first:** `dev-notes/macro-components.md` (driving rules + verbatim
> subagent rules) and `dev-notes/design.md` §6.3. Base your worktree on the
> latest `origin/macro-components` (Step 1's branch).

## Context you need

To run a macro that lives in another component, the compiler must **execute** a
`.wasm` component at build time. The project has no wasm runtime today —
`Cargo.toml` carries `wac-graph`, `wasm-encoder`, `wit-component`, `wit-parser`
(composition + WIT tooling), none of which run code. This step adds the missing
piece and nothing more: load a component, instantiate it, call an exported
function by name with dynamically-typed arguments, get a dynamically-typed result
back. Step 3 layers the `wavelet:meta/macros` contract on top.

## Goal

A native-only module that can instantiate a Component-Model `.wasm` and invoke
its exports with runtime-typed values, smoke-tested against a trivial fixture
component. Sandboxed by construction (§6.3): no WASI, no filesystem, no clock —
the macro guest gets no ambient capabilities.

## Scope

- **Pick and add a runtime.** Recommended: `wasmtime` with the `component-model`
  feature, using its **dynamic** typed-value API (`component::Val` /
  `Func::call` with `&[Val]`) so we can call `manifest`/`expand` without
  codegen-binding to a specific world. Add it under the native-only dependency
  set; it must **not** leak into the `wasm32` (`cdylib`) build used by the docs
  playground — gate the module with `#[cfg(not(target_arch = "wasm32"))]` exactly
  like `emit`/`build`/`wit`/`tools`, and put the dependency behind a
  `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` block (or a feature)
  so `wasm-pack build` for the playground still succeeds. **Verify the playground
  wasm still builds** (`./scripts/regen-examples.sh` runs `wasm-pack`).
- **A small `Engine`/`Store` wrapper** (e.g. `src/host.rs` or extend a `meta`
  module) that:
  - loads a component from bytes or a path,
  - instantiates it with an **empty, capability-free** linker (no WASI), and
  - exposes a way to look up an exported function and call it with `&[Val]`
    returning `Vec<Val>` (or a typed convenience for `result<...>`).
- **A fixture component** for the smoke test. Keep the unit suite hermetic: build
  a tiny component from WAT with `wasm-tools`/`wasm-encoder`, or check a prebuilt
  `.wasm` into `tests/fixtures/`. It should export one trivial function (e.g.
  `add: func(a: s32, b: s32) -> s32`) so the test proves instantiate + call +
  result marshalling without depending on the `tree` type yet.
- **Tests:** instantiate the fixture, call the export, assert the result. Assert
  a clear, actionable error when the bytes aren't a component or the export is
  missing.

## Watch out for

- **Binary-size / build-time cost.** `wasmtime` is a heavy dependency. Confirm
  the native build and `cargo test` time stay acceptable and that nothing pulls
  it into the playground wasm. If the size is a concern, note alternatives
  (`wasmi`, a lighter interpreter) in the handoff — but `wasmtime`'s mature
  component support is the safe default.
- **Determinism & sandboxing.** The guest must not see wall-clock, randomness,
  or the filesystem in a way that makes builds non-reproducible. Start with an
  empty linker; if a future macro needs host functions, that's a deliberate,
  later capability grant — not something to wire in now.
- **Error surface.** Keep the `Result<_, String>` convention used by
  `build.rs`/`emit.rs`/`tools.rs` so callers `?`/`map_err` uniformly.

## Done when

`cargo test` passes; the playground wasm still builds (`wasm-pack` /
`regen-examples.sh`); the runtime module can instantiate the fixture component
and call an export, with actionable errors for the bad-input cases. Nothing in
the compiler pipeline calls it yet — Step 3 is the first consumer.

## Handoff notes

_(fill in: chosen runtime + version + feature flags, how the native/wasm32 split
was done in `Cargo.toml`, the public surface of the runtime module, where the
fixture lives and how it's built, and any build-time/size observations.)_
