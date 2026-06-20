# Step 9 — Produce a `wavelet:meta/macros` component from a Wavelet macro file

- [x] Done

> **Read first:** `dev-notes/macro-components.md`, `dev-notes/design.md` §6.2–§6.3
> and §9 (pipeline / self-hosting). Base your worktree on the latest
> `origin/macro-components` (after Steps 1–8). **This is the largest and most
> independent step** — it is the *producer* side. Steps 2–8 are testable against a
> hand-built fixture; this step lets a macro library be written in Wavelet itself.

## Context you need

So far the consumer can import and run a macro component (Steps 1–8), but the only
macro components are hand-built fixtures. The design's payoff is that a `.wvl`
file full of `DefMacro`s can be **compiled into** a component exporting
`wavelet:meta/macros` (`manifest` + `expand`), so Wavelet dogfoods its own macro
system and the Step 10 end-to-end example uses a real, Wavelet-authored library.

The hard part: the produced component's `expand(name, args: tree)` must, at the
*consumer's* build time, evaluate the named macro's body over `args`. There are
two strategies — **decide and document which** (this is the central design call
of the step):

- **(A) Interpreter-in-a-component.** The macro component bundles the Wavelet
  interpreter (compiled to wasm) plus the macro definitions as data; `expand`
  reads the `tree`, runs `interp`/`expand_once` on the named macro, returns the
  result `tree`. Reuses the existing interpreter (`src/interp.rs`,
  `expand_once`), which is the semantics oracle — least risk of divergence. Cost:
  the interpreter must run in the wasm guest and the macro bodies must be carried
  along.
- **(B) Compile macro bodies to wasm functions.** The emitter (`src/emit.rs`)
  compiles each `DefMacro` body as a function over `tree`, and `expand` dispatches
  by name. Cleaner artifact, but requires the wasm backend to compile
  form-manipulating code (`Quasi`/`Unquote`/`Splice`/`gensym` over `tree`
  values), which is a big emitter extension.

**Recommendation:** start with **(A)** — it leans on the interpreter that already
defines macro semantics and gets a working producer soonest; note (B) as the
performance/cleanliness follow-up. If (A) proves impractical within the step,
report and stop rather than half-doing (B).

## Goal

`wavelet build` (or a dedicated mode/flag) turns a `.wvl` file whose top level is
`DefMacro`s into a component that exports `wavelet:meta/macros`, such that the
Step 1–8 consumer can import it with `macros: true` and use its macros.
`manifest()` reports each macro's `(name, arity)`; `expand(name, args)` returns
the macro's one-step (or fixpoint — match the local-macro contract) expansion.

## Scope

- **Detect / declare a macro-library file.** Decide how a file opts in to being a
  macro component — e.g. a `Target` of `wavelet:meta/macros`, or simply "a file
  that only defines macros and exports none of its own runtime funcs." Document
  the trigger.
- **Synthesize the world.** Emit a component targeting the `wavelet:meta/macros`
  world (the WIT from Steps 1+3). Reuse `src/wit.rs` synthesis machinery where
  possible; the world is fixed (`manifest`/`expand`), so this is mostly wiring
  exports to the chosen strategy.
- **Implement `manifest`.** Derived from the file's `DefMacro` arities — the same
  data `src/reader.rs::register_if_def_macro` computes.
- **Implement `expand`** per the chosen strategy (A or B above), marshalling
  `tree` ↔ forms with Step 1's conversion. The result must match what the local
  ahead-of-time expander (`src/expand.rs`) produces for the same macro on the same
  args (they share the interpreter under strategy A).
- **CLI / pipeline surface.** Whatever invocation builds it (a `build` that
  detects the target, or `wavelet build --macros`, etc.) — keep it consistent
  with the existing `src/main.rs` subcommands (`build`, `compose`, `wit`, …).
- **Tests:** build a small Wavelet macro library, then **consume it** through the
  Step 1–8 path (instantiate, `manifest`, `expand`, use a macro in a second file)
  and assert the expansion matches running the same `DefMacro` locally via
  `expand_file`. This is the real dogfood test.

## Watch out for

- **Equivalence with local expansion.** The whole point of the interpreter being
  the semantics oracle (`CLAUDE.md`) is that a macro must mean the same thing
  whether expanded locally or via a component. Assert this directly.
- **`gensym`/hygiene.** Macros use `gensym` for hygiene discipline (§6.3). Make
  sure `gensym` works inside the component's `expand` (fresh names per call) and
  document any limitation.
- **Build-time vs runtime.** The produced component runs at the *consumer's*
  build time; it should need no WASI / ambient capability (consistent with the
  capability-free linker from Step 2).
- **Scope discipline.** This step is big — keep it to producing + dogfooding one
  component. Don't fold in registry publishing (`wkg` push) or the docs; those
  are Step 5's deferred bits and Step 10.

## Done when

`cargo test` passes (`./scripts/regen-examples.sh` if you added a documented
example); a Wavelet macro file compiles to a `wavelet:meta/macros` component that
the consumer path can import and expand, and its expansions match local
expansion.

## Handoff notes

**Strategy: A (interpreter-in-a-component).** The produced component bundles the
Wavelet interpreter (the `wavelet` crate, which already compiles to `wasm32` for
the docs playground) plus the macro file's source as embedded data, and runs the
macros through the interpreter at the consumer's build time. Chosen because it
reuses `interp.rs`/`expand_once` — the semantics oracle (`CLAUDE.md`) — so a
macro means *exactly* the same thing expanded locally (`expand::expand_file`) or
through a produced component, with the least divergence risk. **Strategy B
(compiling macro bodies to wasm functions) is deferred** as the
performance/cleanliness follow-up; it needs a large `emit.rs` extension to
compile `Quasi`/`Unquote`/`Splice`/`gensym` over `tree`.

