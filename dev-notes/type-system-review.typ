#set document(title: "Wavelet type system: review findings and open issues")
#set page(
  paper: "a4",
  columns: 2,
  margin: (x: 1.7cm, y: 2cm),
  numbering: "1",
)
#set text(font: "New Computer Modern", size: 9.5pt)
#set par(justify: true, leading: 0.6em)
#show heading: set block(above: 1.1em, below: 0.6em)
#set heading(numbering: "1.1  ")

#show raw.where(block: false): box.with(
  fill: luma(240),
  inset: (x: 3pt, y: 0pt),
  outset: (y: 3pt),
  radius: 2pt,
)
#show raw.where(block: true): block.with(
  fill: luma(245),
  inset: 8pt,
  radius: 4pt,
  width: 100%,
)

#let note(body) = block(
  fill: rgb("#eef3fb"),
  inset: 9pt,
  radius: 4pt,
  width: 100%,
  body,
)
#let warn(body) = block(
  fill: rgb("#fbeeee"),
  inset: 9pt,
  radius: 4pt,
  width: 100%,
  body,
)

// Figures (code + tables) are numbered per-section as "Figure 2.2", span both
// columns, and float — matching dd-type-system.typ. The within-section index
// resets at each level-1 heading; the section number is passed explicitly so a
// floating figure can't pick up the wrong "N.x" prefix from its landing page.
#show heading.where(level: 1): it => {
  counter(figure.where(kind: "figure")).update(0)
  it
}
#show figure.where(kind: "figure"): set figure.caption(position: bottom)
#let fig(sect, body, caption) = figure(
  body,
  caption: caption,
  kind: "figure",
  supplement: [Figure],
  numbering: n => numbering("1.1", sect, n),
  placement: auto,
  scope: "parent",
)

// Full-width title + abstract, spanning both columns at the top of page 1.
#place(top, scope: "parent", float: true, clearance: 1.4em, [
  #align(center)[
    #text(17pt, weight: "bold")[Wavelet's type system: review findings and open issues]
    #v(0.3em)
    #text(10pt, style: "italic")[Code review of PR #20 (Steps 1–12) — issues left unfixed, with reproductions]
  ]
  #v(0.4em)
  #note[
    *Summary.* A maximum-effort review of the monomorphic type system found that
    the static checker (Phase A) and the WIT-text synthesizer are sound on the two
    paths the test suite exercises: the interpreter / playground (`eval_snippet`)
    and `wit::synthesize`. Three checker *false positives* were fixed in place
    (commit `7f91902`; see §1). This document records the issues that were
    *deliberately left unfixed* — each is real and reproduced, but its fix either
    changes behaviour the tests pin, or reaches well outside the reviewed diff
    (the emit backend, the CLI command dispatch, the docs). Two of them are
    load-bearing: the headline feature — an *exported overload set* (and therefore
    every `Derive`d operation) — *cannot currently be compiled to a component* by
    `wavelet build`, and any ordinary function whose name collides with a builtin
    is silently mis-mangled at the boundary. The findings are ordered by severity.
  ]
])

= Scope, method, and what was already fixed

The review read every hunk of `main...HEAD` and the enclosing functions, ran ten
finder angles, verified each candidate against the interpreter oracle
(`interp.rs` / `builtins.rs`), and swept once more for gaps. The full test suite
is green before and after the fixes.

Three checker false positives — cases where `check.rs` rejected a program the
interpreter *runs*, violating the module's own "never preempt a runtime success"
invariant — were fixed and locked with regression tests in `tests/type_system.rs`:

- `min` / `max` were modelled as numeric-only, but the interpreter dispatches
  them through `compare`, which is defined over strings and chars
  (`builtins.rs:144`). `min("a" "b")` is now accepted.
- `The list(s32) [...]` element-checked the list, but the interpreter's `The`
  only conformance-checks a bare `Sym` annotation — a `list(…)` constructor is
  never checked (`interp.rs:282`). A constructor annotation is now gradual.
