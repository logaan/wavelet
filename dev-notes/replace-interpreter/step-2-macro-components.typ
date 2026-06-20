// step-2-macro-components.typ — compile macros to components; no interpreting.
// Render: `typst compile dev-notes/replace-interpreter/step-2-macro-components.typ`

#set document(title: "Step 2 — Compiled macro components", author: "Claude (Opus 4.8)")
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
  #text(size: 16pt, weight: "bold")[Step 2 — Compile macro definitions into macro components]
  #v(2pt)
  #text(size: 9pt, fill: luma(35%))[No interpreting at expand time. Strategy A (embed interpreter) → strategy B (compile the body). See `index.typ`.]
])

= 1 · Goal

Every macro — local `DefMacro` and imported macro library — runs as *compiled*
wasm at expand time, so `expand.rs` needs no `Interp`. A macro is a pure function
from forms to a form; forms are exactly the values the backend already boxes
(`TAG_TUP`/`TAG_VAR`/`TAG_REC`/`TAG_LIST`/scalars, #at("emit.rs:44-54")), so a macro
body is "just a function" `emit.rs` can compile — once a few compile-time forms and
builtins are taught to the backend.

= 2 · Current state

- *Local macros interpret.* `expand_file` evaluates `DefMacro` forms and expands
  call sites via `Interp::expand_once` (#at("expand.rs:136"), #at("interp.rs:319")); quasiquote
  is `Interp::quasi` (#at("interp.rs:350")).
- *Foreign macros are components, but the guest interprets.* `macros: true` imports
  resolve to a `wavelet:meta/macros` component run under `wasmtime` (#at("macrodep.rs"),
  #at("host.rs")), but the guest `macrolib.rs` *is the interpreter compiled to wasm*
  ("strategy A"). `macrobuild::build_macro_component` bundles that guest.
- *The boundary contract is fixed:* `manifest() -> list<(string, u32)>` and
  `expand(name, args: tree) -> result<tree, string>`, with `tree`⇄`arena`
  marshalling in `meta.rs`. Keep this contract; only change who implements `expand`.

= 3 · Target: strategy B

A macro component is *an ordinary compiled Wavelet component* that exports
`wavelet:meta/macros`. Its `expand` export:

#block(inset: (left: 1.0em), [
  1. receives `args: tree` (a WIT `record { nodes: list<node>, root: u32 }`) — the
     backend already lifts records/lists/variants across the boundary (verified for
     option/result/variant/record/list);
  2. converts that `tree` into the box form-value representation (a compiled
     `tree → box` adapter);
  3. dispatches on `name` to the *compiled* macro body (params bound from the call
     form's arguments, exactly as `expand_once` binds them);
  4. converts the returned box form back to a `tree` (a compiled `box → tree`
     adapter) and returns `ok(tree)`.
])

No interpreter in the guest. The only genuinely new codegen is the compile-time
form machinery the bodies use.

= 4 · Work breakdown

#task[*Compile `Quote`.* `Quote form` builds the quoted value per `form_to_value`
  (#at("value.rs:104")): `Sym → Variant(name, none)`, `Tup → tuple of quoted`, etc. With
  no holes the result is constant — emit it as a static data box. Backend currently
  rejects quote/quasi (#at("emit.rs:1392")); this is the first removal.]

#task[*Compile `Quasi`/`Unquote`/`Splice`.* Port `Interp::quasi` (#at("interp.rs:350"))
  to codegen: walk the template tracking quasi depth; at depth 1, `Unquote(e)` emits
  the compiled `e`, and `Splice(e)` in a sequence evaluates to a list and concatenates
  into the surrounding `TAG_LIST`/`TAG_TUP` builder; deeper levels rebuild the node as
  data. Reuse `seq_box`/`rec_box`/`var_box` (#at("emit.rs:1083")) for construction.]

#task[*Compile `gensym`.* A module global counter (`i64`) plus a helper that formats
  `g{n}-gen` (reuse the int→string path in `to_str`, #at("emit.rs:3802")) and wraps it as
  a payload-less `TAG_VAR`. Decide determinism: counter seeded per `expand` call vs
  per component instance (must be stable across a build).]

#task[*Compile the form-introspection builtins.* `form-kind` (tag → string,
  #at("builtins.rs:391")), `rec-key`/`rec-val` (read field 0 of a `TAG_REC`,
  #at("builtins.rs:410")). Straight codegen over box tags. These are what real macros
  use (e.g. the `try-let` example destructures a binding record).]

#task[*Build the `tree`⇄`box` adapters.* The guest needs `tree → box` (lift incoming
  `args`) and `box → tree` (lower the result). `tree`/`node` are a record + variant,
  both within the backend's boundary ABI. Write the adapters in Wavelet as a compiled
  prelude, or as generated helpers, so they themselves need no interpreter.]

#task[*Regenerate the macro guest from `emit` (strategy B).* Replace the interpreter
  guest in `macrobuild::build_macro_component` with one whose `manifest`/`expand` are
  compiled: `manifest` is a constant from the `DefMacro` heads/arities; `expand`
  dispatches by name to each compiled body. Keep `is_macro_library` detection
  (#at("macrobuild.rs")) and the output naming unchanged.]

#task[*Route local `DefMacro` through the same path.* At build/expand time, collect a
  file's `DefMacro`s into a synthetic macro library, compile it to a component once,
  and expand local call sites through it — merging with the foreign path in
  `FileExpander` (#at("macrodep.rs")). Remove `Interp::expand_once` from `expand.rs`
  (#at("expand.rs:136")). Only do this when the file actually defines or uses macros.]

#task[*Decide `expand`-inside-macro.* The `expand` builtin (#at("builtins.rs:365"))
  performs one expansion step from within a macro body — recursive into the macro
  machinery. Initially *reject* it inside compiled macros (rare; the shipped examples
  don't use it) or route it back to the host expander. Document the limitation.]

#task[*F4 differential tests for macros.* Expand a corpus of macros through both the
  interpreter and the compiled component; assert identical output forms (the existing
  foreign-macro tests in `lib.rs` are the seed).]

= 5 · Risks

#risk[*`Quote`/`Quasi` codegen is the crux.* It is unimplemented today and must
  exactly match `Interp::quasi` depth semantics, including nested-quasi protection and
  splice-into-sequence. Mirror the interpreter test-for-test.]

#risk[*`expand`-inside-macro recursion.* No clean compiled story without a
  host-mediated callback or a guest mini-expander. Defer and document rather than
  block the step.]

#risk[*Build-time cost.* Compiling + instantiating a `wasmtime` component to expand
  local macros is far heavier than in-process interpretation. Cache per file/package
  (the foreign resolver already caches per package, #at("macrodep.rs:82")); skip
  entirely for macro-free files. This cost lands hardest in the REPL — see Step 3.]

#risk[*`gensym` hygiene.* Counter scoping must stay deterministic and collision-free
  across separately compiled macro invocations within one build.]

= 6 · Exit criteria

- `expand.rs` contains no `Interp`/`expand_once` call; the macro guest links no
  interpreter (`macrolib`'s eval role is gone).
- Local and foreign macros expand identically to the interpreter across the macro
  corpus (F4), including the `try-let` and nested-quasi cases.
- `cargo test` green; macro-free builds incur no macro-component cost.
