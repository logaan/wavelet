# Agent instructions

This file provides guidance to coding agents working in this repository.

Wavelet is a homoiconic language for the WebAssembly Component Model, written in
Rust (edition 2024). See `README.md` for the project overview and CLI, and the
docs site under `docs/docs/` for the language documentation.

The helper scripts (build, test, release, install) and how the release/docs
pipelines fit together are documented in `scripts/README.md`; the rules below
reference those scripts where they bear on language-change workflow.

## Lists of things

Lists of Things (lot) is a system for organising things such as tasks. Things
are stored as plain text files in a vault. The `lot` command is used when
interacting with the vault. `lot` should be run from the repository root,
so that it can read the `.lot.toml` config file and find the vault.

If you are implementing a task that has come from a Thing you should post
`lot update work` updates from time to time as you make progress. When your work
is complete you should post a `lot update info` update summarising the work
you've done. If you've pushed to a PR make sure to link to it. Updates you
create follow the same commit and push rules that you're using for code changes.

## Compiler pipeline

The compiler is **read → expand → analyze → emit → componentize**.
Each stage owns specific files:

- **read** — `lexer.rs`, `reader.rs`, `form.rs`, `printer.rs` (WAVE tokens →
  form-tree arena; all reader sugar is resolved here).
- **expand** — `expand.rs` (macros run to fixpoint over form trees).
- **compile-time evaluation** — `interp.rs`, `value.rs`, `builtins.rs` (macro bodies only).
- **runtime execution** — `runner.rs` (compiled components).
- **WIT synthesis** — `wit.rs` (derive the component's WIT world from its
  forms).
- **emit** — `emit.rs`, `build.rs` (core wasm → component via `wasm-tools`).

## WebAssembly defines runtime semantics

Core WebAssembly instructions and Component Model contracts define Wavelet's
runtime semantics. Validate emitted components against explicit expected results
and Wasm edge cases. The evaluator in `interp.rs` is compile-time macro machinery,
not a runtime implementation or semantics oracle. Do not use its Rust behavior to
define runtime behavior. Macro forms and operations must not survive into runtime
programs or add macro-only support data to emitted application components.

## docs/examples.json is a single source of truth — keep it regenerated

Every runnable documentation example is authored once in
`docs/scripts/gen-examples.mjs`, which generates `docs/examples.json`. That file
is consumed by **both** the docs `<Playground>` (compiled and executed in a browser
worker) and the `tests/examples.rs` suite (native target). A
language change that alters any documented example's behaviour will break
`cargo test`.

After any change to language behaviour or the example set, regenerate the JSON
and re-lock the tests:

``` console
./scripts/regen-examples.sh
```

This rebuilds the compiler component with `scripts/build-compiler-component.sh`,
runs `node docs/scripts/gen-examples.mjs`, the browser runtime tests, and
`cargo test`. The compiler component under `docs/src/compiler` powers example
generation and browser execution. It is committed and must be regenerated when
the language changes. The docs build generates the Worker bundle with Node only.
Run `npm --prefix docs run test:browser` against a built docs site to check every
example through the compiled browser runtime.

## CHANGELOG.md drives the GitHub release notes — keep it current

`CHANGELOG.md` (Keep a Changelog format) is the source of truth for release
notes. The `Release` workflow (`.github/workflows/release.yml`) runs
`scripts/ci/changelog-section.sh <tag>` on a `v*` tag and uses the matching
version's section as the GitHub release body; if the tag has no section the
release **fails** rather than publishing empty notes. See `scripts/README.md`
for the full release pipeline.

This means:

- **Record every user-visible change under `## [Unreleased]` as you make it** —
  new syntax/semantics, CLI flags, build/install changes, editor-tooling
  changes. Group entries under `Added` / `Changed` / `Fixed` / `Removed`. If a
  change isn't user-visible (internal refactor, test-only), it doesn't need an
  entry.
- **Cutting a release** = rename `## [Unreleased]` to `## [X.Y.Z] - <date>`, add
  a fresh empty `## [Unreleased]`, bump the version in `Cargo.toml` *and*
  `tooling/wavelet-lsp/Cargo.toml` to match, update the compare-link footnotes
  at the bottom of the file, then tag `vX.Y.Z`. Confirm
  `scripts/ci/changelog-section.sh vX.Y.Z` prints the right section before
  tagging.

## A language change is not done until the downstream surfaces are checked

Several artifacts outside `src/` track the language and can silently drift from
it. **Any change to Wavelet's syntax or semantics must consider whether each of
these needs updating too** — the change is not finished until they have each
been checked and updated where affected:

- **The docs site** (`docs/`) — a Docusaurus site documenting the language.
  Prose lives in `docs/docs/`; runnable examples are generated from
  `docs/scripts/gen-examples.mjs` into `docs/examples.json` (see the section
  above). Update the prose and regenerate the examples when behaviour changes.

- **Syntax highlighting** — three grammars highlight Wavelet, all derived from a
  single source of truth, the lexer in `src/lexer.rs`:

  - `docs/src/prism/wavelet.js` — Prism grammar for the docs (static
    ```` ```wavelet ```` code blocks and the live `<Playground>` editor).
  - `tooling/neovim/syntax/wavelet.vim` — Neovim syntax (the `tooling/neovim`
    submodule is the `logaan/wavelet.nvim` plugin repo; it also has
    `ftdetect/wavelet.vim` for `.wlt` detection and `plugin/wavelet.lua` to
    start `wavelet-lsp`). Because it's a submodule, changing this grammar means
    committing locally in `wavelet.nvim` and then bumping the submodule pointer
    here.
  - `tooling/vscode/` — VS Code TextMate grammar + language configuration.

  A change to the lexer's token classes (new literal forms, comment syntax,
  macro heads, the attachment rule, qualified references, ...) must be mirrored
  into all three, or highlighting drifts from the language. See
  `tooling/README.md`.

  **Keeping `wavelet.nvim` current is part of finishing a language change**, not
  a follow-up. The `tooling/neovim` submodule is a *separate* git repo
  (`logaan/wavelet.nvim`), so it does not move with an ordinary commit here.
  When a change touches anything the plugin surfaces — the syntax grammar,
  `.wlt` detection, the LSP wiring, or the token-class list in its README — you
  must:

  1.  ensure the submodule is checked out (`./scripts/init-submodules.sh`; a
      fresh clone leaves it empty);
  2.  make the edit inside `tooling/neovim/`, then commit it locally in the
      `wavelet.nvim` repo (its `origin` is `github.com/logaan/wavelet.nvim`);
  3.  stage the moved submodule pointer here (`git add tooling/neovim`) so this
      repo records the new `wavelet.nvim` commit. Skipping the local commit or the
      pointer bump leaves the published plugin stale even though `cargo test`
      here still passes.

- **The LSP server** — the editor language server (lives under `tooling/`). Its
  diagnostics, completion, and hover surface the language contracts and Wasm
  semantics, so semantic changes must be reflected there too.

## Working conventions

Work in the shared checkout; coordinate file ownership between agents. Commit
logical, completed changes as you work. When the requested work is complete and
required checks pass, push the completed commits. Do not create pull requests
unless explicitly requested. These rules supersede historical workflow
instructions in the vault.

Do not add comments to source code, tests, configuration, or scripts. Keep
explanations in documentation and use descriptive names and small functions.
Preserve executable shebangs and tool-required directives. Document public
contracts in the docs website rather than source comments.
