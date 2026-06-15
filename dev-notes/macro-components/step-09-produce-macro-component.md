# Step 9 — Produce a `wavelet:meta/macros` component from a Wavelet macro file

- [ ] Done

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

_(fill in: strategy A vs B and why, how a file declares it's a macro library, the
build invocation, how `manifest`/`expand` are implemented, the gensym/hygiene
state, and what was deferred — e.g. strategy B, registry publishing.)_
