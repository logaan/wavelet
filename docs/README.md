# Wavelet documentation site

The [Wavelet documentation](https://logaan.github.io/wavelet/) is built with
Docusaurus and published to GitHub Pages.

## Local development

From the repository root:

```sh
npm --prefix docs ci
npm --prefix docs start
npm --prefix docs run build
npm --prefix docs run serve
```

The build prepares the browser worker bundle from checked-in compiler artifacts
using Node. Rust is needed only when regenerating those artifacts.

## Compiled examples

The playgrounds and browser REPL compile Wavelet source with
`src/compiler/wavelet_compiler.wasm`, then instantiate and execute the emitted
components in a worker. The reference interpreter under `src/wasm/` is used by
the example generator, not for browser execution.

Author playground examples in `scripts/gen-examples.mjs`. It produces
`examples.json`, containing source and expected values or errors, and verifies
compiled results against the reference evaluator. Pages refer to example IDs
through the globally registered `<Playground>` component. Snippets use `code`;
multi-file examples use `sources`, an `entry` filename, and an `invocation`
identifying a zero-argument export that returns a string. Each project file gets
an editor, and Run compiles the files together before calling that export.
Project examples declare their expected result in the generator and are checked
through both native and browser execution.

The tour's built-in set functor also requires compiled execution: the snippet
interpreter does not resolve built-in `Instantiate` declarations. That example
uses `compiledOnly: true` with an explicit expected result, checked by the native
compiler and browser runtime alongside the project example.

After changing the compiler or example sources, run from the repository root:

```sh
./scripts/regen-examples.sh
```

This rebuilds the reference artifact and compiler component, regenerates example
results, and runs validation. Commit the generated artifacts so the documentation
pipeline can build with Node alone.

## Validation

```sh
npm --prefix docs run test:runtime
npm --prefix docs run build
npm --prefix docs run test:browser
```

Browser tests require Playwright Chromium. They execute the playground examples
and the code tour through the actual compiler component and emitted runtime,
and check recovery after errors and cancellation. The tour tests run every
embedded example on `docs/learn-wavelet.mdx` and check that edits to both files
of its component example affect the result.

The [development guide](docs/development.mdx) describes native conformance,
structural, duplication, coverage, and history-analysis checks.

## Deployment

`.github/workflows/deploy-docs.yml` builds and publishes the site on `v*` tags
or a manual workflow dispatch. The committed compiler artifact and examples
keep that job independent of the Rust toolchain.
