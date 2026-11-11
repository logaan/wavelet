#!/usr/bin/env bash

set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
cd "$here/.."

echo "==> Building native wavelet binary"
cargo build --release

echo "==> Regenerating examples + running tests"
"$here/regen-examples.sh"

echo "==> Building language server (host target)"
"$here/ci/build-lsp.sh"

echo "==> Building docs site"
"$here/ci/build-docs.sh"

echo
echo "Full build complete."
