#!/usr/bin/env bash

set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
cd "$here/.."
root="$(pwd)"

bin_dir="${BIN_DIR:-$HOME/bin}"
mkdir -p "$bin_dir"

echo "==> Building native wavelet binary"
cargo build --release

echo "==> Building language server"
cargo build --release --manifest-path tooling/wavelet-lsp/Cargo.toml

wavelet_bin="$root/target/release/wavelet"
lsp_bin="$root/tooling/wavelet-lsp/target/release/wavelet-lsp"

ln -sf "$wavelet_bin" "$bin_dir/wavelet"
ln -sf "$lsp_bin" "$bin_dir/wavelet-lsp"

echo
echo "Symlinked into $bin_dir:"
echo "  wavelet      -> $wavelet_bin"
echo "  wavelet-lsp  -> $lsp_bin"

case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) echo
     echo "Note: $bin_dir is not on your PATH. Add it to use these commands." ;;
esac
