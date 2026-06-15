# Step 11 — Final verification and raise the PR

- [ ] Done

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

_(fill in: the PR URL, final verification results, and the consolidated list of
deferred follow-ups carried into the PR body.)_
