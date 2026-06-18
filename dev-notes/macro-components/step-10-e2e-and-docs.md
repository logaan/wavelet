# Step 10 — End-to-end example + docs + highlighting + CHANGELOG

- [x] Done

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

**E2E example / integration test.** Added `worked_e2e_build_through_conventional_location`
to `tests/produced_macros.rs` (extending the Step 9 file rather than duplicating).
It is the full user story driven through `wavelet::build::build_files` — the same
entry point the CLI uses:

- The Step 9 Wavelet-authored macro library (prebuilt `tests/fixtures/produced-macros.wasm`,
  package `demo:macros`) is dropped at the **conventional** `wit/macros/demo-macros.wasm`
  location (no explicit `from:`), proving conventional resolution.
- A consumer `src/app.wvl` imports it `macros: true` and uses its `identity`
  macro both **bare** (`Identity`) and **qualified** (`lib/Identity`) inside two
  exported functions.
- `build_files` reads (registering foreign arities from `manifest()`), routes
  expansion through the component's `expand` at build time, and **emits a real
  wasm component** (asserts the `\0asm` magic). The macro-only import is
  compile-time-only, so the consumer emits as a single self-contained component —
  no `wac`/`wkg` toolchain needed, fully hermetic.
- Used the `identity` macro deliberately: `unless` expands to `If c {} body` and
  the `{}` empty-record (flag) literal isn't emittable by the wasm backend yet,
  so it can't be the body of an exported function. `identity x → x` emits cleanly.

The existing `consumer_path_uses_produced_macro` (read/expand-only, via `from:`)
is complementary and was left in place. All 7 `produced_macros` tests pass; full
`cargo test` is 110 + the integration suites, all green.

**Docs pages changed.**
- `docs/docs/language/macros.mdx` — rewrote the "Macros as components — not yet
  implemented" section into a full **"Macros as components"** section: importing
  with `Import {… macros: true}` (with `from:` and the conventional
  `wit/macros/<ns>-<name>.wasm` resolution), the `wavelet:meta/macros` interface
  (`manifest`/`expand`), writing a macro library **in Wavelet** (a Package +
  DefMacros file that `wavelet build` compiles to a component), and
  qualified/aliased foreign macros + collision behaviour. Static code blocks
  throughout (see playground note below).
- `docs/docs/roadmap.mdx` — added a **Macro components** ✅ row to "What works
  today"; removed the "Macro components — compile-time wasm instantiation" and
  "Qualified TitleCase macros" entries from "Not yet implemented"; fixed the
  "import them once macro components land" aside.
- `docs/docs/supply-chain.mdx` — flipped the sandboxed-macro story from
  "not yet implemented" to implemented, with a note that fine-grained
  per-library *capability wiring* is the next refinement (the isolation boundary
  is real, but grants are still coarse); fixed the stale anchor link.

There is **no separate `dev-notes/docs-todo.md`** in the repo — the spec's
reference to it is stale. The "not-yet-implemented" markers it pointed at lived
in the docs prose above, and those are the surfaces that were updated.

**Playground example: not possible.** The browser `<Playground>` runs the
wasm-compiled interpreter, which has **no component runtime** (`wasmtime` is
native-only, gated out of the `wasm32` build behind the `playground` feature). A
foreign-macro example cannot execute there, so all the new examples are static
` ```wavelet ` code blocks, with an explicit `:::note[Browser playground]`
explaining why. `docs/examples.json` is therefore **unchanged**;
`regen-examples.sh` only rebuilt the committed docs wasm
(`docs/src/wasm/wavelet_bg.wasm`), which is committed.

**Highlighting verdict: NO grammar change needed.** Step 8 made no lexer change —
the qualified spelling `kebab-alias/TitleCaseName` reuses the existing
`Tok::QIdent` (qualified `alias/name`, with the TitleCase flag) that already
tokenised qualified calls; there is no new token class. Verified all three
grammars already cover it: Prism (`docs/src/prism/wavelet.js`) highlights the
`macro` (TitleCase) and `namespace` (alias side of `/`) rules; the VS Code
TextMate grammar (`tooling/vscode/`) has the same `macro` + qualified-reference
rules. The `tooling/neovim` submodule is therefore **untouched** — no
commit/push/pointer-bump in `wavelet.nvim` was required.

**LSP decision: gap noted, not implemented.** `tooling/wavelet-lsp` already
surfaces *runtime* imports (functions from `wit/deps`) in completion/hover. A
`macros: true` import contributes compile-time TitleCase macros, not functions,
so foreign macro names/arities are not offered. Surfacing them would require
instantiating the macro component via `wasmtime` and calling `manifest()` on a
per-keystroke path — a non-trivial feature with its own toolchain/error-handling
surface — so it was **deliberately deferred**. Recorded as a `GAP` doc comment
on `imported_completions` in `tooling/wavelet-lsp/src/analysis.rs`. The LSP still
builds. Foreign macros expand correctly at build time; only editor assistance is
missing.

**CHANGELOG entry.** Added three `Added` bullets under `## [Unreleased]`
(complementing the Step 9 producer bullet that was already there): (1) macro
components / `Import {… macros: true}` / `wavelet:meta/macros` / compile-time
instantiation + routing; (2) qualified & aliased foreign macros + collision
behaviour; (3) the build-time `wasmtime` dependency (native-only) and the new
`playground` cargo feature that gates the browser bindings (so the playground
has no component runtime).

**Deferred for Step 11 / the PR to mention.**
- Foreign-macro completion/hover in the LSP (the noted gap above).
- Fine-grained per-macro-library capability wiring on the supply-chain page (the
  noted refinement — macros run sandboxed but grants are coarse today).
- `unless`-style macros whose expansion contains `{}` (empty record / flag
  literal) can't yet be the body of an *exported* function because the wasm
  backend doesn't emit flag literals; the e2e uses `identity` to sidestep this.
- The spec mentioned a `dev-notes/docs-todo.md` that does not exist — flagged so
  Step 11 doesn't go looking for it.
