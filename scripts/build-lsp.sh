#!/usr/bin/env bash
# Cross-compile the Wavelet language server (tooling/wavelet-lsp) for one target
# and stage the binary into dist/ under a stable, target-suffixed name.
#
# Usage: scripts/build-lsp.sh [TARGET]
#   TARGET  Rust target triple (e.g. x86_64-unknown-linux-gnu). Defaults to the
#           host's default target.
#
# Staged as dist/wavelet-lsp-<target>[.exe]. The release packaging step
# (scripts/package-tooling.sh) bundles these into the editor zips, and they are
# also published as standalone release assets.
#
# Used by the `Release` GitHub workflow (matrix builds the binary natively on
# each platform's own runner, so no cross toolchain is needed).
#
# Run from anywhere. Requires the Rust toolchain.
set -euo pipefail
cd "$(dirname "$0")/.."

manifest="tooling/wavelet-lsp/Cargo.toml"

# Resolve the target: explicit arg, else the host default reported by rustc.
target="${1:-}"
if [ -z "$target" ]; then
  target="$(rustc -vV | sed -n 's/^host: //p')"
fi

rustup target add "$target"
cargo build --release --manifest-path "$manifest" --target "$target"

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
