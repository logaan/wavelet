#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
for tool in cargo rustup python3 node npm wasm-tools wasmtime wac wkg wasm-pack; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Missing $tool; see docs/docs/development.mdx for installation." >&2
    exit 1
  fi
done
python3 scripts/check-quality.py
WAVELET_REQUIRE_HOST_TOOLS=1 ./scripts/coverage.sh --check
cargo clippy --manifest-path tooling/wavelet-lsp/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path tooling/wavelet-lsp/Cargo.toml
npm ci --prefix tooling/vscode
npm --prefix tooling/vscode run check
npm ci --prefix docs
npm --prefix docs run build
