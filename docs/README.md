# Wavelet documentation site

The [Wavelet](https://github.com/logaan/wavelet) reference documentation, built
with [Docusaurus](https://docusaurus.io/) and deployed to
<https://logaan.github.io/wavelet/>.

## Local development

```console
$ cd docs
$ npm install
$ npm start            # dev server with hot reload
$ npm run build        # production build into ./build
$ npm run serve        # serve the production build locally
```

## Live, runnable examples

Every example in the docs runs in the reader's browser via the Wavelet
interpreter compiled to WebAssembly — the same tree-walking evaluator that backs
`wavelet run`. The pieces:

- **`src/wasm/`** — the wasm package, built from the Rust crate with `wasm-pack`.
  It is committed so the site builds with Node alone (no Rust toolchain needed
  in CI).
- **`src/components/Playground/`** — the `<Playground>` React component. It is
  registered globally (`src/theme/MDXComponents.js`), so any `.mdx` page can use
  `<Playground id="…" />` without an import.
- **`examples.json`** — the **single source of truth** for every runnable
  example: its Wavelet source plus the expected value / output / error.

### No drift between the docs and the language

`examples.json` is consumed by two things at once:

1. the `<Playground id="…">` component, which renders and runs each snippet, and
2. the Rust test `tests/examples.rs`, which runs every snippet through the same
   interpreter (`wavelet::eval_snippet`) and asserts the recorded result.

So a language change that breaks a documented example breaks `cargo test`. The
docs cannot silently drift from the implementation.

### Regenerating after a language change

The example sources are authored once in `scripts/gen-examples.mjs`. After
changing the language (or editing/adding examples):

```console
# 1. rebuild the in-browser interpreter
$ wasm-pack build --target web --out-dir docs/src/wasm --out-name wavelet

# 2. regenerate expected results from the interpreter
$ cd docs && npm run gen:examples

# 3. lock the new behaviour in
$ cargo test
```

## Deployment

`.github/workflows/deploy-docs.yml` builds `docs/` and publishes to GitHub Pages
on every push to `main`. It needs only Node, because the wasm artifact and
`examples.json` are committed.
