# Wavelet against the roundtrip:suite conformance world

The Wavelet side of the conformance suite (see `../README.md` for the world,
transforms, and harness). The suite's WIT is vendored under
`wit/deps/roundtrip-suite/` so `wavelet build` can resolve the imports.

## Two roles, one buildable today

- **`src/runner.wlt` — the caller role. Builds and runs.** Imports
  `roundtrip:suite/values` + `resources`, exports `roundtrip:suite/runner`,
  drives every check Wavelet can express with literal seeds and literal
  expected values:

  ```console
  wavelet build src/runner.wlt -o out
  wac plug ../dist/roundtrip-a.wasm --plug ../dist/stub.wasm -o callee.wasm
  wac plug out/conformance-wavelet.wasm --plug callee.wasm -o composed.wasm
  wasmtime run --invoke 'run()' composed.wasm          # or run-values() / run-resources()
  ```

  Current result against both rust-a and rust-b callees: `run()`,
  `run-values()`, and `run-resources()` all → `ok`. (Three values checks used
  to fail to a backend bug — byte-width payloads corrupted on lift — fixed in
  `emit.rs` and pinned by `tests/backend_byte_width.rs`.) Goal 4 expanded the
  check set: dep variant/enum cases are constructible (`shape-rt`,
  `direction-rt`, `option-shape-rt`, the err side of
  `result-tuple-direction-rt`), payload-less results are spellable and
  constructible (`result-rt` both ways, the payload-less sides of
  `result-u32-rt`/`result-string-err-rt`; the runner's own signature is the
  honest `result<_, list<string>>`), and dep type aliases resolve
  (`points-rt`). The remaining absent checks are listed in the header comment
  of `runner.wlt` (f32, flags literals, the `none` side, char arithmetic —
  goal-5 representation gaps).

- **`src/roundtrip.wlt` — the callee role. Does not build; kept as the
  target.** Exporting into a foreign interface is all-or-nothing, and a few
  of the world's types/functions are still not expressible in Wavelet source
  (f32, flags literals, char arithmetic, resources). Goal 4 closed the rest
  (variant/enum case construction, payload-less results, dep type aliases,
  cross-package type references in signatures); the goal-5 tasks in the LoT
  vault track the remainder; as they land, this file is the acceptance test.
