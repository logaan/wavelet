// step-1-playground.typ — compile the compiler to wasm, run it in the playground.
// Render: `typst compile dev-notes/replace-interpreter/step-1-playground.typ`

#set document(title: "Step 1 — Compiler in the playground", author: "Claude (Opus 4.8)")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.62em)
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#set heading(numbering: "1.1")
#show heading.where(level: 1): set text(size: 13pt)
#show heading.where(level: 2): set text(size: 11pt)

#let at(loc) = raw(loc)
#let cb = box(width: 0.9em, height: 0.9em, stroke: 0.7pt + luma(45%), radius: 1.5pt, baseline: 0.15em)
#let task(body) = block(above: 0.45em, below: 0.35em, [#cb #h(0.5em) #body])
#let risk(body) = block(width: 100%, fill: rgb("#fdecea"), inset: 8pt, radius: 3pt,
  stroke: 0.5pt + rgb("#e6a6a0"), above: 0.8em, below: 0.8em,
  [#text(weight: "bold", fill: rgb("#a3352b"))[Risk] · #body])
#let note(body) = block(width: 100%, fill: luma(95.5%), inset: 8pt, radius: 3pt,
  above: 0.8em, below: 0.8em, body)

#block(fill: luma(96%), width: 100%, inset: 12pt, radius: 5pt, [
  #text(size: 16pt, weight: "bold")[Step 1 — Compile the compiler to wasm, run it in the playground]
  #v(2pt)
  #text(size: 9pt, fill: luma(35%))[Depends on foundations F1 (parity), F2 (value reader), F3 (emit\@wasm32). See `index.typ`.]
])

= 1 · Goal

The browser playground should *compile* the user's snippet to wasm and *run* it,
producing the same `(value, output)` pair the interpreter does today — then the
interpreter can be dropped from the `wasm32` build.

= 2 · Current state

