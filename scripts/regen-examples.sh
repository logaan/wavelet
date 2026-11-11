#!/usr/bin/env bash

set -euo pipefail
cd "$(dirname "$0")/.."

wasm-pack build --target web --out-dir docs/src/wasm --out-name wavelet
./scripts/build-compiler-component.sh
node docs/scripts/gen-examples.mjs
npm --prefix docs run test:runtime
cargo test
