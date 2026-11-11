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
through the globally registered `<Playground>` component.

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
and check recovery after errors and cancellation. The tour tests extract source
from `docs/learn-wavelet.mdx`, so edits to its executable fences are checked too.

The [development guide](docs/development.mdx) describes native conformance,
structural, duplication, coverage, and history-analysis checks.

## Deployment

`.github/workflows/deploy-docs.yml` builds and publishes the site on pushes to
`main`. The committed compiler artifact and examples keep that job independent
of the Rust toolchain.
