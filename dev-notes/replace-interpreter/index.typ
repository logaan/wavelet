// index.typ — Replace the interpreter with the compiler: plan index.
// Render: `typst compile dev-notes/replace-interpreter/index.typ`

#set document(title: "Replace the interpreter with the compiler — plan index", author: "Claude (Opus 4.8)")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.62em)
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#show link: set text(fill: rgb("#1f6feb"))
#set heading(numbering: "1.1")
#show heading.where(level: 1): set text(size: 13pt)
#show heading.where(level: 2): set text(size: 11pt)

#let at(loc) = raw(loc)
#let risk(body) = block(width: 100%, fill: rgb("#fdecea"), inset: 8pt, radius: 3pt,
  stroke: 0.5pt + rgb("#e6a6a0"), above: 0.8em, below: 0.8em,
  [#text(weight: "bold", fill: rgb("#a3352b"))[Risk] · #body])
#let note(body) = block(width: 100%, fill: luma(95.5%), inset: 8pt, radius: 3pt,
  above: 0.8em, below: 0.8em, body)

#block(fill: luma(96%), width: 100%, inset: 12pt, radius: 5pt, [
  #text(size: 17pt, weight: "bold")[Replace the interpreter with the compiler]
  #v(2pt)
  #text(size: 9pt, fill: luma(35%))[
    Plan index · Wavelet · Date: 2026-06-20 · Author: Claude (Opus 4.8)
  ]
])

*Objective.* Make the wasm compiler the single execution engine and retire the
tree-walking interpreter (`interp.rs`, `builtins.rs`, plus `value.rs`'s eval role)
from the three surfaces it currently powers:

#table(
  columns: (auto, 1fr, auto),
  inset: 6pt, stroke: 0.4pt + luma(80%), align: (left, left, left),
  [*Surface*], [*Powered today by*], [*Step*],
  [Docs playground (browser)], [interpreter → wasm32 via `eval_snippet` (#at("lib.rs:113"))], [Step 1],
  [Macro expansion (compile time)], [`interp.expand_once` for local `DefMacro` (#at("expand.rs:136")); the foreign macro guest `macrolib` is *also* the interpreter], [Step 2],
  [`wavelet repl`], [`Interp` per line (#at("repl.rs")) ], [Step 3],
  [`wavelet run`], [`Interp` over the build set (#at("runner.rs")) ], [(see note)],
)

#note[
  *Scope note.* `wavelet run` (#at("runner.rs")) is the interpreter's fourth caller.
  It is *out of the three requested steps* but shares Step 3's machinery (synthesize
  an entry, compile, run via `wasmtime`). Fold it into Step 3 or drop it for
  `build` + `wasmtime`. Either way the interpreter cannot be deleted until `run`
  is addressed too — track it as a Step 3 rider.
]

= 1 · The three steps

/ Step 1 — Playground (`step-1-playground.typ`): compile the *compiler* to
  `wasm32` and run the user's program in-browser instead of interpreting it. The
  hard part is not the compiler-to-wasm port; it is reaching feature parity with
  the interpreter on the documented example corpus and reading compiled values
  back out of linear memory for display.

/ Step 2 — Macro components (`step-2-macro-components.typ`): compile every macro
  body to wasm so expansion needs no interpreter. Moves the macro guest from
  "strategy A" (embed the interpreter, #at("macrolib.rs")) to "strategy B" (compile
  the `DefMacro` body with `emit.rs`). The crux is teaching the backend to compile
  `Quote`/`Quasi`/`Unquote`/`Splice` and the form-introspection builtins.

/ Step 3 — REPL (`step-3-repl.typ`): replace the per-line `Interp` with an
  accumulate-and-recompile session that compiles the running program and executes
  it under `wasmtime`. The crux is incremental top-level state, result-type
  inference for bare expressions, and error-message fidelity.

= 2 · Shared foundations (gate all three steps)

These are prerequisites, not steps. Each step below references them. Do *not*
start a step before its foundations are in place.

#table(
  columns: (auto, 1fr, auto),
  inset: 6pt, stroke: 0.4pt + luma(80%),
  [*ID*], [*Foundation*], [*Needed by*],
  [F1], [*Backend parity.* The interpreter supports far more than the backend:
    `map/filter/fold/min/max/abs/range/zip/split/join/contains/apply/cell-*` and
    the `to-*` conversions are absent from the backend's `BUILTINS` (#at("emit.rs:2837")),
    and `add/lt/...` are int-only (no `f64`/`char`). The playground runs the
    *docs corpus*, so every example's surface must compile, or the corpus shrinks.],
    [1, 3],
  [F2], [*Value marshalling + printing.* A reader that walks a result box in linear
    memory (tags at #at("emit.rs:44-54")) and renders it like `print_value` (#at("value.rs:168")).
    Needed in JS for Step 1 and natively for Step 3.],
    [1, 3],
  [F3], [*Compile `emit`+`wit` for `wasm32`.* Both are `cfg(not(wasm32))` today
    (#at("lib.rs:16-42")). Gate `emit`/`wit` behind a `playground-compiler` feature that
    builds for `wasm32`; keep `build`/`host`/`tools`/`runner`/`macros*` native-only.],
    [1],
  [F4], [*Differential test harness.* Run every example through the interpreter
    *and* the compiled artifact and assert equality. This is the oracle you are
    deleting — replace it before, not after, so regressions surface in CI.],
    [1, 2, 3],
)

= 3 · Dependency order

#block(inset: (left: 0.5em), [
  `F1 parity` ──┬──▶ `F2 value reader` ──┬──▶ *Step 1* (also needs `F3`) \
  #h(7.2em) └──▶ *Step 2* (quote/quasi codegen) ──▶ *Step 3* (also needs `F2`, `F4`) \
  `F4 differential tests` ──▶ guards every step
])

Recommended sequence: *F4 → F1 → F2 → Step 2 → Step 1 → Step 3*. F4 first so every
later change is checked against the interpreter while it still exists. F1/F2 next
because both consuming steps need them. Step 2 before Step 1/3 because compiled
macros are required for the playground and REPL to drop the interpreter entirely
(a snippet or REPL line may define and use a macro).

= 4 · Risk register (the load-bearing tradeoffs)

#risk[*Parity is the whole game.* The backend is a strict subset of the interpreter.
  Until F1 closes, "replace the interpreter" silently means "support fewer
  programs." Budget the bulk of the effort here, not in the plumbing.]

#risk[*You are removing your correctness oracle.* Today the interpreter is the
  reference the backend is validated against (`CLAUDE.md`). The `emit_*` tests only
  assert the module *encodes* (`bytes.starts_with(b"\0asm")`), never that it
  *runs* or *agrees*. Land F4 first or the safety net is gone exactly when the
  backend is changing most.]

#risk[*Playground footprint.* Shipping `wit-component`/`wit-parser` to `wasm32`
  inflates the playground bundle and may not build cleanly. Mitigation: run the
  *core module* directly in-browser and skip componentization for import-free
  snippets (see Step 1) — `emit_core_module` needs only `wasm-encoder`.]

#risk[*REPL error fidelity.* The interpreter returns rich `eval error: …` strings;
  a compiled run yields a `wasm trap`/`unreachable` with no source context. The
  REPL UX regresses on errors unless mapped back (Step 3).]

#risk[*Macro `expand`/quasi at compile time.* Strategy B requires codegen for
  `Quote`/`Quasi` and the `expand` builtin (which is itself recursive expansion).
  These are unimplemented in the backend (#at("emit.rs:1392")) and are the riskiest
  single piece (Step 2).]
