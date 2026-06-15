# Step 10 — End-to-end example + docs + highlighting + CHANGELOG

- [ ] Done

> **Read first:** `dev-notes/macro-components.md`, `dev-notes/design.md`
> §6.2–§6.3, and the `CLAUDE.md` sections "docs/examples.json is a single source
> of truth", "A language change is not done until the downstream surfaces are
> checked", and "CHANGELOG.md drives the GitHub release notes". Base your worktree
> on the latest `origin/macro-components` (after Steps 1–9).

## Context you need

The machinery now exists end to end: write a macro library in Wavelet (Step 9),
build it into a `wavelet:meta/macros` component, import it from another file with
`Import {… macros: true}` (Steps 4–6), and have its TitleCase macros expand
through the component at build time (Steps 7–8). This step makes the feature
**real for users**: a worked example, prose, highlighting, and release notes.

`CLAUDE.md` is explicit that a language change isn't finished until the
downstream surfaces are checked. Go through each.

## Goal

A documented, runnable end-to-end macro-component example, with all downstream
surfaces (docs prose, generated examples, syntax highlighting, CHANGELOG, LSP)
checked and updated where affected.

## Scope

- **An end-to-end example/integration test.** A macro library `.wvl` + a consumer
  `.wvl` that imports it `macros: true` and uses a foreign macro; build/compose
  and run it (prefer the Step 9 Wavelet-authored library over a hand fixture).
  Put it where the project keeps such tests (alongside the existing build/compose
  tests; check `tests/` and the componentize tests in `src/lib.rs`).
- **Docs prose** (`docs/docs/`): document macro components — `Import {… macros:
  true}`, the `wavelet:meta/macros` interface, writing a macro library, qualified
  refs / aliasing / collisions. Update or extend the section that currently flags
  macro components as not-yet-implemented (see `dev-notes/docs-todo.md`, which
  marks "macro components" as `not-yet-implemented`).
- **`docs/examples.json`** — if you add a runnable `<Playground>` example,
  author it in `docs/scripts/gen-examples.mjs` and regenerate via
  `./scripts/regen-examples.sh`. **Caveat:** the playground runs the
  wasm-compiled interpreter, which has **no component runtime** — a foreign-macro
  example may not be runnable in the browser. If so, present it as a static
  ```` ```wavelet ```` code block (not a live example) and say why; don't add a
  playground example that can't execute there.
- **Syntax highlighting** — only if Step 8 introduced a new token class (e.g. a
  new qualified-macro spelling). If the lexer was untouched, **confirm and state
  that no grammar change is needed.** If it changed, update all three grammars
  per `CLAUDE.md`: `docs/src/prism/wavelet.js`, the `tooling/neovim` submodule
  (commit+push in `wavelet.nvim`, then bump the pointer), and `tooling/vscode/`.
- **LSP** (`tooling/`) — consider whether diagnostics/completion/hover should know
  about imported macros (e.g. completing foreign TitleCase macros, hovering their
  arity). At minimum, note the gap; implement if it's small. Record the decision.
- **CHANGELOG.md** — add the user-visible feature under `## [Unreleased]`
  (`Added`): running macros defined in other components, `Import {… macros:
  true}`, `wavelet:meta/macros`, writing macro libraries in Wavelet, plus the new
  `wkg`/`wac`/runtime dependency note if not already recorded.

## Watch out for

- **`regen-examples.sh` is mandatory** for any example/behaviour change — it
  rebuilds the docs wasm, regenerates `docs/examples.json`, and re-locks
  `tests/examples.rs`. Run it and commit the regenerated artifacts.
- **Don't claim browser-runnability the playground can't deliver.** The component
  runtime is native-only.
- **Submodule discipline** if neovim highlighting changed — the pointer bump is
  part of the step, not a follow-up (`CLAUDE.md`).

## Done when

`cargo test` passes; `./scripts/regen-examples.sh` is green and its artifacts are
committed; docs document macro components with a worked example; CHANGELOG has the
`Added` entry; highlighting and LSP are either updated or explicitly confirmed
unaffected.

## Handoff notes

_(fill in: where the e2e example lives, what docs pages changed, whether a
playground example was possible, the highlighting verdict, the LSP decision, and
the CHANGELOG entry. List anything intentionally deferred for Step 11 to mention
in the PR.)_
