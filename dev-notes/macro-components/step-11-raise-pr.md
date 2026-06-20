# Step 11 — Final verification and raise the PR

- [x] Done

> **Read first:** `dev-notes/macro-components.md` and skim every step's "Handoff
> notes" — those are your raw material for the PR description. Base your worktree
> on the latest `origin/macro-components` (after Steps 1–10). This step does **no
> feature work**: it verifies the whole feature is green and opens the single PR.

## Context you need

This is the one place in the worklist that opens a pull request. All prior steps
pushed to the shared `origin/macro-components` integration branch and did **not**
open PRs. The feature is tracked as **one** PR from `macro-components` → `main`; a
human reviews and merges (per `CLAUDE.md`: agents open PRs, humans merge).

## Goal

Confirm the entire feature is finished and green, then open a clear, reviewable
pull request for `macro-components` → `main`.

## Scope

- **Full verification, from a clean state:**
  - `cargo test` — all green (lib + examples + any new integration tests).
  - `./scripts/regen-examples.sh` — green, and confirm the regenerated artifacts
    (`docs/examples.json`, `docs/src/wasm/*`) are already committed and the tree
    is clean afterwards (no drift).
  - Confirm the **playground wasm still builds** (regen runs `wasm-pack`) — i.e.
    the native-only `wasmtime`/runtime split (Step 2) never leaked into the
    `wasm32` build.
  - Sanity-build the macro-component end-to-end example from Step 10 and confirm
    it runs.
  - Re-read each step's box: every step 1–10 ticked, every "Handoff notes"
    filled.
- **Rebase/merge hygiene.** Ensure `macro-components` is up to date with
  `origin/main` (rebase if `main` moved; resolve conflicts cleanly — if you can't
  resolve safely, stop and report rather than force-pushing over others).
- **Open the PR** with `gh pr create` from `macro-components` to `main`:
  - **Title:** something like `feat: run macros defined in other components`.
  - **Body:** what the feature does (import `macros: true`, `wavelet:meta/macros`,
    compile-time instantiation, expansion routing, writing macro libraries in
    Wavelet), a short tour of the steps/commits, the new runtime dependency
    (`wasmtime` or whatever Step 2 chose) and its native-only gating, anything a
    reviewer should look at closely, and any **deferred follow-ups** gathered from
    the handoff notes (e.g. registry-fetch of macro components, emitter strategy
    B from Step 9, LSP completion of foreign macros, hygiene/`gensym` limits).
  - End the PR body with the required trailer:

    ```
    🤖 Generated with [Claude Code](https://claude.com/claude-code)
    ```

## Watch out for

- **Do not merge.** Open the PR and stop; a human merges.
- **Do not push to `main`.**
- **Tick this step's box and fill its handoff notes** (the PR URL) in a final
  commit on `macro-components` before/with opening the PR.

## Done when

`cargo test` and `./scripts/regen-examples.sh` are green, the tree is clean, and a
single PR from `macro-components` → `main` is open with a complete description and
the deferred-follow-ups list. The PR URL is recorded here.

## Handoff notes

**PR:** https://github.com/logaan/wavelet/pull/12 (`macro-components` →
`main`; title `feat: run macros defined in other components`). A human reviews
and merges — this step did not merge.

**Final verification (from the up-to-date `origin/macro-components`, be31e59).**

- `cargo test` — **all green, 138 tests, 0 failed.** Breakdown: lib 110;
  integration `examples` 8, `produced_macros` 7 (incl. the Step 10 e2e
  `worked_e2e_build_through_conventional_location` and the opt-in
  `reproduce_component_from_source`), `wit_deps` 5, `wkg_populate` 4, plus the
  in-`src/lib.rs` componentize tests (3 + 1) and 0 doctests.
- `./scripts/regen-examples.sh` — **green.** It ran `wasm-pack build --target
  web` (the playground wasm), `node gen-examples.mjs`, then the full `cargo
  test`, all passing. **`git status` was clean afterwards — no drift**; the
  committed `docs/examples.json` and `docs/src/wasm/*` already matched. No
  artifacts needed re-committing.
- **Playground wasm still builds** — the `wasm-pack` step in regen succeeded, so
  the native-only `wasmtime`/runtime split (Step 2) and the `playground`-feature
  gating of the wasm-bindgen bindings (Step 9) never leaked into the `wasm32`
  build.
- **Step 10 end-to-end** — `worked_e2e_build_through_conventional_location`
  (full `build::build_files` of a consumer that imports the Wavelet-authored
  macro library from the conventional `wit/macros/demo-macros.wasm`, uses its
  macro bare + qualified, and emits a real `\0asm` component) passes.
- **Steps 1–10 all ticked `- [x]`** on `origin/macro-components` and every
  "Handoff notes" section is filled. No feature work was redone.
- **Rebase/merge hygiene** — `origin/main` is at 23e0c31, which is exactly the
  merge-base of `main` and `macro-components` (main is a strict ancestor). **Main
  has not moved since the branch point, so no rebase was needed**; the branch
  merges cleanly.

**Consolidated deferred follow-ups (carried into the PR body):**

1. **Registry-fetch of macro components.** `wkg` only fetches dependency *WIT*
   into `wit/deps`, not executable components; today a macro component is located
   on disk (explicit `from:` or the conventional `wit/macros/<ns>-<name>.wasm`).
   Fetching/publishing macro components from a registry is future work. (Step 5/9)
2. **Emitter strategy B.** Produced macro components use strategy A
   (interpreter-in-a-component) for fidelity. Compiling macro bodies directly to
   wasm functions (strategy B) is the deferred performance/cleanliness path; it
   needs a large `emit.rs` extension over `tree`. (Step 9)
3. **LSP completion/hover of foreign macros.** Runtime imports are surfaced, but
   `macros: true` imports contribute compile-time TitleCase macros, not
   functions, so they are not offered in editor assistance. Recorded as a `GAP`
   on `imported_completions` in `tooling/wavelet-lsp/src/analysis.rs`. (Step 10)
4. **Cross-call gensym / hygiene counter reset.** gensym is unique *within* one
   expansion, but a produced component uses a fresh `Interp` per `expand` call so
   the counter resets across calls (local `expand_file` is monotonic across a
   whole file). Threading a gensym seed across calls is future work. (Step 9)
5. **Both-runtime-and-macro import edge case.** An import that is *both* a runtime
   dependency *and* a macro library is unsupported: `is_macro_only` treats any
   `macros: true` import as macro-only and drops it from the runtime world.
   `wit::is_macro_only` is the single chokepoint where a future `runtime: true`
   refinement would land. (Step 5)
6. **`{}`-in-export-body wasm-backend limitation.** A macro whose expansion
   contains `{}` (empty record / flag literal — e.g. `unless` → `If c {} body`)
   can't yet be the body of an *exported* function, because the wasm backend
   doesn't emit flag literals. The e2e uses `identity` to sidestep this. (Step 10)
7. **Fine-grained per-macro-library capability wiring.** Macros run sandboxed
   (capability-free linker), but capability grants are coarse today; per-library
   wiring is the next supply-chain refinement. (Step 10)
