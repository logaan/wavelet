// dev-notes/functor/index.typ — index for the functor build/emit work plan.
// Render: `typst compile index.typ`.

#set document(title: "Functor build/emit — plan index", author: "Claude (Opus 4.8)")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.62em)
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#set heading(numbering: none)
#show heading.where(level: 1): set text(size: 14pt)
#show heading.where(level: 2): set text(size: 11pt)

#block(
  fill: luma(96%), width: 100%, inset: 12pt, radius: 5pt,
  [
    #text(size: 16pt, weight: "bold")[Functor build/emit — implementation plan]
    #v(2pt)
    #text(size: 9pt, fill: luma(35%))[
      Goal: make `wavelet build` emit the `set` functor as an exported WIT
      resource, at full parity with the interpreter. ·
      Executed by one fresh subagent per step. · Planned: 2026-06-26.
    ]
  ],
)

= What this is

Wavelet's `set` functor — `Import {pkg: "wavelet:coll/set" elem: T as: alias}` —
already does two of the three things it needs to:

- *Synthesis* (`wit.rs`): it derives a concrete, per-element WIT interface
  (`point-set`) containing a `resource set` with `new` / `add` / `contains` /
  `size`. `wavelet wit` prints it.
- *Run* (`interp.rs` + the `set-*` builtins in `builtins.rs`, landed by PR #22):
  the interpreter executes the functor and is the *semantics oracle*.

What is missing is the wasm backend. `wavelet build` currently fails with a
clean, honest error (added by PR #22) because emitting the component means
emitting an *exported, program-implemented* WIT resource — a resource table,
the `resource.new` / `resource.rep` / `resource.drop` intrinsics, core
functions for the constructor / methods / destructor, and `own<set>` handle
lift/lower at the boundary. `emit.rs` does not have this yet: it only handles
*imported*, opaque host resources (a handle is an opaque `i32` it never
inspects).

This plan delivers that backend in six isolated steps, each run by a fresh
subagent to keep context small. The interpreter stays the oracle: the backend
must agree with it on every program. A backend that diverges from the
interpreter is *the* hard bug this project forbids (`CLAUDE.md`).

= Decisions locked (from the planning interview)

- *Acceptance bar — FULL interpreter parity.* Any functor program the
  interpreter runs must also `build` and execute identically: any element type,
  multiple `set` instantiations in one world. (The backend machinery —
  structural `eq_raw`, boxed lists — is generic, so this is only marginally more
  than the single worked example.)
- *Branch basis — STACKED on PR #22.* The shared branch `worktree-functor-build`
  was created from PR #22's tip (`worktree-agent-ab091bae27225f3b1`), so the
  run-path and the parity-test harness are already present.
- *Landing — ONE branch, ONE PR.* All steps commit to the one shared branch;
  a single PR opens only at the end (step 06), after the full `cargo test`
  suite is verified green. No per-step PRs.

= The shared-branch / fresh-agent model

All steps run on the one branch `worktree-functor-build`, in the worktree
`.claude/worktrees/functor-build`. "A fresh subagent per step" does *not* mean a
fresh branch: each agent enters the *same* worktree
(`EnterWorktree path=.claude/worktrees/functor-build`), reads its step brief plus
the previous step's summary, does the work, commits to the shared branch, and
writes its own summary for the next agent. Summaries are the inter-agent memory
and are committed so the next agent sees them.

Every executing agent must first read `plan/00-agent-rules.typ` and follow it
verbatim. The critical rules are also restated at the top of each step brief.

= Steps

+ `plan/01-abi-spike.typ` — De-risk the one true unknown: the exact
  wit-component 0.251 convention for an *exported, implemented* resource. Prove
  it with a tiny scratch component. → writes `summaries/01-abi.typ`.
+ `plan/02-rep-and-bodies.typ` — Emit the `set` representation (a mutable cell →
  boxed list, mirroring the interpreter's `Value::Cell(RefCell<Lst>)`) and the
  constructor / add / contains / size / dtor core functions, reusing existing
  emit helpers. → writes `summaries/02-bodies.typ`.
+ `plan/03-wire-emit-build.typ` — Drop the early-return; wire `type_env` so
  `set` is a resource; export the resource funcs and import the intrinsics; make
  the worked example produce a *validating* component. → writes
  `summaries/03-wiring.typ`.
+ `plan/04-call-routing-and-handles.typ` — Route `alias/op` calls (`pts/new`,
  `pts/add`, …) to the resource funcs; lift/lower `own<set>` / `borrow<set>`
  handles at the boundary so the worked example *runs correctly*. → writes
  `summaries/04-routing.typ`.
+ `plan/05-parity-tests.typ` — Build-through-emitter + execute-in-wasmtime parity
  tests: worked example, multiple instantiations in one world, a primitive
  element type. Fix any divergence (oracle wins). → writes `summaries/05-parity.typ`.
+ `plan/06-downstream-and-pr.typ` — Tie-off per `CLAUDE.md`: docs, examples
  regen, CHANGELOG, `dd-type-system.typ` open-question resolution; full test
  sweep; open the single PR. → writes `summaries/06-done.typ`.

= Dependency chain

`01 → 02 → 03 → 04 → 05 → 06`, strictly sequential. Each step trusts the
previous summary and need not re-derive it. Intermediate commits may leave the
build temporarily red (e.g. between 02 and 03); each step's summary states the
build state it leaves behind.

= Why the work is well-isolated

A `set` rep is expressible almost entirely with machinery `emit.rs` already has:
boxed lists (`list_box`/`seq_box`), a one-word mutable cell (`alloc`), and
structural equality (`eq_raw`). The genuinely new piece is narrow: emitting an
*exported* resource (the intrinsics + constructor/method/dtor funcs + handle
lift/lower) and routing the `alias/op` calls to it. So most agents read only
their step brief, the prior summary, and a handful of cited `emit.rs` /
`wit.rs` spots — not the whole compiler.
