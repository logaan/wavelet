// dev-notes/functor/summaries/04-routing.typ — hand-off from step 04 to step 05.
// How qualified `alias/op` functor calls are routed to the emitted `set` resource
// core funcs, the OWN-handle carriage convention, and the DISCOVERED eq_raw
// structural-equality divergence (which corrects summaries 01/02). Trust this:
// the routing is committed and proven by executing tests, not just validation.

#set document(title: "Step 04 summary — route alias/op to the set resource")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.62em)
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#set heading(numbering: none)
#show heading.where(level: 1): set text(size: 13pt)

= Step 04 summary — `alias/op` calls now RUN, not just validate

`wavelet build` now *routes* the qualified functor ops (`pts/new`, `pts/add`,
`pts/contains`, `pts/size`) to the locally-emitted `set` resource core funcs and
executes correctly in `wasmtime`. The step-03 stub (eval-args + `Unreachable` +
unit box) is gone. *Proven by executing the built component*, not just
validating it: `tests/backend_functor.rs` builds two functor programs through
the real `wavelet build` path and runs them in-process, asserting the backend
agrees with the interpreter (the semantics oracle).

== 1. How `alias/op` is dispatched (`dep_call`)

A qualified op parses as `Node::Qsym(alias, op)` and dispatches through
`Emitter::call` → `Emitter::dep_call` (`src/emit.rs:2272`). The functor branch is
the first thing `dep_call` checks (`:2296`): if `alias` names one of
`self.info.functors`, it looks up the alias's `ResourceFns` in
`self.functor_fns` (`:2297`) and routes by op name, mirroring the interpreter's
`bind_functor`:

- `new` (`:2301`) → `Call(fns.ctor)` (`() -> i32` own handle), then `self.lift(fx,
  &WitTy::Handle)` boxes the handle as the set value (see §3).
- `add` / `contains` / `size` (`:2308`) → arg 0 is the set value: lower it,
  unbox the int, `I32WrapI64`, then `Call(fns.rep_import)` (the `[resource-rep]set`
  intrinsic) to recover the method's `self` *rep*, stash it in a local, and push
  it first. For `add`/`contains`, the element value is then lowered through the
  element `WitTy` (`wit_ty(&inst.elem, &self.type_env)`, `:2327`) in canonical-ABI
  order. Then `Call(fns.add|contains|size)` and lift the result: `add` → unit box
  (`unit_addr`), `contains` → `WitTy::Bool`, `size` → `WitTy::IntU` (`u32`).

Unknown ops and bad arity return honest errors (`:2346`, `:2302`/`:2310`), never
a diverging value. After the functor branch returns, the rest of `dep_call`
(`:2355`+) is the unchanged ordinary-import path.

== 2. The alias→`ResourceFns` lookup: reservation + hoist

`dep_call` runs *while lowering the internal/export bodies* — i.e. before the
resource bodies exist in `em.bodies`. Step 03 flagged this ordering hazard.
Step 04 resolves it exactly as summary 03 §6b recommended (option 1):

- *Reserve* the five resource func indices per instantiation UP FRONT, in the
  `take()` sequence, right after the helper-index reservations
  (`emit.rs:3885`–`3915`). Each inst's `(ctor, add, contains, size, dtor)` plus
  the three intrinsic import indices `(new_import, rep_import, drop_import)` are
  recorded in a new `Emitter` field `functor_fns: HashMap<String, ResourceFns>`
  (`:1096`, init `:3768`). Reserving here shifts the internal/overload/export
  `take()`s that follow past the resource slots, keeping the index space
  consistent.
- *Hoist* the actual body emission (`emit_set_resource`) to *just after*
  `emit_helpers` and *before* the internal/export bodies (`emit.rs:3978`–`4000`).
  `emit_set_resource` self-indexes from `em.imports.len() + em.bodies.len()`
  (step 02), so emitting it here makes the five `em.bodies` positions line up
  with the reserved indices. A `debug_assert_eq!` (`:3995`) pins
  `em.functor_fns[alias] == emit_set_resource(...)` — reserved indices must equal
  emitted positions.
