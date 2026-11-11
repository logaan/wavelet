#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
WAVELET=${WAVELET:-wavelet}
for tool in "$WAVELET" wac wasmtime wasm-tools; do
  command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 2; }
done
../test.sh
"$WAVELET" build src/roundtrip.wlt -o out-matrix
wasm-tools validate out-matrix/conformance-wavelet.wasm
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
run_pair() {
  local caller=$1 callee=$2 label=$3 output
  wac plug "$callee" --plug ../dist/stub.wasm -o "$scratch/callee.wasm"
  wac plug "$caller" --plug "$scratch/callee.wasm" -o "$scratch/composed.wasm"
  output=$(wasmtime run --invoke 'run()' "$scratch/composed.wasm")
  if [[ "$output" != "ok" ]]; then
    echo "FAIL $label: $output" >&2
    return 1
  fi
  echo "PASS $label"
}
for seed in a b; do
  run_pair "../dist/roundtrip-$seed.wasm" out-matrix/conformance-wavelet.wasm "rust-$seed -> wavelet"
  run_pair out-matrix/conformance-wavelet.wasm "../dist/roundtrip-$seed.wasm" "wavelet -> rust-$seed"
done
run_pair out-matrix/conformance-wavelet.wasm out-matrix/conformance-wavelet.wasm 'wavelet -> wavelet'