- `len` was modelled as concrete `s64`, so `The u8 len(xs)` was rejected; `len`
  returns a plain `Int` that range-checks against any int type, so it is now an
  unconstrained int literal.

Everything below is *unfixed*. Each entry gives the symptom, a reproduction, the
mechanism (with `file:line`), why it was deferred, and a suggested direction.
@fig-summary is the index.

#fig(1,
  table(
    columns: (auto, auto, 1fr, auto),
    inset: 6pt,
    align: (x, y) => if y == 0 { center } else { left },
    table.header([*§*], [*Severity*], [*Symptom*], [*Site*]),
    [2], [High], [Exported overload set / any `Derive`d op cannot be built to a component.], [`emit.rs:3782`],
    [3], [High], [Any function named like a builtin is force-mangled; constructor params yield illegal WIT names.], [`wit.rs:367`],
    [4], [Medium], [Checker runs only in `eval_snippet`; `build`/`run` skip it; docs over-promise.], [`lib.rs:134`],
    [5], [Medium], [Overloads differing only past the first parameter mangle to one WIT name.], [`wit.rs:375`],
    [6], [Low], [`Derive` auto-exports collide with an explicit re-export of the same op.], [`expand.rs:159`],
    [7], [Low], [Any `Import` carrying `elem:` is hijacked as a functor and hard-errors.], [`wit.rs:299`],
    [8], [Low], [`wavelet wit` skips expansion, so `Derive` is invisible on that path.], [`main.rs:48`],
    [9], [Low], [`The` / string-builtin errors diverge in wording from the runtime message.], [`check.rs`],
    [10], [Cleanup], [Duplicated range table, repeated body inference, hardcoded functor, over-broad reader arm.], [various],
  ),
  [Index of unfixed findings, by severity. "Site" is the primary location; most findings touch more.],
) <fig-summary>

= An exported overload set cannot be built to a component

#warn[
  *Impact.* This is the headline feature of Steps 8–9. It synthesizes correct WIT
  text, and the interpreter resolves it correctly, but `wavelet build` fails to
  emit a component for it. No test catches the gap because every Step 8–12 test
  asserts on `synth` (WIT text) only — the emit code generator is never reached.
]

== Symptom

The single-definition Step 8 example synthesizes, but does not build
(@fig-emit-repro). The two-definition overload set fails identically.

#fig(2,
  ```
  // source.wave
  Package "demo:geo@0.1.0"
  DefType point {x: s32 y: s32}
  Def eq Fn {a: point b: point} true
  Export eq

  $ wavelet wit  source.wave          # OK — prints the synthesized world:
  //   interface api { eq-point: func(a: point, b: point) -> bool; }
  $ wavelet build source.wave -o c.wasm
  //   error: export `eq-point` has no Def Fn
  ```,
  [`wavelet wit` succeeds and `wavelet build` fails on the same well-typed source.],
) <fig-emit-repro>

== Mechanism

There are two name spaces that never meet:

+ *WIT synthesis* decides that `eq` is an overload export and renames it to the
  mangled WIT label `eq-point` (`wit.rs:229`, via `mangle_name`, `wit.rs:375`).
  The export signatures therefore carry *mangled* names.

+ *Emit* builds its function map `em.funcs` keyed by the *original* definition
  names taken from `info.defs` / `internal_order` (`emit.rs:3751`–`3754`) — here,
  `eq`. When it then walks the export signatures and looks each body up by
  `sig.name` (the mangled `eq-point`) at `emit.rs:3782`, the lookup misses and it
  raises `export eq-point has no Def Fn` at `emit.rs:3783`.

A second, deeper problem compounds it: `info.defs` is a `name -> (params, body)`
map in which *same-named definitions collapse, last-wins* (`wit.rs:23`). For a
genuine two-member overload set, only one body survives in `info.defs` at all, so
even a name-matched lookup could not recover both members' bodies. The
overload-aware run path solves this for the interpreter by rewriting each member
to a unique internal symbol `name$k` (`check.rs`, `resolve_overloads`); emit
consumes neither that rewrite nor the `name-<type>` WIT mangling.

