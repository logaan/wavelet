# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Wavelet is a homoiconic language for the WebAssembly Component Model, written in
Rust (edition 2024). See `README.md` for the project overview and CLI, and
`design.md` for the full language design (draft 0.1).

## Compiler pipeline

The compiler is **read → expand → interpret/analyze → emit → componentize**.
Each stage owns specific files:

- **read** — `lexer.rs`, `reader.rs`, `form.rs`, `printer.rs` (WAVE tokens →
  form-tree arena; all reader sugar is resolved here).
- **expand** — `expand.rs` (macros run to fixpoint over form trees).
- **interpret** — `interp.rs`, `value.rs`, `builtins.rs`, `runner.rs`.
- **WIT synthesis** — `wit.rs` (derive the component's WIT world from its forms).
- **emit** — `emit.rs`, `build.rs` (core wasm → component via `wasm-tools`).

## The interpreter is the semantics oracle

`interp.rs` is a tree-walking evaluator that defines the language's reference
semantics. The wasm backend (`emit.rs`) is validated *against* it — the two must
agree on every program. When changing language behaviour, update the interpreter
first; a wasm-backend change that diverges from the interpreter is a bug.

## docs/examples.json is a single source of truth — keep it regenerated

Every runnable documentation example is authored once in
`docs/scripts/gen-examples.mjs`, which generates `docs/examples.json`. That file
is consumed by **both** the docs `<Playground>` (in the browser, via the
wasm-compiled interpreter) and the `tests/examples.rs` suite (native target). A
language change that alters any documented example's behaviour will break
`cargo test`.

After any change to language behaviour or the example set, regenerate the JSON
and re-lock the tests:

```console
./scripts/regen-examples.sh
```

This runs `wasm-pack build --target web --out-dir docs/src/wasm --out-name wavelet`,
then `node docs/scripts/gen-examples.mjs`, then `cargo test`. The wasm artifact
under `docs/src/wasm` is committed (CI builds the docs with Node only, no Rust
toolchain), so it must be regenerated locally when the language changes.

## A language change is not done until the downstream surfaces are checked

Several artifacts outside `src/` track the language and can silently drift from
it. **Any change to Wavelet's syntax or semantics must consider whether each of
these needs updating too** — the change is not finished until they have each been
checked and updated where affected:

- **The docs site** (`docs/`) — a Docusaurus site documenting the language. Prose
  lives in `docs/docs/`; runnable examples are generated from
  `docs/scripts/gen-examples.mjs` into `docs/examples.json` (see the section
  above). Update the prose and regenerate the examples when behaviour changes.

- **Syntax highlighting** — three grammars highlight Wavelet, all derived from a
  single source of truth, the lexer in `src/lexer.rs`:
  - `docs/src/prism/wavelet.js` — Prism grammar for the docs (static
      ```` ```wavelet ```` code blocks and the live `<Playground>` editor).
  - `tooling/vim/` — Vim/Neovim syntax + `.wvl` filetype detection.
  - `tooling/vscode/` — VS Code TextMate grammar + language configuration.

  A change to the lexer's token classes (new literal forms, comment syntax, macro
  heads, the attachment rule, qualified references, ...) must be mirrored into all
  three, or highlighting drifts from the language. See `tooling/README.md`.

- **The LSP server** — the editor language server (lives under
  `tooling/`). Its diagnostics, completion, and hover surface the
  interpreter's reference semantics, so semantic changes must be reflected there
  too.
