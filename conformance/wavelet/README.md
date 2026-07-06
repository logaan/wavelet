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

- **`src/roundtrip-resources.wlt` — the callee role for `resources`. Builds
  and passes.** Exports the whole `counter` resource (4.5, `DefResource`) plus
  the own/borrow free functions into `roundtrip:suite/resources`. Because
  exporting the *full* `roundtrip` world is all-or-nothing and `values` still
  has goal-5 gaps (f32, flags literals, char arithmetic), this file exports the
  `resources` slice only; the harness composes a rust caller with it and
  terminates the caller's `values` import with the exports-only stub:

  ```console
  ./test-resources-callee.sh          # rust-a and rust-b callers, both PASS
  ```

  The composition is `caller ∘ resources(wavelet) ∘ values(stub)`, driven by
  `run-resources()`. Both rust-a and rust-b callers pass, so the counter is
  decomposed and rebuilt correctly (not hard-coded).

- **`src/roundtrip-values.wlt` — the callee role for `values`. Types fully,
  does not yet compile.** The parallel to the resources callee: exports the
  whole `values` interface, every function signature-annotated so WIT synthesis
  derives the interface's exact shape (which it now does, for all 35 functions),
  with real char next-scalar arithmetic (the old char-identity gap is closed via
  `to-u32`/`to-char`). The remaining blocker is the wasm backend: it does not
  yet implement the higher-order list builtins `map`/`filter`/`fold`, the
  list-building primitives `push`/`concat`, or `flg`/`contains` — used only by
  the list and flags functions (`list-*-rt`, `tuple-nested-rt`, `points-rt`,
  `permissions-rt`). Since a `values` export is all-or-nothing, the file builds
  no farther until those land; then a `test-values-callee.sh` (parallel to
  `test-resources-callee.sh`) will drive it under a rust caller via
  `run-values()`.

- **`src/roundtrip.wlt` — the full symmetric callee role. Does not build; kept
  as the target.** Exporting the whole world at once is all-or-nothing, and a
  few of `values`'s types are still not expressible in Wavelet source (f32,
  flags literals, char arithmetic). Goal 4 closed the rest (variant/enum case
  construction, payload-less results, dep type aliases, cross-package type
  references in signatures) and 4.5 closed resources (now exercised by
  `roundtrip-resources.wlt` above); the goal-5 tasks in the LoT vault track the
  remainder; as they land, this file is the acceptance test.
