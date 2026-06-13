# Wavelet documentation site — todo

Tracking doc for building the Docusaurus reference site (lives in `docs/`,
deploys to https://logaan.github.io/wavelet/). Separate from `todo.md`, which
tracks the language implementation.

Keep this updated: mark `[x]` when done, add notes inline.

## Phase 0 — scaffold & config
- [x] Scaffold Docusaurus classic site into `docs/` (JS, blog removed)
- [x] Configure for GitHub Pages project site (`url`, `baseUrl: /wavelet/`,
      org `logaan`, project `wavelet`, docs-at-root via `routeBasePath: '/'`)
- [x] Custom CSS / branding pass (colors, logo placeholder)
- [x] Manual sidebar ordering (`sidebars.js`)
- [x] GitHub Actions workflow to build + deploy to Pages

## Phase 1 — in-browser interpreter (interactive examples)
Compile the Rust interpreter to WASM so doc examples are editable + runnable.
- [x] Add `wasm-bindgen` glue: `eval(src) -> {ok, value, output, error}` (src/wasm.rs)
- [x] Output capture: route `print`/`println` through a thread-local sink so
      the playground can show stdout (native CLI unchanged) — lib.rs emit_output
- [x] `[lib] crate-type = ["cdylib", "rlib"]`; wasm-only target deps; native
      modules (emit/build/wit/runner/repl) gated off wasm
- [x] Install `wasm-pack`; build to `docs/src/wasm/` (committed; smoke-tested in
      node: values, captured output, and errors all behave)
- [x] `<Playground>` React component: editable code, Run button, output pane,
      lazy-loads the wasm; registered globally via theme/MDXComponents
- [x] Wire `<Playground>` into MDX; an editable+runnable example per function

## Phase 1b — single source of truth for examples (no doc/test drift)
- [x] `docs/examples.json` — every runnable example (78), code + expected
- [x] `docs/scripts/gen-examples.mjs` — authoring source; runs each through the
      wasm interpreter to record expected value/output/error
- [x] `tests/examples.rs` — runs every example through native `eval_snippet`
      and asserts the recorded result (fails on any drift)
- [x] `eval_snippet` shared by wasm bindings + tests (one evaluation path)
- [x] `<Playground id="…">` loads code from examples.json; all docs migrated
      off inline code (zero `code={` left)

## Phase 2 — content (every form & function, marked for what's implemented)
- [x] Introduction (what Wavelet is, the three commitments)
- [x] Philosophy (no FFI, design ledger, costs acknowledged)
- [x] Getting started (build the compiler, hello world, run vs build)
- [x] Syntax (WAVE tokens, attachment rule, desugaring table, macro sugar)
- [x] Values (the WIT type/value inventory)
- [x] Evaluation (the four rules, Lisp-1)
- [x] Special forms — all 17, with runnable examples
- [x] Pattern matching (`Match`)
- [x] Macros (`DefMacro`, `Quasi`/`Unquote`/`Splice`, `gensym`, `expand`)
- [x] Tail calls
- [x] Standard library — every builtin, grouped, with runnable examples
- [x] Components & composition (files, packages, imports/exports)
- [x] CLI reference (`read expand repl wit run build compose`)
- [x] Supply-chain security (the threat, how Wavelet's model helps)
- [x] Roadmap / not-yet-implemented (clearly flagged; e.g. macro components,
      resources beyond cells, `safely`, `--fuse`, async, hygiene, registry)

## Phase 3 — verify
- [x] `npm run build` succeeds with no broken links
- [x] Playground runs a sample expression in a built preview
- [x] Cross-check every documented builtin against `src/builtins.rs` NAMES
- [x] Cross-check every special form against `src/interp.rs`
