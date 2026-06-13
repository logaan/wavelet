#!/usr/bin/env bash
# Full local build: compile, regenerate the documentation examples, run the
# whole test suite, build the language server, and build the docs site.
#
# This is the "everything" entry point — run it before a release (or whenever
# you want the same coverage CI gives you) to catch drift across every surface:
#
#   1. cargo build --release        native wavelet binary
#   2. scripts/regen-examples.sh    recompile the interpreter to wasm, re-run
#                                   every documented example into
#                                   docs/examples.json, then `cargo test`
#                                   (locks the example suite + all unit tests)
#   3. scripts/build-lsp.sh         build the language server (host target)
#   4. scripts/build-docs.sh        build the Docusaurus site into docs/build
#
# Run from anywhere. Requires the Rust toolchain, `wasm-pack`, and `node`/`npm`.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
cd "$here/.."

echo "==> Building native wavelet binary"
cargo build --release

echo "==> Regenerating examples + running tests"
"$here/regen-examples.sh"

echo "==> Building language server (host target)"
"$here/build-lsp.sh"

echo "==> Building docs site"
"$here/build-docs.sh"

echo
echo "Full build complete."
