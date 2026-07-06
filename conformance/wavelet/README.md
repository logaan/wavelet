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
  derives the interface's exact shape (for all 35 functions), with real char
  next-scalar arithmetic. On the wasm backend, every scalar / char / string /
  record / variant / enum / option / result / **flags** transform and every
  list function (`map`/`filter`/`fold`) now compiles. In particular
  `permissions-rt` (the flags complement `v ^ all()`) is expressed with **zero
  new language surface** — a `Match` over the 16 declaration-order flags
  literals, each result ascribed `The permissions` so the arms unify to the
  imported flags type (verified in the interpreter oracle and confirmed to lower
  on the backend). The `none` side of the option round-trips uses the bare
  `none` symbol (parenthesized `none()` is not lowered; bare `none` is).

  The file is now blocked by a **single remaining gap that needs new language
  surface**, not a conformance rewrite (this corrects the earlier note, and the
  flags decision Thing's premise, which listed `permissions-rt`/flags as the sole
  blocker):
    - **Tuple construction.** `tup(...)` has no oracle semantics at all — the
      interpreter reports `unbound name tup in call position` — and the backend
      has no tuple constructor; bare `(a b)` reads as a call form. There is no
      way to *build* a tuple value in Wavelet source today (tuple *patterns*
      already work). Blocks `tuple-rt`, `tuple-nested-rt`, and the `ok` side of
      `result-tuple-direction-rt` — the only three exports that call `tup`.

  **Resolved** since the last revision: the second blocker, `drop` / no-result
  bodies. `drop` is now a wasm-backend builtin (it returns the unit empty-record
  box, which is discarded at a no-result WIT boundary), so `no-result` and
  `no-params-no-result` compile. Confirmed by building this file with the three
  `tup` exports removed: the core module — including both `drop` bodies — emits
  cleanly, leaving only the world-completeness failure for the removed exports.

  Because a `values` export is all-or-nothing, the tuple gap keeps the file
  un-buildable, so `test-values-callee.sh` (parallel to
  `test-resources-callee.sh`) is deferred until a tuple constructor lands;
  everything else already compiles.

- **`src/roundtrip.wlt` — the full symmetric callee role. Does not build; kept
  as the target.** Exporting the whole world at once is all-or-nothing, and a
  few of `values`'s types are still not expressible in Wavelet source (f32,
  flags literals, char arithmetic). Goal 4 closed the rest (variant/enum case
  construction, payload-less results, dep type aliases, cross-package type
  references in signatures) and 4.5 closed resources (now exercised by
  `roundtrip-resources.wlt` above); the goal-5 tasks in the LoT vault track the
  remainder; as they land, this file is the acceptance test.
