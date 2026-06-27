// gaps.typ — the single, consolidated list of Wavelet's outstanding work.
// Render: `typst compile dev-notes/gaps.typ`.
//
// This file replaces the scattered todo/plan/review notes that used to live in
// `dev-notes/`. It is the one place to look for "what's missing". When you start
// a piece of work, lift its detail out into its own plan note; when you finish,
// tick it here (or delete it).

#set document(title: "Wavelet — outstanding work (gaps)", author: "Wavelet")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.6em)
#set text(size: 10pt, font: "New Computer Modern")
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#show link: set text(fill: rgb("#1f6feb"))
#set heading(numbering: "1.1  ")
#show heading.where(level: 1): set text(size: 13pt)
#show heading.where(level: 2): set text(size: 11pt)
#set list(indent: 0.5em, spacing: 0.5em)

#let at(loc) = raw(loc) // a `file:line` or source reference
#let src(tag) = box(
  fill: luma(238), inset: (x: 4pt, y: 1pt), radius: 2pt, baseline: 0.1em,
  text(size: 7.5pt, fill: luma(35%), tag),
)
#let box-(body) = [☐ #h(0.3em) #body]

#block(fill: luma(96%), width: 100%, inset: 12pt, radius: 5pt, [
  #text(size: 17pt, weight: "bold")[Wavelet — outstanding work]
  #v(2pt)
  #text(size: 9pt, fill: luma(35%))[
    A single consolidated list of missing features, deferred features, and known
    implementation gaps. · Last consolidated: 2026-06-27.
  ]
])

This note gathers every still-open item from the dev notes and the source
comments into one place. Each entry is tagged with where it came from — a
`file:line` for code-grounded gaps, or a short source tag for design-level ones.

#block(fill: rgb("#eef3fb"), inset: 9pt, radius: 4pt, width: 100%, [
  *What is _not_ here.* Completed plans and resolved reviews were removed during
  this consolidation: the macro-component feature (PR \#12), the source/binary
  functor build path (PR \#24) and the monomorphic type system (PR \#20) all
  landed, and the two 2026-06-20 code reviews
  (`review.todo.typ`, `type-system-review.typ`) had essentially every finding
  fixed in follow-up commits (runtime float dispatch, strictly-binary + checked
  arithmetic, whole-word `-inf`, curated overload mangling with identifier-safe
  labels, the checker wired into `build`/`run`/`wit`, export de-duplication, …).
  The residue of those reviews that is _still_ live is folded in below.
])

= wasm backend ↔ interpreter parity gaps

The interpreter (`interp.rs`) is the semantics oracle; the wasm backend
(`emit.rs`) is validated against it and is explicitly a v0 subset. These are the
constructs the backend still rejects with an honest "not supported by the wasm
backend yet" rather than miscompiling — each is a place the compiler trails the
interpreter.

- #box-[*No garbage collection — leaks by design.* The linear-memory backend
  never frees; fine for short-lived commands, but it blocks any long-running
  compiled surface (notably a compile-and-run REPL). A GC-types backend with a
  linear-memory fallback is the intended end state. #src("emit.rs:6")]

- #box-[*Flag literals.* `{read write}` flag values are not lowered by the
  backend. #at("emit.rs:1267")]

- #box-[*Qualified symbols as values.* `alias/name` is only supported in call
  position; using it as a bare value errors. #at("emit.rs:1267 (Qsym arm)")]

- #box-[*`def` / `defmacro` as expressions.* `DefMacro`/`Def` forms are not
  compilable by the backend (they are an expand-time concern). #at("emit.rs:2054")]

- #box-[*Pattern coverage.* `Match` patterns in compiled code are limited to
  literals, names, list/tuple, record, and variant patterns. #at("emit.rs:2272")]

- #box-[*Argument bundling for imports.* Calling an imported function by bundling
  several arguments into one tuple parameter is unsupported. #at("emit.rs:2470")]

- #box-[*String/list fields inside a boundary record's in-memory store.*
  `store_to_mem` lays out scalar fields only. #at("emit.rs:3086")]

- #box-[*Large sum/flag types.* `variant` > 256 cases, `enum` > 256 cases, and
  `flags` > 32 members are rejected. #at("emit.rs:3150, 3159, 3174")]

- #box-[*Spilling > 16 flat params to memory.* A signature that flattens to more
  than 16 core params is rejected. #at("emit.rs:4152")]

- #box-[*Constructing user variant/enum cases in compiled code.* The dynamic core
  has constructors only for the built-in `ok`/`some`/`err`/`none`; a named 3+-case
  `variant`/`enum` `DefType` has no case constructor at the boundary, so a
  `DefType` body the emitter cannot render is silently left out and any reference
  to it surfaces the "not supported" error. #at("emit.rs:944") · #src("notes.md")]

