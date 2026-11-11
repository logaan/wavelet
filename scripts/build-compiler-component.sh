#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --manifest-path components/compiler/Cargo.toml --target wasm32-unknown-unknown --release --locked
mkdir -p docs/src/compiler
wasm-tools component new components/compiler/target/wasm32-unknown-unknown/release/wavelet_compiler_component.wasm -o docs/src/compiler/wavelet_compiler.wasm
wasm-tools validate docs/src/compiler/wavelet_compiler.wasm
