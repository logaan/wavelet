# Step 2 — A compile-time component runtime (instantiate + call exports)

- [x] Done

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

**Chosen runtime.** `wasmtime` `45` (resolved to `45.0.2`), declared as:

```toml
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
wasmtime = { version = "45", default-features = false, features = ["runtime", "cranelift", "component-model"] }
```

`default-features = false` trims the crate; `component-model` enables the
Component-Model + the dynamic `component::Val` / `Func::call(&[Val])` API;
`cranelift` is re-enabled explicitly because component *lifting* needs a compiler
backend; `runtime` is the actual execution engine. No `async`, no `wasi`, no
`wat` features — we marshal raw bytes and the fixture is prebuilt.

**Native/wasm32 split.** `wasmtime` lives under a
`[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` table — the inverse of
the existing `cfg(target_arch = "wasm32")` table that carries the playground's
`wasm-bindgen`. The module is gated `#[cfg(not(target_arch = "wasm32"))]` in
`lib.rs`, exactly like `emit`/`build`/`wit`/`tools`/`meta`. Verified the split:
`cargo tree --target wasm32-unknown-unknown -i wasmtime` prints *"nothing to
print"* (absent), while `cargo tree -i wasmtime` shows it on the native target.
`wasm-pack build --target web` still succeeds and `wasmtime`/`cranelift` never
appear in its compile log.

**Public surface** (`src/host.rs`, native-only):

- `pub use wasmtime::component::Val;` — re-exported so callers build/match
  argument and result values without naming `wasmtime` directly.
- `pub struct HostComponent` — owns the `Engine`, `Store<()>` (no host state),
  and `Instance`.
  - `HostComponent::from_bytes(&[u8]) -> Result<Self, String>` — load + decode +
    instantiate from raw component bytes.
  - `HostComponent::from_file(&Path) -> Result<Self, String>` — read a file then
    `from_bytes`.
  - `HostComponent::engine(&self) -> &Engine` — for callers that need the engine
    to build component values (rarely needed).
  - `HostComponent::func(&mut self, name) -> Result<Func, String>` — look up an
    export by name, actionable error if absent.
  - `HostComponent::call(&mut self, name, &[Val]) -> Result<Vec<Val>, String>` —
    look up + invoke + return results; sizes the result buffer from
    `func.ty(&store).results().len()`.

  Instantiation always uses an **empty `Linker<()>`** — no WASI, no host imports
  of any kind. A component that imports anything fails at `from_bytes` with the
  missing-import diagnostic. This is the §6.3 sandbox; granting a capability
  later is a deliberate, explicit act.

**Fixture.** `tests/fixtures/add.wasm` (153 bytes), a Component-Model component
exporting `add: func(a: s32, b: s32) -> s32`. Source is checked in beside it as
`tests/fixtures/add.wat`; regenerate with
`wasm-tools parse tests/fixtures/add.wat -o tests/fixtures/add.wasm`. The `.wasm`
is committed so the unit suite is hermetic (no tool needed at `cargo test` time).
Three `#[cfg(test)]` tests in `host.rs` cover: instantiate + call + result (incl.
a second call on the same instance), a missing export ("no exported function
`subtract`"), and bad input (random bytes *and* a bare core module both rejected
with "not a valid WebAssembly component").

**Build-time / size observations.**

- `wasmtime` + `cranelift` is heavy: a cold native build (deps + crate) was
  ~30s of wall time on this machine (Apple Silicon, ~5x parallelism). Incremental
  rebuilds of just `wavelet` are ~2.5s. `cargo test` end-to-end stays well under
  a minute and individual host tests run in <1ms (engine creation dominates, a
  few seconds amortized across the suite — note the lib-test binary's ~4.6s line
  is mostly the wasmtime-backed tests warming the engine).
- This pulls a large transitive tree (cranelift codegen, object, regalloc, etc.)
  into the **native** build only. If build-time/binary-size becomes a concern, a
  lighter alternative is **`wasmi`** (a pure-Rust interpreter) — but as of writing
  `wasmi`'s Component-Model support is less mature than `wasmtime`'s, so
  `wasmtime` is the safe default for the macro-host work. Another option is to
  hide it behind an off-by-default cargo feature if non-macro builds shouldn't pay
  for it.

**API notes for Step 3 (and anyone touching `host.rs`).**

- `wasmtime` 45's `Func::call` signature is
  `call(&mut store, params: &[Val], results: &mut [Val]) -> Result<()>` — you
  pre-size `results` (we use `func.ty(&store).results().len()`).
- `Func::post_return` is **deprecated and a no-op** in 45 ("no longer needs to be
  called"); cleanup is internal, so the same instance can be called repeatedly
  without it. We don't call it.
- `HostComponent` is intentionally **not** `Debug` (it wraps non-`Debug`
  `wasmtime` handles), so `Result::unwrap_err` on a `HostComponent` won't compile
  — tests extract the error via a small `match` helper instead.
- Step 3 maps `meta::Tree`/`meta::Node` (Step 1) to/from `Val` to call
  `manifest`/`expand`. The `Val` variants you'll need mirror the wire `node`:
  `Val::Bool/S32/S64/Float64/Char/String`, plus `Val::List`, `Val::Record`,
  `Val::Variant`, `Val::Tuple`, `Val::Flags` for the aggregate `tree` shape.
