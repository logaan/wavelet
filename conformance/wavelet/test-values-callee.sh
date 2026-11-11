#!/usr/bin/env bash

set -euo pipefail
cd "$(dirname "$0")"

WAVELET=${WAVELET:-wavelet}
DIST=../dist

for f in roundtrip-a roundtrip-b stub; do
  [[ -f "$DIST/$f.wasm" ]] || { echo "missing $DIST/$f.wasm — run ../test.sh first" >&2; exit 2; }
done

echo "== building the wavelet values callee"
"$WAVELET" build src/roundtrip-values.wlt -o out-values
CALLEE=out-values/conformance-wavelet.wasm

run_pair() {
  local caller=$1 label=$2 tmp out
  tmp=$(mktemp -d)

  wac plug "$caller" --plug "$CALLEE" -o "$tmp/step1.wasm"
  wac plug "$tmp/step1.wasm" --plug "$DIST/stub.wasm" -o "$tmp/composed.wasm"
  if out=$(wasmtime run --invoke 'run-values()' "$tmp/composed.wasm" 2>&1) \
     && [[ "$out" == "ok" ]]; then
    echo "PASS  $label"
  else
    echo "FAIL  $label"
    sed 's/^/      /' <<< "$out"
    rm -rf "$tmp"; return 1
  fi
  rm -rf "$tmp"
}

failures=0
run_pair "$DIST/roundtrip-a.wasm" "rust-a caller  <- wavelet values callee" || failures=1
run_pair "$DIST/roundtrip-b.wasm" "rust-b caller  <- wavelet values callee" || failures=1
exit $failures