- #box-[*A `set` functor handle returned over a _local-record_ element.* The
  functor build path is otherwise at full interpreter parity, but an export that
  returns the `set` handle over a locally-declared record element forms a WIT
  interface cycle (the element's interface and the resource's interface would each
  `use` the other), which WIT cannot express, so it is rejected with a clear
  error. Lifting it needs the element record hoisted into a shared types
  interface. #at("emit.rs:7977-7988") · #src("dd-type-system.typ §6")]

= Language features deferred / not yet implemented

- #box-[*Hygiene.* Expansion is unhygienic in the Common-Lisp/Clojure tradition;
  `gensym` is the discipline. A syntax-object layer over `wavelet:meta/code`
  (scope sets on nodes) is sketched and would extend the wire type without
  reshaping it. #src("design.md §10")]

- #box-[*Async.* Maps onto the Component Model's `stream`/`future` types as WASI
  0.3 lands; the intent is a `Fn` whose body awaits compiling to an async-lifted
  export, with no surface beyond an `Await` macro — but this is undesigned.
  #src("design.md §10")]

- #box-[*Pattern exhaustiveness.* Checking `Match` exhaustiveness is possible
  wherever the scrutinee's boundary type is known, but is currently at most a lint
  — not enforced. #src("design.md §10") · #at("emit.rs:267")]

- #box-[*Boundary coercions + a `safely` wrapper.* The "errors at the edge"
  coercion story and a `safely` wrapper form are unimplemented (no `safely` in the
  source). #src("notes.md")]

- #box-[*Calling file-local _functions_ at expand time.* Macro bodies run against
  builtins and previously-defined macros only; calling ordinary file-local
  functions during expansion is future work tied to the macro-component story.
  #at("expand.rs:1-9")]

- #box-[*User-declared resource types.* The backend handles imported opaque host
  resources and the built-in `cell` plus the `set` functor's exported resource,
  but a general user `DefType` resource (program-implemented, beyond `set`) is not
  yet emittable. #src("notes.md")]

- #box-[*`--fuse` and cross-component tail calls.* Tail calls are bounded by the
  component boundary; the `--fuse` optimisation that restores cross-component tail
  calls is unimplemented (no `fuse` in the source). #src("design.md §10, notes.md")]

- #box-[*`compose.wave` manifest.* The declarative composition manifest is not
  implemented. #src("notes.md")]

- #box-[*Richer inference for lists / options / results.* The checker still asks
  for annotations in some positions it could in principle infer. #src("notes.md")]

= Build, CLI & dependency gaps

- #box-[*Registry fetch of macro components.* `wkg` fetches dependency _WIT_ into
  `wit/deps` but does not fetch _components_, so a `macros: true` import must point
  at a locally-built `.wasm` (`from:` path or the conventional build location). A
  real `wavelet add` / registry fetch is deferred. #at("macrodep.rs:14-24") ·
  #src("notes.md")
  (Note: that comment references a non-existent `dev-notes/decouple-wasi.md`.)]

- #box-[*Runtime dep + macro library in one import.* A single import that is
  _both_ a runtime dependency and a macro library is an unsupported edge case (it
  would need two surfaces — a runtime import in the world plus a compile-time
  instantiation). #at("wit.rs:803-804") · #at("build.rs:81")]

- #box-[*`wavelet run` is an interpreter stand-in.* `run` resolves imports and
  executes on the tree-walker rather than building + composing; the compiled
  execution path is the replace-interpreter initiative below. #src("runner.rs")]

- #box-[*REPL needs readline.* `wavelet repl` has no line-editing/history.
  #src("notes.md")]

- #box-[*Shebang support.* `#!/usr/bin/env wavelet` so a `.wvl` file runs directly
  as a script (import `wasi:cli`, treat top-level code as `run`).
  #src("blueberries.md")]

- #box-[*Test the VS Code tooling end-to-end.* Verify the TextMate grammar +
  language config actually work in-editor. #src("blueberries.md")]

= Deferred initiative — replace the interpreter with the compiler

A coherent, multi-step initiative with its own detailed plan
(`dev-notes/replace-interpreter/`) and the feasibility study behind it
(`dev-notes/drop-the-interpreter.md`). Both are kept; this is the index entry.

Goal: make the wasm compiler the execution engine for the compile-time and CLI
surfaces (macro expansion, `repl`, `run`) so the tree-walker no longer _runs user
programs_ there. The interpreter is _retained_ as the differential-testing oracle.
The browser playground stays on the interpreter (out of scope).

Gating prerequisites, none yet done:

- #box-[*GC first* — without it a compile-and-run REPL leaks on every definition
  (see the backend section).]