**How a file declares it's a macro library:** a `.wvl` whose top level is a
`Package` declaration plus `DefMacro`s *only* — no `Export`, no runtime
`Def`/`DefType`/`Import`, no bare expressions (`macrobuild::is_macro_library`).
This is the step's "a file that only defines macros and exports none of its own
runtime funcs" trigger; it needs **no new syntax or lexer/highlighting token**.
`wavelet build` detects such a file and routes it to the producer instead of the
ordinary emit path (the file is neither macro-expanded nor WIT-synthesised — its
`DefMacro`s are the component's payload).

**Build invocation:** `wavelet build src/macros.wvl -o out` — same command as any
component. It emits `out/<package-path>.wasm` (`:`→`-`), exporting
`wavelet:meta/macros@0.1.0`. No new flag. A consumer then points an `Import {…
macros: true from: "…/<package-path>.wasm"}` at it (or drops it at the
conventional `wit/macros/<ns>-<name>.wasm`).

**Mechanics:** a checked-in guest crate `tools/macro-guest/` depends on the
`wavelet` crate (so the interpreter is *in* the guest) and exports
`wavelet:meta/macros` via `wit-bindgen`. The producer
(`macrobuild::build_macro_component`): (1) writes the macro source to a temp file
and sets `WAVELET_MACRO_SRC`; (2) runs `cargo build --release --target
wasm32-unknown-unknown` in the guest (its `build.rs` `include_str!`s the source
from `OUT_DIR`); (3) componentizes the core module **in-process** with
`wit_component::ComponentEncoder` (the metadata wit-bindgen embeds is enough — no
`wasm-tools` shell-out). Built for **`wasm32-unknown-unknown` (no WASI)** so it
instantiates under the consumer's empty, capability-free linker (Step 2).

To keep the guest free of `wasm-bindgen`'s `__wbindgen_placeholder__` imports (a
component can't carry them), the `wavelet` crate's playground bindings
(`src/wasm.rs` + `wasm-bindgen`) moved behind a **default-on `playground`
feature**; the guest depends on `wavelet` with `default-features = false`. The
docs `wasm-pack` build is unchanged (default features), and both `cargo check
--target wasm32 --lib` and `--no-default-features` pass.

**`manifest`/`expand`:** thin guest wrappers over a new **non-gated** shared
module `src/macrolib.rs` (called by both the wasm guest and native tests):
- `manifest(src)` reads the source and reports each top-level `DefMacro`'s
  `(unsuffixed-name, arity)` — the same `{params}`-count arity
  `reader::register_if_def_macro` computes.
- `expand_one(src, name, call_arena, call_id)` seeds an env with the builtins +
  the file's `DefMacro`s (exactly as `expand_file` does), looks up
  `<name>-MACRO`, and runs **one** `expand_once` over the call form's `items[1..]`
  (honouring the PINNED args-tree contract: `args` is the whole call tup). One
  step, because the *consumer* recurses to fixpoint (`macrodep::expand_call`).
The guest marshals the canonical-ABI `tree` ↔ `form::Arena` itself (the native
`meta` module is wasm-gated), mirroring `meta.rs` against the wit-bindgen types.

**gensym/hygiene:** `gensym` **works** inside the component — a fresh `Interp`
per `expand` call, so multiple gensyms within one expansion get distinct names
(`[g0-gen, g1-gen]`, matching local expansion, verified). **Known limitation:**
the counter resets to 0 per `expand` call, whereas local `expand_file` shares one
`Interp` across a whole file, so gensym is monotonic across *all* calls there. So
gensym names are unique *within* an expansion (the hygiene that matters per §6.3)
but may repeat *across* separate component calls. Tightening this (threading a
gensym seed across calls) is future work and aligns with §10's "hygiene is future
work".

**Tests:** `tests/produced_macros.rs` is the dogfood test. It consumes a
**prebuilt, checked-in** component `tests/fixtures/produced-macros.wasm` (built
from `tests/fixtures/produced-macros.wvl`) through the full Step 1–8 consumer
path — `MacroComponent` `manifest`/`expand`, plus the reader/`FileExpander` route
with a `macros: true` import — and asserts every expansion equals local
`expand_file` of the same macro. Hermetic: no wasm toolchain needed for `cargo
test`. The producer itself (which needs the `wasm32` target + `cargo`) is
exercised by an **opt-in** test gated behind
`WAVELET_TEST_BUILD_MACRO_COMPONENT=1` (`reproduce_component_from_source`), so the
suite never hard-depends on a toolchain that may be absent in CI — exactly like
the Step 3 fixture. `src/macrolib.rs` and `src/macrobuild.rs` also carry unit
tests.

**Deferred:** strategy B; registry publishing (`wkg` push of produced
components); docs/highlighting/CHANGELOG-beyond-Unreleased and the end-to-end
example (Step 10). Tightening cross-call gensym hygiene.

**For Step 10:** the end-to-end example can now prefer a *Wavelet-authored* macro
library — `wavelet build`-ed via the producer here — over the hand-built Rust
fixture. `tests/fixtures/produced-macros.wvl` is a minimal working library
(`identity`/1, `unless`/2); the e2e example should write its own `.wvl` macro
library, build it, and consume it. Regeneration of the checked-in `.wasm` is
documented in `tests/fixtures/produced-macros.README.md` and
`tools/macro-guest/README.md`.
