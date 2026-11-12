#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
for tool in cargo cargo-component rustup python3 node npm wasm-tools wasm-opt wasmtime wac wkg; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Missing $tool; see docs/docs/development.mdx for installation." >&2
    exit 1
  fi
done
if ! rustup target list --installed | grep -qx wasm32-wasip1; then
  echo "Missing wasm32-wasip1 target; run rustup target add wasm32-wasip1." >&2
  exit 1
fi
python3 scripts/check-quality.py
python3 scripts/check-duplication.py
python3 scripts/code-maat.py
WAVELET_REQUIRE_HOST_TOOLS=1 ./scripts/coverage.sh --check
cargo build
WAVELET="$PWD/target/debug/wavelet" conformance/wavelet/test-matrix.sh
cargo clippy --manifest-path tooling/wavelet-lsp/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path tooling/wavelet-lsp/Cargo.toml
npm ci --prefix tooling/vscode
npm --prefix tooling/vscode run check
npm ci --prefix docs
npm --prefix docs run build
npm --prefix docs run test:runtime
npm --prefix docs run test:browser
