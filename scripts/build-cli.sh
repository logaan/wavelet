#!/usr/bin/env bash
# Cross-compile the Wavelet CLI (the root crate's `wavelet` binary) for one
# target and stage it into dist/ under a stable, target-suffixed name.
#
# Usage: scripts/build-cli.sh [TARGET]
#   TARGET  Rust target triple (e.g. x86_64-unknown-linux-gnu). Defaults to the
#           host's default target.
#
# Staged as dist/wavelet-<target>[.exe]. The release packaging step
# (scripts/package-tooling.sh) publishes these standalone and bundles each
# platform's CLI + language server into a wavelet-<target>.tar.gz for the
# Homebrew formula.
#
# Used by the `Release` GitHub workflow alongside scripts/build-lsp.sh (the
# matrix builds natively per platform; the one cross build is the macOS x86_64
# target on the arm runner, which links fine for this pure-Rust crate).
#
# Run from anywhere. Requires the Rust toolchain.
set -euo pipefail
cd "$(dirname "$0")/.."

# Resolve the target: explicit arg, else the host default reported by rustc.
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
