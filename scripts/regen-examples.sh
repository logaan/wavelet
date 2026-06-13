#!/usr/bin/env bash
# Regenerate the documentation-example source of truth after a language change.
#
# `docs/examples.json` is consumed by BOTH the docs <Playground> (in the
# browser) and `tests/examples.rs` (on the native target), so it must be
# rebuilt from the current interpreter whenever language behaviour changes.
#
# Steps:
#   1. Recompile the interpreter to wasm (powers the playground + gen script).
#   2. Re-run every documented example through that wasm interpreter, recording
#      its value / output / error into docs/examples.json.
#   3. Run the test suite to lock the new behaviour in.
#
# Run from anywhere. Requires `wasm-pack` and `node`.
set -euo pipefail
cd "$(dirname "$0")/.."

wasm-pack build --target web --out-dir docs/src/wasm --out-name wavelet
node docs/scripts/gen-examples.mjs
cargo test
