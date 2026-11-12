#!/usr/bin/env bash

set -euo pipefail
cd "$(dirname "$0")/.."

./scripts/build-compiler-component.sh
node docs/scripts/gen-examples.mjs
npm --prefix docs run test:runtime
cargo test