== Why it was deferred, and a direction

Making this work is emit-backend *feature completion*, not a localized bug fix:
the export lowering must thread each mangled boundary name back to the specific
overload member's body, which in turn requires emit to read an *un-collapsed*
view of the defs (the new `info.fn_defs`, `wit.rs`, already gathers every member
per name — emit just doesn't use it yet). The cleanest shape is probably for the
overload-export branch in `collect` to record, alongside each emitted `FuncSig`,
the exact `(params, body)` it was synthesized from, so emit keys on identity
rather than on a reconstructed name. Until then, `wavelet build` should at least
*fail loudly and early* for any program containing an overload export, rather
than deep inside code generation, so the limitation is not mistaken for a
miscompile.

= Overload-export mangling is over-broad and can produce illegal WIT

== Symptom

A perfectly ordinary function whose name happens to match a builtin is mangled
at the boundary, and when its first parameter is a constructor type the mangled
name is *not a legal WIT identifier* (@fig-getmangle).

#fig(3,
  ```
  Package "demo:util@0.1.0"
  Def get Fn {xs: list(s32)} xs
  Export get

  $ wavelet wit source.wave
  //   interface api {
  //     get-list<s32>: func(xs: list<s32>) -> list<s32>;
  //   }                ^^^^^^^^^^^^  '<', '>' are illegal in a WIT label
  $ wavelet build source.wave -o c.wasm     # rejected downstream
  ```,
  [A user function named `get` is force-mangled to the invalid label `get-list<s32>`.],
) <fig-getmangle>

== Mechanism

`is_overload_export` (`wit.rs:367`) returns `true` for *any* name in
`builtins::NAMES` — not only the operator-like names (`eq`, `lt`, `add`, …) the
overload story is about, but the whole collection and string library: `get`,
`put`, `push`, `concat`, `head`, `tail`, `reverse`, `range`, `map`, `filter`,
`fold`, `zip`, `split`, `join`, `contains`, `read`, and the `to-*` conversions —
roughly sixty names. A single ordinary `Def` of any of them is therefore treated
as an overload set of one and renamed. `mangle_name` (`wit.rs:375`) appends
`type_text` of the first parameter; for a constructor type that text is
`list<s32>`, so the resulting label carries angle brackets and is rejected by any
WIT consumer.

== Why it was deferred, and a direction

The *single*-definition mangling is intended, not accidental: the Step 8 test
`mangled_overload_signature_is_concrete` pins a lone `Def eq` + `Export eq` to
`eq-point`. So the builtin-collision trigger cannot simply be dropped — that
would turn the test red. Fixing this well needs a deliberate decision on two
axes, ideally both:

- *Which names trigger mangling?* Restrict the builtin-collision branch to a
  curated set of overloadable *operations* (the comparison / arithmetic
  operators and the derivable ops `eq`, `compare`, `show`, `hash`), so library
  functions like `get` keep their given names.

- *What is a safe mangled label?* `mangle_name` must emit an identifier-safe
  token: `list<s32>` should render to something like `list-s32`, never with `<`,
  `>`, spaces, or commas. This is necessary even for an *intended* overload whose
  distinguishing parameter is a constructor type.

= The checker runs on one path only

== Symptom

Type errors and overload resolution happen in the playground but not in the CLI.
The same overloaded program that the playground resolves correctly executes the
*wrong* body under `wavelet run`, and `wavelet build` performs no type checking
at all (@fig-paths).

#fig(4,
  table(
    columns: (auto, 1fr, 1fr),
    inset: 7pt,
    align: left,
    table.header([*Path*], [*Entry point*], [*Checker / overload resolution?*]),
    [playground], [`eval_snippet` (`lib.rs:134`)], [yes — `check::resolve_overloads`],
    [`wavelet run`], [`runner.rs`], [no — `Def` is plain last-wins shadowing],
    [`wavelet build`], [`build.rs` / `emit.rs`], [no — emit consumes raw forms],
    [`wavelet wit`], [`main.rs:48`], [no — and no expansion either (§8)],
  ),
  [Only the playground path type-checks and resolves overloads.],
) <fig-paths>

