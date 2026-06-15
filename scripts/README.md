# scripts/

Helper scripts for building, testing, releasing, and installing Wavelet. Every
script is safe to run from any directory — each `cd`s to the repo root itself.
Most are also invoked by the GitHub workflows in `.github/workflows/`.

| Script | What it does | When to run it |
| --- | --- | --- |
| `build.sh` | Full local build: native binary → `regen-examples.sh` → language server → docs site. The "everything" entry point that mirrors CI coverage. | Before a release, or whenever you want to catch drift across every surface. Needs the Rust toolchain, `wasm-pack`, and `node`/`npm`. |
| `regen-examples.sh` | Recompiles the interpreter to wasm, re-runs every documented example into `docs/examples.json`, then `cargo test` to lock the result. | **After any change to language behaviour or the example set** (see `CLAUDE.md`). Needs `wasm-pack` and `node`. |
| `build-cli.sh [TARGET]` | Cross-compiles the `wavelet` CLI for one Rust target and stages it as `dist/wavelet-<target>`. | Release packaging; rarely by hand. Defaults to the host target. Needs the Rust toolchain. |
| `build-lsp.sh [TARGET]` | Cross-compiles the `wavelet-lsp` language server for one target and stages it as `dist/wavelet-lsp-<target>`. | Release packaging; rarely by hand. Defaults to the host target. Needs the Rust toolchain. |
| `build-docs.sh` | Builds the Docusaurus site (`docs/`) into `docs/build`. Node-only — the playground wasm is committed. | Building/previewing the docs site. Needs `node`/`npm`. Run `regen-examples.sh` first if the language changed. |
| `package-tooling.sh` | Packages the editor tooling and standalone binaries into release artifacts in `dist/` (VS Code zip, per-platform tarballs, standalone CLI/LSP binaries). | Release packaging (consumes the staged binaries from `build-cli.sh`/`build-lsp.sh`). Needs `node`/`npm`, `zip`, `tar`. |
| `changelog-section.sh VERSION` | Prints the `CHANGELOG.md` section for one version to stdout; exits non-zero if there's no section. | Feeds the GitHub release body. Run it to sanity-check release notes before tagging `vX.Y.Z`. |
| `coverage.sh [--html\|--lcov]` | Measures native test coverage with `cargo-llvm-cov`. Prints a summary table; `--html` opens a report, `--lcov` writes `target/coverage/lcov.info`. | Checking which lines the tests exercise. Bootstraps `cargo-llvm-cov` + `llvm-tools` on first run. |
| `install.sh` | Builds the CLI and language server, then symlinks both into `~/bin` (override with `BIN_DIR`). Symlinks point in-tree, so a later `cargo build --release` updates them in place. | Installing Wavelet locally for development. Needs the Rust toolchain. |
| `init-submodules.sh` | Fetches and checks out git submodules (today just `tooling/neovim`, the `wavelet.nvim` plugin). | Once after a fresh `git clone`, or after a branch switch that moves a submodule pointer. |

See `CLAUDE.md` for how `regen-examples.sh`, `build-docs.sh`, and
`changelog-section.sh` fit into the release and docs workflows.
