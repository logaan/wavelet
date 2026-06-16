#!/usr/bin/env bash
# Build the Docusaurus documentation site (docs/) into docs/build.
#
# The wasm-compiled interpreter that powers the live playground is committed
# under docs/src/wasm, so this needs only Node — no Rust toolchain. Regenerate
# that artifact and docs/examples.json with scripts/regen-examples.sh whenever
# the language changes.
#
# Used by the `Deploy docs` GitHub workflow and by scripts/build.sh.
#
# Run from anywhere. Requires `node`/`npm`.
set -euo pipefail
cd "$(dirname "$0")/../.."

cd docs
npm ci
npm run build