- The playground links *only* the interpreter. `eval_snippet` (#at("lib.rs:113")) does
  `read_file` → `Interp::eval` per root → returns the final value's `print_value`
  plus captured output. The wasm bindings live in `wasm.rs`, gated
  `cfg(all(target_arch = "wasm32", feature = "playground"))` (#at("lib.rs:50")).
- The compiler back end is *excluded* from the wasm build on purpose: `emit`,
  `wit`, `build`, `host`, `tools`, … are `cfg(not(target_arch = "wasm32"))`
  (#at("lib.rs:16-42")) because they pull native-only crates.
- Output today is in-process: `print`/`println` write to `OUTPUT_SINK`
  (#at("lib.rs:60"), #at("lib.rs:78")). A compiled program instead writes to a WASI
  interface (the templates import `wasi:cli/stdout`).

= 3 · Target architecture

#note[
  *Key decision — run the core module, not the full component.* The playground only
  needs to execute a snippet and show its result. `emit_component` (#at("emit.rs:697"))
  wraps a *core module* (`emit_core_module`, #at("emit.rs:2845")) with `wit-component`.
  Componentizing and then running a component in-browser needs either `jco`
  transpilation or an in-browser component runtime — heavy. Instead, instantiate the
  *core module* directly with `WebAssembly.instantiate`, shim its imports in JS, and
  read the result box out of its exported `memory`. For import-free snippets this
  needs only `wasm-encoder` (no `wit-component`/`wit-parser`), keeping the bundle
  small. Keep the full-component path as a later, higher-fidelity option (§6).
]

Driver shape: wrap the snippet so its final value is reachable. Synthesize
`Package "playground:snippet@0.0.0"`, lift the snippet's top-level forms, and add a
synthetic export `#[playground] eval() -> i32` whose body evaluates the last form and
returns its *box pointer*. JS then walks the box (F2) to reproduce `EvalOutcome.value`;
`print` output is captured by shimming the snippet's stdout import into a JS buffer.

= 4 · Work breakdown

#task[*F3 — make the compiler compile for `wasm32`.* Introduce a
  `playground-compiler` feature. Under it, compile `form`/`reader`/`expand`/`wit`/
  `emit` for `wasm32`; keep `build`/`host`/`tools`/`runner`/`scaffold`/`macros`/
  `macrobuild`/`macrodep` native-only. Replace the blanket `cfg(not(wasm32))` on
  `emit`/`wit` (#at("lib.rs:16-42")) with the feature gate.]

#task[*Verify dependency crates build for `wasm32`.* `wasm-encoder` is pure Rust and
  near-certain. Confirm `wit-parser`/`wit-component` build for
  `wasm32-unknown-unknown` *before* relying on them; if not, the core-module path
  (§3) avoids them entirely for import-free snippets. Gate componentization behind a
  sub-feature so the default playground build excludes it.]

#task[*Add a core-module entry point.* Expose `compile_snippet_core(src) ->
  Result<Vec<u8>, String>` that runs `read → expand(local macros) → wit::collect →
  emit_core_module` and returns raw module bytes (no `wit-component`). Synthesize the
  `eval` export wrapper that returns the final form's box pointer.]

#task[*F2 — JS value reader.* Port `print_value` (#at("value.rs:168")) to JS over the box
  layout (#at("emit.rs:44-54")): `TAG_BOOL/INT/STR/LIST/REC/VAR/TUP/DEC` and closures.
  Read from the instance's `memory` buffer; reproduce canonical WAVE text exactly so
  the corpus's expected strings match. This is the single most error-prone surface —
  cover it with a golden test against `print_value` output.]

#task[*Output capture via a stdout shim.* A snippet that prints imports a stdout-like
  interface. Provide a JS import object implementing it that appends UTF-8 to a
  buffer; surface that buffer as `EvalOutcome.output`. Decide whether playground
  snippets keep using a `print` builtin (then give the backend a `print` that lowers
  to the shimmed import) or the explicit `wasi:cli/stdout` import.]

#task[*Swap the bindings.* Rewrite `wasm.rs` to call the compile-and-run path and
  assemble `EvalOutcome { ok, value, output, error }` with the same shape, so the
  Docusaurus `<Playground>` is unchanged. Map compile errors and traps to
  `ok: false` + `error`.]

#task[*Trap → message mapping.* A backend `Unreachable` (e.g. a type mismatch) is an
  opaque trap. Catch it and emit an actionable `error` string; where feasible, have
  the backend emit a distinguishable trap/return-code per failure class.]

#task[*F1 parity gate (see index).* Audit every `docs/examples.json` entry for
  builtins/features the backend lacks (`map`/`fold`/float math/`char`/…). Each gap is
  either backend work or an example rewrite. The playground cannot ship until the
  corpus compiles. *This is the bulk of Step 1.*]

#task[*Wire F4 differential tests.* Extend `tests/examples.rs` to run each example
  through `eval_snippet` *and* the compiled core module, asserting equal
  `(value, output)`. Keep it while the interpreter still exists.]

= 5 · Risks

#risk[*Bundle size / buildability of `wit-component` on `wasm32`.* Mitigated by the
  core-module path; confirm early (Task 2) so the architecture choice is settled
  before downstream work.]

#risk[*Parity gap.* The corpus exercises interpreter-only builtins and float/char.
  Until F1 lands, swapping the playground regresses it. Do not swap before the
  corpus is green under the compiled path.]

#risk[*JS marshalling drift.* The JS value reader (F2) duplicates `print_value`; they
  can diverge. The golden test (Task 4) and F4 differential tests are the guard.]

= 6 · Alternative (higher fidelity, later)

If WASI-using examples must run in-browser, transpile the *component* with `jco` and
instantiate the generated ES module, supplying browser WASI shims. More faithful to
production, heavier bundle and toolchain. Defer unless the corpus needs it.

= 7 · Exit criteria

- `docs/examples.json` runs green through the compiled core-module path with byte-
  identical `(value, output)` to `eval_snippet` (F4).
- The default playground `wasm32` build no longer links `interp`/`builtins` for
  evaluation (only the compiler and the value reader).
- Playground bundle size and cold-compile latency are within budget (record both).
