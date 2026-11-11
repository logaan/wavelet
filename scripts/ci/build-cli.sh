#!/usr/bin/env bash

set -euo pipefail
cd "$(dirname "$0")/../.."

target="${1:-}"
if [ -z "$target" ]; then
  target="$(rustc -vV | sed -n 's/^host: //p')"
fi

rustup target add "$target"
cargo build --release --bin wavelet --target "$target"

mkdir -p dist
bin="target/${target}/release/wavelet"
case "$target" in
  *windows*)
    cp "${bin}.exe" "dist/wavelet-${target}.exe"
    ;;
  *)
    cp "$bin" "dist/wavelet-${target}"
    ;;
esac

echo "Staged wavelet for ${target} into dist/"
