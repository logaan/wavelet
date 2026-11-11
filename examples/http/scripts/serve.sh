#!/usr/bin/env bash
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
cd "$here"

scripts/build.sh
exec wasmtime serve "$@" out/app.wasm
