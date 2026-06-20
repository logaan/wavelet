// step-3-repl.typ — replace the interpreter with the compiler in the REPL.
// Render: `typst compile dev-notes/replace-interpreter/step-3-repl.typ`

#set document(title: "Step 3 — Compiler-backed REPL", author: "Claude (Opus 4.8)")
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
  #text(size: 16pt, weight: "bold")[Step 3 — Replace the interpreter with the compiler in the REPL]
  #v(2pt)
  #text(size: 9pt, fill: luma(35%))[Depends on F1 (parity), F2 (value reader), F4 (diff tests), and Step 2 (compiled macros). See `index.typ`.]
])

= 1 · Goal

`wavelet repl` evaluates each entry by *compiling* the running program and executing
it, with no `Interp`. After this step (plus Steps 1–2 and the `run` rider),
`interp.rs`/`builtins.rs`'s eval role has no callers and can be deleted.

= 2 · Current state

`repl.rs` (#at("repl.rs")) keeps a persistent `MacroTable` + `Env`, reads each line with
`read_with`, evaluates via `Interp::eval`, and prints `print_value` of the result.
State (Defs, DefMacros) accumulates in the `Env` across lines — the property a
whole-program compiler does not natively have.

= 3 · Target: accumulate-and-recompile

#note[
  *Session model.* The REPL accumulates only *definitions* (`Def`, `DefMacro`,
  `DefType`, `Import`). A bare expression line is the *evaluation target*, never
  persisted — which sidesteps re-running prior side effects on recompile. Each entry:
  build a synthetic program = `Package` + accumulated definitions + a synthetic
  `Def __eval Fn {} <expr>` exported as the entry; compile it to a *core module*
  (`emit_core_module`, #at("emit.rs:2845")); instantiate under `wasmtime`; call `__eval`,
  which returns the result's *box pointer* (`i32`); read the box from the instance's
  exported `memory` with the native value reader (F2) and print it. A definition line
  is compiled to validate it, then committed to the accumulation with a `unit` echo.
]

Run the *core module* (not the component) under `wasmtime`: it can instantiate a bare
`Module`, so no componentization is needed for the REPL — mirroring Step 1's
in-browser choice but native. Returning a box pointer avoids needing the export's WIT
result type, which inference cannot always supply for an arbitrary expression.

= 4 · Work breakdown

#task[*Session accumulator.* Replace the `Env` with an ordered list of accepted
  definition forms plus the live `MacroTable`. Classify each line: definition →
  append; expression → wrap as `__eval`. Keep the reader's cross-line arity
  accumulation (the existing `read_with` hook).]

#task[*Program synthesis.* Assemble `Package "repl:session@0.0.0"` + accumulated
  definitions + `Def __eval Fn {} <expr>` + `Export __eval`. Reuse the normal
  `read → expand → wit::collect → emit_core_module` pipeline so REPL and `build`
  share one path.]

#task[*Core-module runner.* Add a small native runner (sibling to `host.rs`'s
  component runner, #at("host.rs")) that instantiates a `wasmtime::Module`, calls
  `__eval` for an `i32`, and exposes the `memory` export for reading. Empty/import-free
  by default; shim any stdout import to the terminal.]

#task[*F2 native value reader.* Port `print_value` (#at("value.rs:168")) over the box
  layout (#at("emit.rs:44-54")) reading `wasmtime` linear memory, so the printed form is
  byte-identical to the interpreter's. Share the spec (and golden tests) with Step 1's
  JS reader.]

#task[*Definition echo + commit-on-success.* A `Def`/`DefMacro` line compiles (to
  validate), prints `unit`/nothing, and commits to the accumulation only if
  compilation succeeded; a failing line is reported and *not* committed, so the
  session stays consistent.]

#task[*Macro lines via Step 2.* `DefMacro` entered interactively accumulates and is
  compiled into the program's macro set on each recompile (Step 2). Verify a macro
  defined on one line expands on a later line.]

#task[*Error mapping.* Surface `emit`/`wit` compile errors verbatim (they are already
  actionable `Result<_, String>`). Map runtime traps to a message; where the backend
  can, emit distinguishable trap codes (div-by-zero, type mismatch) for better text.]

#task[*`wavelet run` rider.* Re-point `runner.rs` (#at("runner.rs")) at `build` +
  the core-module/component runner so the interpreter loses its last non-REPL caller.
  Multi-file import resolution already exists in `build`; reuse it.]

#task[*Delete the interpreter.* Once playground (Step 1), macros (Step 2), REPL, and
  `run` no longer call it, remove `Interp`'s eval surface and the eval-only parts of
  `builtins.rs`/`value.rs`. Keep `form`/`reader`/`printer`. Update `lib.rs` exports.]

= 5 · Risks

#risk[*Per-line latency.* Recompile + instantiate on every entry; cost grows with
  history length. Acceptable for small sessions (ms-scale); for long sessions consider
  caching the compiled definition prefix. Note that Step 2's macro compilation adds a
  `wasmtime` instantiate per line that defines/uses macros.]

#risk[*Error fidelity.* Compile-time errors stay good (`emit` returns readable
  strings), but a runtime *trap* (e.g. `div(1 0)`, a type mismatch) is far less
  informative than the interpreter's `eval error: …`. The REPL UX regresses on
  runtime errors unless trap codes are mapped (Task 7).]

#risk[*Inference for bare expressions.* Returning a box pointer avoids needing the
  result's WIT type — but if any intermediate needs a synthesized signature and
  inference returns `Unknown` (#at("wit.rs")), compilation fails where the interpreter
  would have run. Annotations or the box-pointer convention are the escape hatch.]

#risk[*Statefulness semantics.* Persisting only definitions changes the observable
  model slightly (a re-referenced prior expression is not re-run). Document it; it is
  the price of monolithic recompilation.]

= 6 · Exit criteria

- `repl.rs` and `runner.rs` contain no `Interp` use; `cargo test` green.
- A scripted REPL session (Defs, a DefMacro defined then used, expressions, an error
  line) produces output matching today's interpreter REPL, modulo the documented
  state-model and error-text differences (F4 covers the value equality).
- `interp.rs` eval surface is deleted with no remaining callers.
