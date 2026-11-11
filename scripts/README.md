# scripts/

Helper scripts for working on Wavelet. Every script is safe to run from any
directory — each `cd`s to the repo root itself.

The top level holds the scripts an engineer runs day to day. The
[`ci/`](#scriptsci--release--docs-pipeline) subdirectory holds the
build/package steps that the GitHub workflows drive; they're kept as scripts (not
inlined into the YAML) so a release or docs build can be reproduced identically
from a local checkout.

## Engineer scripts (top level)

| Script | What it does | When to run it |
| --- | --- | --- |
| `check.sh` | Runs quality limits, advisory history analysis, coverage-backed native tests, language-server tests, grammar checks, and the documentation build. | Before merging compiler changes; requires the documented host tools. |
| `code-maat.py` | Downloads checksum-pinned Code Maat and reports revision frequency, churn, and co-change coupling from bounded Git history under `target/code-maat/reports`. Requires Java (tested with Java 17), Git, Python, and curl. | Run directly or through `check.sh`; use `--since YYYY-MM-DD` and `--max-commits N` to adjust the history window. Metrics are advisory; tool and incomplete-history errors fail explicitly. |
| `check-duplication.py` | Runs pinned jscpd on implementation Rust and JavaScript/TypeScript, rejects new copy/paste clones, and writes `target/duplication/jscpd-report.json`. Automatically installs the locked detector under `tooling/quality/node_modules`. | Run directly or through `check.sh`; use `--update-baseline` only after reviewing clone removals or deliberate exceptions. |
| `check-quality.py` | Checks formatting, Clippy, Lizard function size/cyclomatic complexity and module-size ratchets. Automatically installs its Python analyzer into `target/quality-venv`. | Run directly for a quick structural gate; use `--update-baseline` only for reviewed baseline changes. |
| `build.sh` | Full local build: native binary → `regen-examples.sh` → language server (`ci/build-lsp.sh`) → docs site (`ci/build-docs.sh`). The "everything" entry point that mirrors the coverage CI gives you. | Before a release, or whenever you want to catch drift across every surface. Needs the Rust toolchain, `wasm-pack`, and `node`/`npm`. |
| `regen-examples.sh` | Rebuilds the interpreter reference artifact and compiler component, regenerates `docs/examples.json`, and runs runtime and Rust tests. | **After any change to language behaviour or the example set** (see [the docs pipeline](#the-docs--examples-pipeline)). Needs `wasm-pack` and `node`. |
| `coverage.sh` | Measures native test coverage with `cargo-llvm-cov`. Prints a summary table; `--html` opens a report, `--lcov` writes `target/coverage/lcov.info`; `--check` enforces the checked-in minimum coverage. | Checking which lines the tests exercise. Bootstraps `cargo-llvm-cov` + `llvm-tools` on first run. |
| `install.sh` | Builds the CLI and language server, then symlinks both into `~/bin` (override with `BIN_DIR`). Symlinks point in-tree, so a later `cargo build --release` updates them in place. | Installing Wavelet locally for development. Needs the Rust toolchain. |
| `init-submodules.sh` | Fetches and checks out git submodules (today just `tooling/neovim`, the `wavelet.nvim` plugin). | Once after a fresh `git clone`, or after a branch switch that moves a submodule pointer. |

## `scripts/ci/` — release & docs pipeline

Run by the GitHub workflows in `.github/workflows/`. You can run them by hand to
reproduce CI locally, but they aren't part of the normal dev loop.

| Script | What it does | Driven by |
| --- | --- | --- |
| `ci/build-cli.sh [TARGET]` | Cross-compiles the `wavelet` CLI for one Rust target and stages it as `dist/wavelet-<target>`. Defaults to the host target. | `release.yml` (matrix); also via `build.sh`'s host build path. |
| `ci/build-lsp.sh [TARGET]` | Cross-compiles the `wavelet-lsp` language server for one target and stages it as `dist/wavelet-lsp-<target>`. Defaults to the host target. | `release.yml` (matrix); `build.sh`. |
| `ci/package-tooling.sh` | Packages the editor tooling and standalone binaries into release artifacts in `dist/` (VS Code zip, per-platform tarballs, standalone CLI/LSP binaries). | `release.yml` (consumes the staged binaries above). |
| `ci/build-docs.sh` | Builds the Docusaurus site (`docs/`) into `docs/build`. Node-only — the playground wasm is committed. | `deploy-docs.yml`; `build.sh`. |
| `ci/changelog-section.sh VERSION` | Prints the `CHANGELOG.md` section for one version to stdout; exits non-zero if there's no section, so a release fails loudly rather than publishing empty notes. | `release.yml`. Also run by hand to sanity-check release notes before tagging `vX.Y.Z`. |

## How the pipelines fit together

### The release pipeline (`release.yml`)

On a `v*` tag: a per-platform matrix builds the CLI and language server
(`ci/build-cli.sh` + `ci/build-lsp.sh`, staged into `dist/`), then a release job
runs `ci/package-tooling.sh` to bundle the editor tooling and repackage the
binaries, and uses `ci/changelog-section.sh <tag>` to extract the release body
from `CHANGELOG.md`. Asset names are stable and unversioned so install
instructions can point at `.../releases/latest/download/<asset>`.

`CHANGELOG.md` (Keep a Changelog format) is the source of truth for release
notes — record user-visible changes under `## [Unreleased]` as you make them, and
confirm `ci/changelog-section.sh vX.Y.Z` prints the right section before tagging.

### The docs + examples pipeline

`docs/examples.json` supplies source to the docs `<Playground>` and reference
results to native and browser tests. Its examples are authored in
`docs/scripts/gen-examples.mjs`. The generator evaluates them with the interpreter
as the semantics oracle. Browser Playgrounds compile the source with the compiler
component and execute the emitted component in a Worker.

`regen-examples.sh` rebuilds both Wasm artifacts, regenerates JSON, and runs
runtime and Rust tests. `build-compiler-component.sh` builds only the compiler
component with `wasm32-unknown-unknown` and validates it with `wasm-tools`.
`docs/src/wasm` and `docs/src/compiler` are committed so docs deployment needs
only Node. The docs build bundles the Worker and its Jco dependencies.

Run `npm --prefix docs run test:browser` after building the docs to check every
example in Chromium and verify cancellation and recovery. Install its browser
with `docs/node_modules/.bin/playwright install chromium`; use
`PLAYWRIGHT_BROWSERS_PATH` to select the browser cache location.
