#!/usr/bin/env bash
# Wavelet as the CALLEE for the roundtrip:suite `resources` interface (4.5).
#
# `src/roundtrip-resources.wlt` exports the whole `counter` resource plus the
# own/borrow free functions, but NOT `values` (goal-5 representation gaps). The
# full `roundtrip` world is therefore not exportable yet, so this drives the
# `resources` slice directly: compose a rust caller with our resources export and
# terminate the caller's `values` import with the exports-only stub, then invoke
# `run-resources()`.
#
#   caller(rust-a/b) ∘ resources(wavelet) ∘ values(stub)
#
# Prereqs: `dist/{roundtrip-a,roundtrip-b,stub}.wasm` (built by `../test.sh`),
# `wac`, `wasmtime`, and the `wavelet` CLI on PATH (or set $WAVELET).
set -euo pipefail
cd "$(dirname "$0")"

WAVELET=${WAVELET:-wavelet}
DIST=../dist

for f in roundtrip-a roundtrip-b stub; do
  [[ -f "$DIST/$f.wasm" ]] || { echo "missing $DIST/$f.wasm — run ../test.sh first" >&2; exit 2; }
done

echo "== building the wavelet resources callee"
"$WAVELET" build src/roundtrip-resources.wlt -o out-res
CALLEE=out-res/conformance-wavelet.wasm

run_pair() {
  local caller=$1 label=$2 tmp out
  tmp=$(mktemp -d)
  # Plug our `resources` export into the caller, then satisfy its remaining
  # `values` import with the stub (whose `resources` export is now unused).
  wac plug "$caller" --plug "$CALLEE" -o "$tmp/step1.wasm"
  wac plug "$tmp/step1.wasm" --plug "$DIST/stub.wasm" -o "$tmp/composed.wasm"
  if out=$(wasmtime run --invoke 'run-resources()' "$tmp/composed.wasm" 2>&1) \
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
run_pair "$DIST/roundtrip-a.wasm" "rust-a caller  <- wavelet resources callee" || failures=1
run_pair "$DIST/roundtrip-b.wasm" "rust-b caller  <- wavelet resources callee" || failures=1
exit $failures
