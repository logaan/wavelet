#!/usr/bin/env bash
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
cd "$here"

wavelet build src/*.wlt -o out
echo "built out/app.wasm"