- The canonical export *names* (`{iface}#[constructor]set`, …) are still
  registered later, alongside the export wrappers (`emit.rs:4125`+), reading
  indices back out of `em.functor_fns`. Name-keyed `exports` entries are not
  index-order-sensitive, so registration site is free.

So `dep_call` reads stable, pre-reserved indices out of `em.functor_fns` while
the bodies that contain the calls are still being lowered. `emit_set_resource`
and `ResourceFns` (step 02) are reused UNCHANGED.

== 3. The handle convention (carriage + lift/lower unchanged)

*A `set` Wavelet value carries the OWN handle minted by the constructor's
`resource.new`, boxed as an int box* (`TAG_INT`) — the same opaque-handle
carriage `lower`/`lift` already use for `WitTy::Handle` (summary 03 §3: Handle
flattens to one `i32` and rides the int-box scalar path). Consequences:

- *`lift`/`lower` are UNCHANGED.* `new` lifts the ctor's i32 via the existing
  `WitTy::Handle` path; the export boundary (`nearest-set -> own<set>`) just
  unboxes that i32 — no functor special-casing at the boundary.
- *Methods receive the REP, not the handle.* The canonical resource ABI hands an
  exported method its rep directly (summary 01 §3), but the caller holds the
  minted handle. So intra-guest, `add`/`contains`/`size` convert handle→rep with
  the `[resource-rep]set` intrinsic (`fns.rep_import`) before the method call
  (`emit.rs:2322`). This is the alternative to "carry the rep guest-side, mint at
  the boundary": it keeps `lower`/`lift` untouched and reuses the ctor verbatim,
  at the cost of one `resource.rep` call per method.

== 4. DISCOVERED DIVERGENCE — `eq_raw` was not structural for compounds

*This corrects summaries 01/02's claim that "`eq_raw` is already the
interpreter's equality".* It was only true for primitives.

*What it was.* `eq_raw` (the set-membership equality, `emit.rs:6652`) compared
content for `bool`/`int`/`dec`/`str`, then FELL BACK TO POINTER IDENTITY
(`local.get 0; local.get 1; i32.eq`) for *everything else* — including `TAG_REC`,
`TAG_LIST`, `TAG_TUP`, `TAG_VAR`, `TAG_FLG`, `TAG_CHAR`.

*Why it diverged.* The interpreter compares all of those STRUCTURALLY (`impl
PartialEq for Value`, `src/value.rs:69`–`87`: Rec/Lst/Tup/Variant/Flg/Char are
structural; only Closure/Macro/Cell use `Rc::ptr_eq`). Two separately-built
`point(1,2)` boxes are distinct pointers, so `eq_raw` called them unequal: a
routed `pts/size` over `{(1,2),(3,4),(1,2)}` returned 3 where the interpreter
dedups to 2. Per `CLAUDE.md` (interpreter is the oracle), a backend that diverges
is a bug — *fixed the backend*.

*What changed.* Made `eq_raw` mirror `Value::PartialEq` as a single recursive
core (it calls itself via the already-reserved `em.h.eq_raw`):
- `char` (`TAG_CHAR=10`): i64 scalar @8 (TAG_INT layout).
- `record` (`TAG_REC=6`): `n` @4 must match, then each `(key strbox @8+8i,
  value box @12+8i)` pair recursed positionally — order-sensitive, matching the
  `Vec` compare.
- `list`/`tuple`/`flags` (`TAG_LIST=3`/`TAG_TUP=8`/`TAG_FLG=9`): `len` @4 then
  element boxes @8+4i, recursed in order. Flags share the layout — a flags box
  stores its name str boxes @8+4i, so structural recursion matches the
  interpreter's `Flg(Vec<String>)` equality too (no gap).
