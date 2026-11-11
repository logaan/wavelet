#!/usr/bin/env bash

set -euo pipefail
cd "$(dirname "$0")"

build() {
  echo "== validating WIT"
  wasm-tools component wit wit/ > /dev/null

  echo "== building artifacts"
  mkdir -p dist
  cargo component build --release -p roundtrip-suite
  cp target/wasm32-wasip1/release/roundtrip_suite.wasm dist/roundtrip-a.wasm
  cargo component build --release -p roundtrip-suite --features seed-b
  cp target/wasm32-wasip1/release/roundtrip_suite.wasm dist/roundtrip-b.wasm
  cargo component build --release -p roundtrip-stub
  cp target/wasm32-wasip1/release/roundtrip_stub.wasm dist/stub.wasm
}

run_pair() {
  local caller=$1 callee=$2 label=$3 tmp out status=0
  tmp=$(mktemp -d)
  if out=$(
    wac plug "$callee" --plug dist/stub.wasm -o "$tmp/callee.wasm" 2>&1 \
      && wac plug "$caller" --plug "$tmp/callee.wasm" -o "$tmp/composed.wasm" 2>&1 \
      && wasmtime run --invoke 'run()' "$tmp/composed.wasm" 2>&1
  ) && [[ "$out" == "ok" ]]; then
    echo "PASS  $label"
  else
    echo "FAIL  $label"
    sed 's/^/      /' <<< "$out"
    status=1
  fi
  rm -rf "$tmp"
  return $status
}

build

failures=0
if [[ $# -eq 2 ]]; then
  run_pair "$1" "$2" "$(basename "$1") -> $(basename "$2")" || failures=1
elif [[ $# -eq 0 ]]; then
  echo "== self-test"
  run_pair dist/roundtrip-a.wasm dist/roundtrip-b.wasm "rust-a -> rust-b" || failures=1
  run_pair dist/roundtrip-b.wasm dist/roundtrip-a.wasm "rust-b -> rust-a" || failures=1
else
  echo "usage: $0 [CALLER.wasm CALLEE.wasm]" >&2
  exit 2
fi

exit $failures
