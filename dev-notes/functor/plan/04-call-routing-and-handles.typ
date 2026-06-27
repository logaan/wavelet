// dev-notes/functor/plan/04-call-routing-and-handles.typ — Step 04: routing + handles.

#set document(title: "Step 04 — call routing and own<set> handles")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.62em)
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#set heading(numbering: none)
#show heading.where(level: 1): set text(size: 13pt)

= Step 04 — route `alias/op` calls and lift/lower handles

*First read `plan/00-agent-rules.typ`*, then `summaries/03-wiring.typ` (and 02,
01 as needed). Critical rules: branch `worktree-functor-build` via
`EnterWorktree path=.claude/worktrees/functor-build`; commit as you go with the
two trailers; no PR; interpreter is the oracle.

== Goal

Make the worked example *run correctly* when executed: route the `alias/op`
calls in `Def` bodies (`pts/new()`, `pts/add(s p)`, `pts/contains`, `pts/size`)
to the resource core funcs from step 02/03, and lift/lower `own<set>` /
`borrow<set>` handles at the export boundary.

== How `alias/op` calls work today

A qualified call `alias/op` already has machinery for the *imported*-resource
case. See `dep_func_op` (`emit.rs:964`) and its doc comment (`emit.rs:952`–
`963`): for a dep resource `r`, `r/new` resolves to `[constructor]…`, `r/op` to
`[method]….op`, `r/drop-…` to `[resource-drop]…`. Mirror this for the
SYNTHESIZED resource: when the head is `alias/op` and `alias` is a functor
instantiation in `info.functors`, route to the funcs you exported in step 03
rather than to a dep.

The run-path equivalent is `bind_functor` in `builtins.rs` (from PR #22), which
binds `alias/new` → `set-new`, `alias/add` → `set-add`, etc. Use it as the
spec for which op maps to which function and their arity/argument order.

== Tasks

+ *Dispatch in `em.expr`.* Recognise a call whose head is `alias/op` for an
  `alias` registered as a functor instantiation, and emit a call to the matching
  resource core fn:
  - `alias/new` → constructor fn → leaves an OWNED handle on the stack (boxed,
    see below).
  - `alias/add` / `alias/contains` / `alias/size` → method calls: the first
    argument is the set handle (a BORROWED `self`); unbox it to the `i32`
    handle, then pass the remaining flattened args.
+ *Handle as a Wavelet value.* A resource handle is carried inside Wavelet as an
  int box — `emit.rs:90`–`91`: "a single i32 handle from the host, carried in an
  int box" (TAG_INT layout, `emit.rs:47`). So `new` returns a boxed handle;
  methods take a boxed handle and unbox the `i32`. Confirm against the existing
  imported-resource handle path so the convention matches.
+ *Boundary lift/lower for `WitTy::Handle`.* An exported function returning
  `point-set.set` has result `own<set>` → `WitTy::Handle`. The export wrapper
  (`emit.rs:3854`/`3860`) lowers `FlatRes::One(Handle)` via `lower`, and lifts
  params via `lift_flat`. Confirm `lower(Handle)` unboxes the int box → `i32`
  and `lift_flat(Handle)` boxes `i32` → int box. If the *exported*-resource
  direction isn't already covered (imports may only have needed one direction),
  add it. `Handle` is already in the flat lists (`emit.rs:368`, `:393`,
  size/align at `:434`,`:486`,`:561`).
+ *Ownership correctness.* Methods receive a BORROWED handle (`self`); the
  function result and the constructor produce an OWNED handle. Match the
  synthesized WIT (`add: func(value: T)` is a method on borrowed self; the
  exported function's result is `own`). If the canonical ABI requires the guest
  to NOT drop a borrowed self but to transfer an owned result, ensure the bodies
  honour that (e.g. constructor's `resource.new` yields an owned handle the
  caller will eventually `resource.drop`).

== Isolated area

The call-dispatch site in `em.expr` (where qualified/`/`-headed calls are
lowered), plus `lower`/`lift_flat` for `WitTy::Handle`. Reuse the step-03
exported func indices (thread them through the emitter so `em.expr` can find
them by `alias`).

== Verify (the clincher for this step)

Build the worked example, instantiate via `wavelet::host::HostComponent`, and
call `nearest-set` with a `list<point>` containing duplicate points. Assert the
returned handle's `size` equals the deduped count — matching what the
interpreter (`wavelet run`) produces for the same input. A focused test (may still be
`#[ignore]`d; step 05 formalises the suite) is fine. Also sanity-check the
returned handle can be used (`contains` of a present vs absent point).

== Write `summaries/04-routing.typ` for step 05

- How `alias/op` is dispatched and where (`em.expr` site), and how the alias →
  `ResourceFns` lookup is threaded.
- The handle box convention (int box) and any `lower`/`lift_flat` changes made
  for `WitTy::Handle`.
- Confirmation the worked example runs correctly end-to-end (size matches the
  interpreter), with the exact input you tested.
- Any ownership/drop subtleties discovered.
