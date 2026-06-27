// dev-notes/functor/plan/05-parity-tests.typ — Step 05: full parity test suite.

#set document(title: "Step 05 — full interpreter-parity tests")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.62em)
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#set heading(numbering: none)
#show heading.where(level: 1): set text(size: 13pt)

= Step 05 — prove full interpreter parity

*First read `plan/00-agent-rules.typ`*, then `summaries/04-routing.typ`.
Critical rules: branch `worktree-functor-build` via
`EnterWorktree path=.claude/worktrees/functor-build`; commit as you go with the
two trailers; no PR; interpreter is the oracle.

== Goal

Prove the locked acceptance bar — FULL interpreter parity — with a real test
suite that builds through the emitter and executes in wasmtime, and fix any
divergence found (the interpreter always wins).

== Add `tests/backend_functor.rs`

Mirror `tests/backend_numeric.rs`: build via `wavelet::build::build_files`, read
the bytes, instantiate via `wavelet::host::HostComponent`, call exports.

- *Use a per-call unique temp dir* keyed on `(pid, AtomicU32 seq)`, NOT pid
  alone. `backend_numeric.rs` was flaky for exactly this reason — see the
  comment in its `numeric_component()` (a pid-only dir let two concurrent builds
  in the same process `remove_dir_all` each other mid-flight). Copy that pattern.
- Where you need the expected value, compute it from the interpreter — either
  run the same source through the run-path in-process, or hard-code the value
  the interpreter produces and cite it. Do not invent expected values.

== Cases (each asserts backend == interpreter)

+ *Worked example.* `set` at `point`, `nearest-set` over a `list<point>` with
  duplicates → returned handle's `size` equals the deduped count; `contains` of
  a present vs absent point.
+ *Multiple instantiations in ONE world.* `point-set` AND `string-set` together;
  exercise both resources in the same component (this is the real test of "any
  element type, multiple instantiations").
+ *A primitive element type.* `set` at `s32` (or `u32`): add/contains/size and
  dedup.
+ *Edges.* Adding the same element twice keeps `size` 1; empty set `size` 0;
  `contains` true and false.

== Clean up the spikes

Remove or convert any `#[ignore]`d scratch tests left by steps 01–04 into real
assertions, or delete the throwaways. The tree should end with one coherent
`backend_functor.rs` (plus the pre-existing `functor_runtime.rs` run-path
suite from PR #22, which stays).

== Run the full suite

`cargo test` must be green, including the new suite. Run it back-to-back at
least twice to guard against the temp-dir flakiness class. If any case diverges
from the interpreter, fix the BACKEND to match the interpreter (never the
reverse) and note the fix.

== Definition of done

- `tests/backend_functor.rs` exists and passes; all four case groups covered.
- Full `cargo test` green across repeated runs.
- No stray `#[ignore]`d spike tests remain.

== Write `summaries/05-parity.typ` for step 06

- The cases covered and their expected values' provenance (interpreter).
- Any backend divergence found and how it was fixed.
- Confirmation the full suite is green (and that you ran it repeatedly).
- Anything step 06 should mention in docs/CHANGELOG (e.g. "multiple
  instantiations supported", element types proven).
