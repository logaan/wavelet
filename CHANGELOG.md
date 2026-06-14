# Changelog

All notable changes to Wavelet are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This file is the source of truth for GitHub release notes: on a `v*` tag the
release workflow extracts the matching version's section below and uses it as
the release body (see `.github/workflows/release.yml` and
`scripts/changelog-section.sh`). Keep the `[Unreleased]` section up to date as
you work, and rename it to the new version when you cut a release.

## [Unreleased]

## [0.4.0] - 2026-06-14

### Added
- `wavelet new --type=cli` scaffolds a `wasi:cli/command` program: `src/main.wvl`
  exports `run` and greets its first argument, delegating to the pure `greet`
  function in `src/greeting.wvl`, with `scripts/build.sh` + `scripts/run.sh`.

### Changed
- `wavelet new` now defaults to `--type=cli` (was `--type=http`).

## [0.3.0] - 2026-06-14

### Added
- `wavelet new <name>` scaffolds a new project: a `.gitignore`, a `src/` with a
  `wasi:http/incoming-handler` front end and the domain model it imports, and
  `scripts/build.sh` + `scripts/serve.sh`. `--type=http` selects the template
  and is the default.

## [0.2.5] - 2026-06-14

### Added
- Release builds now publish the `wavelet` CLI as well (previously only
  `wavelet-lsp`), for macOS (arm64 and x86_64) and Linux (x86_64 and arm64),
  plus a per-platform `wavelet-<target>.tar.gz` bundle consumed by the Homebrew
  formula.

### Changed
- The Homebrew formula now installs prebuilt binaries instead of building from
  source, so `brew install logaan/tap/wavelet` no longer fetches a Rust
  toolchain (`--HEAD` still builds from source).
- The release workflow no longer fails the whole release when one target's
  build leg fails; it publishes whatever binaries succeeded.

## [0.2.4] - 2026-06-14

### Added
- Homebrew install path: `brew install logaan/tap/wavelet`, documented in the
  README.

## [0.2.3] - 2026-06-14

### Added
- `--version` flag on both `wavelet` and `wavelet-lsp`.
- `scripts/coverage.sh` for `cargo-llvm-cov` test-coverage reports.
- `scripts/install.sh` to symlink `wavelet` and `wavelet-lsp` into `~/bin` for
  local development.

### Changed
- Synced the `wavelet` and `wavelet-lsp` crate versions to 0.2.3.
- Bumped the GitHub Actions runners to the Node 24 action versions.

## [0.2.2] - 2026-06-14

### Added
- `scripts/init-submodules.sh` to check out the `tooling/neovim` submodule on a
  fresh clone.
- MIT license.

### Changed
- Moved the Neovim plugin out into the standalone `logaan/wavelet.nvim`
  repository, tracked here as the `tooling/neovim` submodule.
- The docs site now also deploys on `v*` tags.

## [0.2.1] - 2026-06-13

### Changed
- Build the docs site only for releases rather than on every push.
- Dropped the Apple x86_64 target from the release matrix.

## [0.2.0] - 2026-06-13

### Added
- `wavelet-lsp` language server providing diagnostics, completion, and hover
  backed by the interpreter's reference semantics.
- The language server is bundled into both the VS Code and Neovim editor
  packages, and published as per-platform standalone binaries on each release.

## [0.1.0] - 2026-06-13

Initial release.

### Added
- The full `read → expand → interpret/analyze → emit → componentize` compiler
  pipeline: WAVE lexer/reader/desugarer with a canonical printer, ahead-of-time
  macro expansion to fixpoint, a tree-walking interpreter (the language's
  reference semantics) with macros and multi-file runs, WIT world synthesis
  (`wavelet wit`), and wasm emission + componentization + composition.
- Interpreter-backed REPL.
- Canonical-ABI emission across component boundaries for records, variants,
  tuples, `option`, `result`, `list<T>`, and string fields in aggregates.
- First-class closures via a funcref table, plus a `to-string` builtin.
- `expand` builtin (one macro-expansion step on a form value).
- `///` doc comments that attach to the following form.
- Grouped exports landing in a named interface.
- Editor syntax-highlighting tooling for Vim and VS Code, published as release
  artifacts.
- Docusaurus documentation site with a live, wasm-compiled `<Playground>`.

[Unreleased]: https://github.com/logaan/wavelet/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/logaan/wavelet/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/logaan/wavelet/compare/v0.2.5...v0.3.0
[0.2.5]: https://github.com/logaan/wavelet/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/logaan/wavelet/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/logaan/wavelet/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/logaan/wavelet/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/logaan/wavelet/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/logaan/wavelet/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/logaan/wavelet/releases/tag/v0.1.0