- `variant` (`TAG_VAR=7`): case-name strbox @4 recursed; then payload box @8 —
  both absent (0) ⇒ equal, exactly one absent ⇒ unequal, else recurse on the two
  payloads. Mirrors `Variant(a,p) == Variant(b,q) => a == b && p == q`.
- *closures* (`TAG_FN=5`) keep pointer identity — matching `Rc::ptr_eq` for
  `Closure`/`Macro`. This is the only remaining identity fallback.

*Side effect (intended).* `eq_raw` also backs the `eq` builtin, so `eq` is now
structural for records/lists too — which is what the interpreter already does.
No test encoded the old identity behaviour, so nothing regressed; the full suite
is green (§6).

== 5. The tests (inputs + expected, oracle-sourced)

`tests/backend_functor.rs` (2 tests, both green). Expected values come from the
interpreter (`wavelet run` / the `functor_runtime.rs` oracle tests).

- `routed_record_set_size_matches_interpreter` — intra-guest routing over a
  *record* element, deriving a `u32`. `count-distinct` builds a `point` set, adds
  `{x:1 y:2}`, `{x:3 y:4}`, `{x:1 y:2}` (one a structural duplicate), returns
  `pts/size(s)`. *Expected `U32(2)`* (the third add dedups). This is the test the
  eq_raw fix turned green. Oracle:
  `functor_runtime::element_equality_matches_eq_for_records`.
- `returned_handle_methods_match_interpreter` — the handle-return boundary plus
  host-side methods over a *primitive* (`s32`) element (no local-record WIT
  cycle). `build-ints` adds `1`, `2`, `1` and *returns* `own<set>`; the host then
  calls `size` (*expected `U32(2)`*, deduped) and `contains` (`1`→true, `2`→true,
  `9`→false), then drops the handle (no-op dtor runs cleanly). Oracle:
  `functor_runtime::add_is_observed_on_the_same_handle_and_dedups`.

== 6. Build/test state left behind

- `dep_call` functor branch (`emit.rs:2296`–`2354`) replaces the step-03 stub;
  index reservation (`:3885`) + body hoist (`:3978`) + `functor_fns` field;
  `eq_raw` made structural (`:6652`). `emit_set_resource`/`ResourceFns` UNCHANGED.
- Refreshed the now-stale comment on
  `functor_runtime::build_emits_a_validating_set_resource` (it claimed the
  `pts/...` calls were "not yet routed (step 04)" / "bodies trap if reached").
- `cargo test` GREEN twice back-to-back: all binaries pass, 1 `#[ignore]`d (the
  step-01 ABI spike), 0 failures. The step-02 `set_resource_tests` and the
  step-03 `functor_runtime` contracts still pass.

== 7. For step 05 — proven shapes vs the local-record handle-return limit

*PROVEN end-to-end (built + executed, backend == interpreter):*
- *Primitive element* (`s32`): full lifecycle incl. *handle RETURN* (`own<set>`
  out of an export) and host-side `size`/`contains`/drop on the returned
  resource. (`returned_handle_methods_match_interpreter`.)
- *Record element* (`point`): intra-guest `new`/`add`/`size` with *structural
  dedup* deriving a `u32`. (`routed_record_set_size_matches_interpreter`.)

The `eq_raw` fix makes record/list/tuple/variant/flags/char dedup correct in
general; step 05 should add parity tests for list/tuple/variant element shapes to
formalise the suite.

*LIMITED (unchanged from step 03 — a WIT, not a routing, limit):* an export that
*returns* a handle whose element is a *LOCAL RECORD* hits the `api ↔ point-set`
WIT interface cycle (`api` `use`s the handle while `point-set` `use`s the
record). The backend rejects it honestly, pinned by
`functor_runtime::build_rejects_handle_returning_export_over_local_record`. The
primitive-element handle return has no cycle and works (above). Lifting this
needs hoisting the element record into a shared types interface so the
dependency is one-directional — follow-up, not routing.
