# Wavelet against the roundtrip:suite conformance world

The Wavelet side of the conformance suite (see `../README.md` for the world,
transforms, and harness). The suite's WIT is vendored under
`wit/deps/roundtrip-suite/` so `wavelet build` can resolve the imports.

## Two roles, one buildable today

- **`src/runner.wvl` — the caller role. Builds and runs.** Imports
  `roundtrip:suite/values` + `resources`, exports `roundtrip:suite/runner`,
  drives every check Wavelet can express with literal seeds and literal
  expected values:

  ```console
  wavelet build src/runner.wvl -o out
  wac plug ../dist/roundtrip-a.wasm --plug ../dist/stub.wasm -o callee.wasm
  wac plug out/conformance-wavelet.wasm --plug callee.wasm -o composed.wasm
  wasmtime run --invoke 'run()' composed.wasm          # or run-values() / run-resources()
  ```

  Current result against both rust-a and rust-b callees: `run()`,
  `run-values()`, and `run-resources()` all → `ok`. (Three values checks used
  to fail to a backend bug — byte-width payloads corrupted on lift — fixed in
  `emit.rs` and pinned by `tests/backend_byte_width.rs`.) Checks Wavelet
  cannot yet express are absent and listed in the header comment of
  `runner.wvl`.

- **`src/roundtrip.wvl` — the callee role. Does not build; kept as the
  target.** Exporting into a foreign interface is all-or-nothing, and several
  of the world's types/functions are not yet expressible in Wavelet source
  (variant/enum case construction, f32, flags literals, dep type aliases,
  char arithmetic, payload-less results, resources). The stage-3 tasks in the
  LoT vault track these; as they land, this file is the acceptance test.
