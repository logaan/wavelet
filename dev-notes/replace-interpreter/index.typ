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

*Objective.* Make the wasm compiler the execution engine for the *compile-time
and command-line* surfaces, so the tree-walking interpreter no longer *runs user
programs* there. The interpreter is *not* deleted. It is retained as the
differential-testing oracle (`CLAUDE.md`) — the reference the compiler is
validated against — and the differential harness stays for the life of the
project. "Replace the interpreter" therefore means *replace it as an execution
path*, not remove it from the tree.

#note[
  *Out of scope: the docs playground.* The browser playground keeps running on the
  interpreter (via `eval_snippet`, #at("lib.rs:113")). Compiling the compiler to
  `wasm32` and running snippets in-browser is *no longer a goal*, so the surfaces
  below are the only ones that move off the interpreter.
]

The surfaces to move off the interpreter:

#table(
  columns: (auto, 1fr, auto),
  inset: 6pt, stroke: 0.4pt + luma(80%), align: (left, left, left),
  [*Surface*], [*Powered today by*], [*Step*],
  [Macro expansion (compile time)], [`interp.expand_once` for local `DefMacro` (#at("expand.rs:136")); the foreign macro guest `macrolib` is *also* the interpreter], [Step 1],
  [`wavelet repl`], [`Interp` per line (#at("repl.rs")) ], [Step 2],
  [`wavelet run`], [`Interp` over the build set (#at("runner.rs")) ], [(see note)],
)

#note[
  *Scope note.* `wavelet run` (#at("runner.rs")) is the interpreter's third caller.
  It is *outside the two steps* but shares Step 2's machinery (synthesize
  an entry, compile, run via `wasmtime`). Fold it into Step 2 or drop it for
  `build` + `wasmtime`. Either way the interpreter cannot be retired as an
  *execution path* until `run` is addressed too — it survives as the oracle
  regardless. Track it as a Step 2 rider.
]

= 1 · The two steps

/ Step 1 — Macro components (`step-1-macro-components.typ`): compile every macro
  body to wasm so expansion needs no interpreter. Moves the macro guest from
  "strategy A" (embed the interpreter, #at("macrolib.rs")) to "strategy B" (compile
  the `DefMacro` body with `emit.rs`). The crux is teaching the backend to compile
  `Quote`/`Quasi`/`Unquote`/`Splice` and the form-introspection builtins.

/ Step 2 — REPL (`step-2-repl.typ`): replace the per-line `Interp` with an
  accumulate-and-recompile session that compiles the running program and executes
  it under `wasmtime`. The crux is incremental top-level state, result-type
  inference for bare expressions, and error-message fidelity.

= 2 · Shared foundations (gate both steps)

These are prerequisites, not steps. Each step below references them. Do *not*
start a step before its foundations are in place.

#table(
  columns: (auto, 1fr, auto),
  inset: 6pt, stroke: 0.4pt + luma(80%),
  [*ID*], [*Foundation*], [*Needed by*],
  [F1], [*Core vs. standard library — not "backend special-cases".* The
    interpreter exposes far more than the backend:
    `map/filter/fold/min/max/abs/range/zip/split/join/contains/apply/cell-*` and
    the `to-*` conversions are absent from the backend's `BUILTINS` (#at("emit.rs:2837")),
    and `add/lt/...` are int-only (no `f64`/`char`). *The compiler must not grow a
    special case for each.* Split the surface in two: a *small core* the backend
    implements directly (arithmetic over `int`/`f64`/`char`, comparisons, the box
    constructors, the irreducible string/list primitives) and a *standard library
    written in Wavelet itself* (`map`/`filter`/`fold`/`range`/`zip`/… — everything
    expressible over the core) that compiles like any other program. The stdlib is
    *imported by default* into every file, with a *per-file opt-out*. Rule of
    thumb: if it can be written in Wavelet over the core primitives, it lives in
    the stdlib, not in `emit.rs`. The REPL (and `build`/`run`) must compile against
    *core + default stdlib*; the native `tests/examples.rs` corpus is the parity
    bar. Open decision: whether the oracle interpreter also loads the stdlib or
    keeps its native builtins (default: keep native; the differential harness then
    cross-checks compiled-stdlib `map` against interpreter-native `map`).],
    [2],
  [F2], [*Value marshalling + printing.* A native reader that walks a result box in
    linear memory (tags at #at("emit.rs:44-54")) and renders it like `print_value`
    (#at("value.rs:168")), reading `wasmtime` memory for the REPL.],
    [2],
  [F4], [*Differential test harness — the oracle, kept permanently.* Run every
    example through the interpreter *and* the compiled artifact and assert
    equality. This is *not* being deleted: the interpreter is retained precisely
    so this comparison keeps running. Land it *before* backend changes begin, so
    every later change is checked against the interpreter, and keep it for the
    life of the project.],
    [1, 2],
  [F5], [*Diagnostic quality — keep errors as good as the interpreter's.* The
    interpreter returns rich `eval error: …` messages with source context; a raw
    compiled run yields a `wasm trap`/`unreachable` with none. To avoid a UX
    regression the compiled path must map failures back to source: emit
    *distinguishable trap codes per failure class* (div-by-zero, type mismatch,
    out-of-bounds), carry a *form→source-span table* so a trap points at the
    offending form, and keep compile errors as actionable `Result<_, String>`
    strings (they already are). F4 can then assert the error *class*, not only the
    value. Treat this as first-class work, not a post-hoc mitigation.],
    [2],
)

= 3 · Dependency order

#block(inset: (left: 0.5em), [
  *Step 1* (macros: quote/quasi codegen) ──▶ *Step 2* (REPL) \
  `F1 core+stdlib` ──┬──▶ `F2 value reader` ──┬──▶ *Step 2* (REPL; also needs `F5`) \
  `F4 differential tests` ──▶ guards both steps (permanent) \
  `F5 diagnostics` ──▶ guards the REPL's errors (Step 2)
])

Recommended sequence: *F4 → Step 1 → F1 → F2 → F5 → Step 2*. F4 first so every
later change is checked against the interpreter (which stays). Step 1 (compiled
macros) next, since a REPL line may define and use a macro, so the REPL cannot
drop the interpreter from the execution path until macros are compiled. F1/F2/F5
gate the REPL itself: the standard library it evaluates against (F1), the value
reader that prints results (F2), and the diagnostics that keep its errors at
interpreter quality (F5). *F5 is not cleanup* — schedule it alongside the REPL
work.

= 4 · Risk register (the load-bearing tradeoffs)

#risk[*Coverage is the whole game.* The backend is a strict subset of the
  interpreter. Until F1 closes, "replace the interpreter" silently means "support
  fewer programs." Budget the bulk of the effort here, not in the plumbing — but
  spend it building the *standard library* (in Wavelet) over a small core, not on
  per-builtin special cases in `emit.rs`.]

#risk[*Drawing the core/stdlib line.* Some surface is genuinely irreducible and
  must be core (arithmetic, comparisons, the box constructors, base string/list
  ops); the rest should be stdlib. Misclassify downward and `emit.rs` bloats with
  special cases; misclassify upward and the stdlib can't be expressed. Settle the
  boundary explicitly in F1, and decide whether the oracle interpreter consumes
  the same stdlib or keeps native builtins (it stays the oracle either way).]

#risk[*Keep the correctness oracle wired up.* The interpreter is *retained* as the
  reference the backend is validated against (`CLAUDE.md`); it is not being
  removed. The hazard is *changing the backend before the comparison is
  automated*: the `emit_*` tests only assert the module *encodes*
  (`bytes.starts_with(b"\0asm")`), never that it *runs* or *agrees*. Land F4
  first, and keep it running, or the safety net is absent exactly when the backend
  is changing most.]

#risk[*Error fidelity is a planned workstream, not a hope (F5).* The interpreter
  returns rich `eval error: …` strings; a raw compiled run yields a `wasm
  trap`/`unreachable` with no source context. The REPL UX regresses on errors
  unless failures are mapped back to source via F5 (trap codes per failure class +
  a form→span table). Step 2 carries the concrete tasks.]

#risk[*Macro `expand`/quasi at compile time.* Strategy B requires codegen for
  `Quote`/`Quasi` and the `expand` builtin (which is itself recursive expansion).
  These are unimplemented in the backend (#at("emit.rs:1392")) and are the riskiest
  single piece (Step 1).]
