#!/usr/bin/env bash

set -euo pipefail
cd "$(dirname "$0")/../.."

manifest="tooling/wavelet-lsp/Cargo.toml"

target="${1:-}"
if [ -z "$target" ]; then
  target="$(rustc -vV | sed -n 's/^host: //p')"
fi

rustup target add "$target"
cargo build --locked --release --manifest-path "$manifest" --target "$target"

mkdir -p dist
bin="tooling/wavelet-lsp/target/${target}/release/wavelet-lsp"
case "$target" in
  *windows*)
    cp "${bin}.exe" "dist/wavelet-lsp-${target}.exe"
    ;;
  *)
    cp "$bin" "dist/wavelet-lsp-${target}"
    ;;
esac

echo "Staged wavelet-lsp for ${target} into dist/"