- #box-[*Core / standard-library split (F1).* Move `map`/`filter`/`fold`/`range`/
  `zip`/`split`/`join`/`contains`/`apply`/`cell-*`/`to-*` and float/char
  arithmetic out of per-builtin special cases and into a Wavelet standard library
  over a small irreducible core, imported by default with a per-file opt-out.]
- #box-[*Value marshalling + printing (F2)* — a native reader that walks a result
  box in linear memory and renders it like `print_value`.]
- #box-[*Differential harness (F4)* — run every example through interpreter _and_
  compiled artifact and assert equality; land it before further backend changes.]
- #box-[*Diagnostic fidelity (F5)* — distinguishable trap codes per failure class
  + a form→source-span table so a compiled trap points at the offending form.]
- #box-[*Compiled `Quote`/`Quasi`/`Unquote`/`Splice` + `expand` codegen* — the
  riskiest single piece; needed so a REPL line can define and use a macro without
  the interpreter in the execution path.]

= Docs gaps

- #box-[Soften the "argument" wording, and the "NO-FFI!!" example.
  #src("blueberries.md")]
- #box-[A "Trivia" callout beginners can ignore (e.g. the "Lisp-1" mention).
  #src("blueberries.md")]
- #box-[Move the formal grammar to an appendix; it lands too early.
  #src("blueberries.md, notes.md")]
- #box-[A full per-type rules page (what can be a record key, what flags are, …),
  not just the type summary. #src("notes.md")]
- #box-[A dedicated macros page: how to write them and why they matter.
  #src("notes.md")]
- #box-[Canonical formatting / a formatter, ideally user-macro-aware (`If` over
  three lines, `Package` on one). #src("notes.md")]
- #box-[Worked examples building each WASI app type (cli, http, deploy targets)
  and using each WASI proposal (clocks, random, filesystem, sockets); publishing a
  library via `wkg`. #src("notes.md")]
- #box-[Pick a nicer docs font (low priority, subjective). #src("blueberries.md")]

= Open design questions

Design-level, not yet decided. From `dd-type-system.typ §Open questions`,
`design.md §10`, and `notes.md`.

- #box-[*Functor spelling.* Is `Import {pkg: … elem: t as: …}` the right surface,
  or should application be its own form (an `Instantiate` macro)? How are
  multi-parameter functors expressed? #src("dd-type-system.typ")]
- #box-[*Derive surface & extent.* Is `Derive {Eq Ord Show} t` right; which
  classes ship built-in; can users author their own derivers? #src("dd-type-system.typ")]
- #box-[*Overload declaration.* Implicit same-name sets vs. an explicit grouping
  form; the exact resolution algorithm and its interaction with `The`.
  #src("dd-type-system.typ")]
- #box-[*Boundary name-mangling control.* When an overload set is exported, are
  the WIT names compiler-chosen or user-controlled via `Export`?
  #src("dd-type-system.typ")]
- #box-[*Recursion: resources vs. arenas.* Should the language offer a derive that
  generates an arena representation (and accessors) from a recursive type, instead
  of hand-rolled arenas like `tree`? #src("dd-type-system.typ")]
- #box-[*Unify qualified references with record access / call-chaining.* Could
  `foo/bar` become `foo.bar()` if qualified imports returned a record and records
  supported virtual accessors? Add `1.increment()` → `(increment 1)` chaining as
  pure rewriting. #src("notes.md")]
- #box-[*Alternatives to the bundled standard library.* `std: false` exists; push
  further toward the stdlib being just another pullable dependency, including
  purely-functional or real-time variants. #src("notes.md")]
- #box-[*Trim the surface?* Open questions on whether to drop: the unit value,
  doc comments (`///`), `f(x y)` tuple-call sugar, grouping `(a)` semantics; and
  whether `Do`/`Match` could live in the standard library rather than be special
  forms. #src("notes.md")]

= Future directions / ideas (not scheduled)

- #box-[*Wavelet as a markup language* — a Markdown-class markup as a pure Wavelet
  library, no core/stdlib changes. Detailed in `dev-notes/wavelet-as-markup.typ`.]
- #box-[*Closures as components over the wire* — bundle a captured environment in
  a component with an `apply` method (Erlang-style). #src("notes.md")]
- #box-[*The compiler as an importable component* — enabling closure compilation
  and in-language build pipelines. #src("notes.md")]
- #box-[*Deployment targets* — standalone CLI (bundled runtime), JS in browser /
  node / bun, JVM, Docker, Kubernetes; publish a wasm build of the compiler to a
  registry. #src("notes.md")]
- #box-[*File extension* — preference for `.wlt` over `.wvl`. #src("notes.md")]