== Mechanism and impact

`check::resolve_overloads` is invoked from exactly one place, `eval_snippet`
(`lib.rs:134`); `grep` for `check::` finds no other caller. On the `wavelet run`
path the interpreter binds same-named `Def`s by ordinary last-wins shadowing, so
an overloaded call reaches whichever definition was read last and then fails or
silently does the wrong thing at runtime — precisely the pre-feature behaviour
the Step 6 tests describe as "today". On the `wavelet build` path nothing rejects
an ill-typed program.

This also makes the prose over-promise. The docs state types are decided "before
any code runs … if it builds, it is well-typed", and the `CHANGELOG` says the
checker "runs before emit" and errors "surface at build time". With the wiring as
it is, those guarantees hold only for the playground. Either the checker should
be lifted into the shared front end (so every path — `run`, `build`, `wit` —
sees it), or the documentation should be narrowed to claim only what the
playground delivers.

= First-parameter-only mangling collides

`mangle_name` distinguishes overload members by the WIT type of the *first*
parameter alone (`wit.rs:375`), and the export loop pushes one signature per
member with no collision check. Two members that differ only past the first
parameter therefore mangle to the same WIT label and synthesize two functions of
the same name into one interface — invalid WIT (@fig-collide).

#fig(5,
  ```
  Package "demo:geo@0.1.0"
  DefType point {x: s32 y: s32}
  Def eq Fn {a: point b: string} true
  Def eq Fn {a: point b: s32}    true
  Export eq

  // synthesized interface api {
  //   eq-point: func(a: point, b: string) -> bool;
  //   eq-point: func(a: point, b: s32)    -> bool;   // duplicate name
  // }
  ```,
  [Two overloads sharing a first-parameter type collapse to one mangled label.],
) <fig-collide>

The current scheme is documented as keying on "the member's distinguishing
(first) parameter"; the assumption that the first parameter distinguishes the set
is unchecked. A fix either mangles over *all* parameter types (so the label is a
function of the whole signature) or, at minimum, detects the collision and
reports it as a compile error rather than emitting invalid WIT.

= `Derive` auto-export collides with an explicit re-export

`derive_roots` always splices an `Export {op}-{tname}` for every derived class
(`expand.rs:159`) in addition to the derived `Def`. If the author also writes an
explicit `Export eq-point` — plausible, since the derived operation is an
ordinary exportable function and the design figures show derived ops crossing the
boundary — synthesis sees the same export twice and emits a duplicate WIT
function (@fig-derive-dup).

#fig(6,
  ```
  Package "demo:geo@0.1.0"
  DefType point {x: s32 y: s32}
  Derive {Eq Ord Show} point     // auto-emits Export eq-point, …
  Export eq-point                 // explicit re-export → defined twice
  ```,
  [`Derive`'s implicit export plus an explicit one yields a duplicate function.],
) <fig-derive-dup>

The question this raises is a small design one: should `Derive` auto-export at
all, or only emit the definitions and leave exporting to the author? If it keeps
auto-exporting, `collect` should de-duplicate identical export declarations
before lowering.

= A functor `Import` hijacks any `elem:` field

`parse_functor` (`wit.rs:299`) classifies *any* `Import` record that carries an
`elem:` field as a functor instantiation, and then hard-errors if the `pkg:` is
not one of the known functor packages: `unknown functor package …`. `elem` is a
generic field name, so an ordinary import that happens to use it is silently
reinterpreted and rejected (@fig-functor-hijack), where previously the unknown
field was ignored and the import processed normally.

#fig(7,
  ```
  Import {pkg: "acme:widget/thing" elem: point as: w}
  //   was: an ordinary import of acme:widget/thing
  //   now: error — unknown functor package `acme:widget/thing`
  //        (known: wavelet:coll/set)
  ```,
  [Any import carrying `elem:` is treated as a functor and rejected if `pkg` is unknown.],
) <fig-functor-hijack>

