# Step 7 — Route expansion through the component's `expand`

- [ ] Done

> **Read first:** `dev-notes/macro-components.md`, `dev-notes/design.md` §6.3 and
> §9 (the pipeline: "the expander runs macros to fixpoint, instantiating macro
> components on demand"). Base your worktree on the latest
> `origin/macro-components` (after Steps 1–6). **This is the keystone step** —
> after it, foreign macros actually run.

## Context you need

There are **two** expanders and both must learn about foreign macros:

1. **Ahead-of-time expander** — `src/expand.rs`. `expand_file` walks the form
   tree; `expand_form` (`src/expand.rs:42`–) sees a `Call` whose head `Sym` is a
   macro and, if it's a **local** `Value::Macro` in the env, calls
   `interp.expand_once(&mac, arena, payload)` and recurses to fixpoint
   (`src/expand.rs:55`–`60`). `quote-MACRO`/`quasi-MACRO` are skipped
   (`src/expand.rs:52`). This pass feeds WIT synthesis and the wasm emitter.
2. **Lazy interpreter expander** — `src/interp.rs` (`expand_once` ~`:300`,
   `expand_macro` ~`:286`, macro-call dispatch ~`:127`). Used by `wavelet run`
   and the `expand` builtin.

A foreign macro is **not** a `Value::Macro` in the env — it lives in an
instantiated `MacroComponent` (Steps 3/5) and is identified by the
`<name>-MACRO` head that Step 6 registered an arity for. Expanding it means:
marshal the call's argument forms (the `payload`) into a `Tree` (Step 1),
call `MacroComponent::expand(name, &tree)` (Step 3), lift the returned `Tree`
back into the arena, and recurse — to fixpoint, exactly like a local macro.

## Goal

When an expander hits a TitleCase/`-MACRO` head that resolves to a foreign macro
component rather than a local macro, it expands it by calling the component's
`expand`, splices the result back into the tree, and continues to fixpoint —
producing a tree indistinguishable from one a local macro would have produced.

## Scope

- **A lookup that unifies local and foreign macros.** Given a head name, the
  expander must find either the local `Value::Macro` (current behaviour) or the
  `MacroComponent` that owns that macro name (from the Step 5 resolver, keyed by
  the manifest names registered in Step 6). Decide where this registry lives so
  both `expand.rs` and (optionally) `interp.rs` can reach it. The ahead-of-time
  `expand.rs` is the **primary** target — it's what the build pipeline uses.
- **The foreign-expand path in `src/expand.rs`:**
  - convert `payload` (the args form) → `Tree` via Step 1,
  - call `MacroComponent::expand(name_without_suffix, &tree)`,
  - on `Ok(tree)`: convert back to `(arena, root)`, copy into the output arena,
    and **recurse** through `expand_form` so the expansion is itself expanded,
  - on `Err(msg)`: return an actionable error naming the macro (mirror the
    existing `format!("expanding `{}`: …", name.trim_end_matches("-MACRO"))`).
- **Quote/quasi still opaque.** Preserve the existing rule that forms under
  `quote-MACRO`/`quasi-MACRO` are not expanded (`src/expand.rs:52`).
- **The interpreter expander (`interp.rs`).** Decide whether `wavelet run` and the
  `expand` builtin also route through foreign components. The native `run` path
  benefits; the wasm playground can't (no runtime). Recommend: wire the
  ahead-of-time `expand.rs` fully (covers `build`), and make the interpreter path
  either delegate or degrade gracefully when no resolver is present. Document the
  decision; don't break the existing `expand` builtin tests
  (`src/lib.rs` `eval_expand_builtin`).
- **Tests:** an end-to-end-ish test in the build/expand layer — a file importing
  the fixture macro library, using a foreign macro, runs `expand_file` and
  produces the expected expanded tree; then it componentizes (reuse the existing
  `expand::expand_file` + componentize assertion pattern at the bottom of
  `src/lib.rs`). Include a macro whose expansion **contains another macro call**
  to prove fixpoint recursion. Cover the `expand` → `result::err` error path.

## Watch out for

- **Fixpoint & termination.** Foreign expansion can loop just like local
  expansion. Match the existing recursion structure; if you add a depth guard,
  keep it consistent with how local macros behave (today there's none — note if
  you add one).
- **Arena identity.** `expand_once` returns a *new* arena; the foreign path
  likewise yields a fresh `(arena, root)`. Follow `expand.rs`'s existing
  copy-into-`out` discipline (`copy_form`/`descend`) so node ids stay valid.
- **Native/wasm split** again: the foreign-expand path is native-only. In the
  playground, only local macros exist.

## Done when

`cargo test` passes; `expand_file` expands foreign macros from a fixture macro
component to fixpoint and the result componentizes; the error path is covered;
local-macro behaviour and the `expand` builtin are unchanged.

## Handoff notes

_(fill in: where the unified macro registry lives, exactly how `expand.rs`
dispatches local vs foreign, what you decided for the `interp.rs`/`run` path, any
fixpoint/depth handling, and the shape of the end-to-end test.)_