The risk is low — `elem:` is functor-specific by current convention — but the
classifier is keyed on an easily-collided field name rather than on the package
identity. Keying the functor test on a recognised functor `pkg:` (and treating
`elem:` on a non-functor package as an ordinary field) would remove the hazard.

= `wavelet wit` does not expand macros

`wit_cmd` synthesizes straight from `read_file` output without running
`expand_file` (`main.rs:48`–`49`), so `Derive` — and any other macro — never runs
on the `wavelet wit` path. A program that `wavelet build` compiles (build *does*
expand) fails under `wavelet wit` with `Export eq-point has no definition`,
because the derived `Def eq-point` was never produced. The two CLI subcommands
thus disagree about the same source. The fix is small and local — expand before
synthesizing in `wit_cmd` — but it sits in `main.rs`, outside the reviewed diff,
and `wavelet wit`'s no-expand behaviour predates this branch, so it is recorded
here rather than patched blind.

= Message-divergence nuances

Two classes of error message diverge in wording from the interpreter's runtime
message (@fig-messages). Both concern programs that are ill-typed either way, so
neither is a false positive; they are recorded only because the module's stated
invariant is "never preempt a runtime error *with a different message*".

#fig(9,
  table(
    columns: (auto, 1fr, 1fr),
    inset: 6pt,
    align: left,
    table.header([*Program*], [*Checker says*], [*Interpreter says*]),
    [`The s32 "hi"`], [type mismatch: expected S32, got String], [The: \"hi\" does not conform to type `s32`],
    [`upper(42)`], [`upper` requires a string operand], [`upper` expects a string, got 42],
  ),
  [Compile-time wording versus runtime wording for ill-typed programs.],
) <fig-messages>

This is judged *benign and partly by design*: the checker deliberately emits its
own compile-time diagnostics — the arithmetic operand message
(`add requires numeric operands`) is even locked as a documentation example — and
a value-based runtime message ("got 42") cannot in general be reproduced
statically, since the offending value need not be a literal. The `The`-literal
arms already reproduce the runtime wording where they can. No change is proposed;
the entry exists so a future decision to unify diagnostic wording knows where the
seams are.

= Cleanup and altitude notes

These do not affect correctness today; they are recorded so the debt is visible.

- *Duplicated range table.* `check.rs`'s `int_in_range` reproduces the per-width
  integer bounds that `interp.rs`'s `check_type` already encodes — the comment
  even says it "mirrors" them. Two copies with nothing enforcing agreement; a
  future bound change must touch both or the compile-time `The` check silently
  diverges from the runtime one. A shared, type-name-parameterised predicate
  would make them one source.

- *Repeated body inference.* In return-type-directed overload resolution,
  `infer_sig_result` (`check.rs:835`) re-runs full inference over each candidate
  body — once per surviving candidate, then again for the winner — and recurses
  if those bodies contain their own overloaded calls. Harmless for today's tiny
  bodies; memoising per `(sig, scope)` would remove the redundant descent.

- *Hardcoded single functor.* The functor machinery is a bespoke special case for
  exactly one template, `Set`: a single `FunctorKind` variant, a `parse_functor`
  that recognises only `coll/set`, and a `functor_op_table` / `functor_interface`
  pair that inline the four ops and the resource shape as literal strings in two
  places that can drift. Acceptable as a first cut; a data-driven template
  descriptor would let the next functor be added without editing four sites.

- *Over-broad reader arm.* The `Tok::Title`-in-flags arm added for
  `Derive {Eq Ord}` fires in *every* `{…}` flags literal, not only in `Derive`
  argument position — the reader has no `Derive` context there. A TitleCase token
  mistakenly placed in any other flags literal is now silently accepted as a flag
  name instead of raising the previous `expected a flag name` error, weakening an
  existing diagnostic. Scoping the recovery to the derive-class position would
  keep the change at the altitude of the feature.
