// dev-notes/functor/summaries/05-parity.typ — hand-off from step 05 to step 06.
// The backend-vs-interpreter parity suite for the `Set` functor build path: the
// element-shape and multiplicity cases now covered end-to-end, where the expected
// values come from (interpreter structural dedup), the worked-example substitution
// (the local-record handle-return WIT cycle), and the green-suite confirmation.
// No code changed in this step — tests only; the backend (commit 0444f6a) stands.

#set document(title: "Step 05 summary — backend/interpreter parity suite")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.62em)
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#set heading(numbering: none)
#show heading.where(level: 1): set text(size: 13pt)

= Step 05 summary — the functor parity suite is complete and green

Step 04 routed `alias/op` calls and proved two shapes by execution. Step 05
*formalises the suite*: `tests/backend_functor.rs` now builds six functor
programs through the real `wavelet build` path and runs them in-process,
asserting the wasm backend agrees with the interpreter (the semantics oracle per
`CLAUDE.md`) on every case group the brief calls for. *No backend code changed in
this step* — these are tests only; the backend support landed in commit
`0444f6a` (compound functor element types + multiple instantiations per world).
The step-01 ABI scratch spike (`tests/functor_abi_spike.rs`, the suite's only
`#[ignore]`d test) was deleted; its findings live in `summaries/01-abi.typ`.

== 1. Cases covered (six tests, all green)

The four new tests plus the two from step 04 span the brief's case groups —
worked-example/compound, multiple instantiations, a primitive element, and edges:

- *Record element* (`routed_record_set_size_matches_interpreter`, step 04) —
  intra-guest `new`/`add`/`size` over a derived `point` record, deriving `u32`;
  structural dedup ⇒ `2`.
- *Primitive element + handle return* (`returned_handle_methods_match_interpreter`,
  step 04) — `s32` element, export *returns* `own<set>`; host-side `size` (`2`,
  deduped) and `contains` (`1`/`2`→true, `9`→false), then drop.
- *Compound (list) element* (`compound_list_element_dedups_like_interpreter`,
  new) — element `list(s32)`, deriving `u32`. Adds `[1 2]`, `[3 4]`, `[1 2]`;
  the structural `eq_raw` dedups the duplicate *list* (order-sensitively) ⇒ `2`.
  This exercises the new compound-element backend path.
- *String element + handle return* (`string_element_handle_methods_match_interpreter`,
  new) — non-primitive scalar element returning the handle. Adds "hi","yo","hi";
  host-side `size` is `2`, `contains` is exact ("hi"→true, "nope"→false), drop.
- *Two instantiations in one world* (`two_instantiations_in_one_world_match_interpreter`,
  new) — `s32` and `string` `Set`s in ONE world, both returning handles, built
  once and called on the same component. This exercises the `set as <iface>-handle`
  aliasing fix: the two resources land in distinct interfaces (`s32-set`,
  `string-set`) without colliding. `build-ints` adds `1` twice ⇒ `1` (the
  *same-element-twice* edge); `build-words` adds "a","b","a" ⇒ `2`.
- *Empty set* (`empty_set_size_is_zero`, new) — `new()` then `size` with no adds
  ⇒ `0` (the *empty* edge).

Together: compound coverage (T1 list + the record test), multiplicity (T3),
primitives (the s32 test + T3/T4), and edges (T3 same-twice, T4 empty, T2/the
s32 test `contains` true *and* false).

== 2. Where the expected values come from (interpreter structural dedup)

Every expected number is the interpreter's answer for the same program, not an
independently recomputed one. The interpreter dedups *structurally*: lists,
strings, tuples and records compare by value (`impl PartialEq for Value`,
`src/value.rs`), with lists/tuples order-sensitive. Step 04 made the backend's
set-membership equality (`eq_raw`) mirror that `PartialEq` recursively (see
summary 04 §4), so the backend now dedups the same way. Hence:

- `[1 2]` twice ⇒ one element (T1, list by value); "hi"/"a" repeated ⇒ one
  element (T2/T3, string by value); `1` twice ⇒ one element (T3, primitive); a
  fresh set ⇒ zero (T4). All match `wavelet run` / the `functor_runtime.rs`
  oracle tests.

== 3. The worked-example substitution (a WIT cycle, not a routing gap)

The brief's literal "worked example" returns a handle over `list<point>` — a
*local record* inside a list. That hits the known WIT *interface-cycle*
limitation: `api` `use`s the set handle while the element interface `use`s the
local record, so the dependency is not one-directional. It is a WIT shaping
limit, not a routing bug, and is already pinned by
`functor_runtime::build_rejects_handle_returning_export_over_local_record` (and
described in summary 04 §7). We therefore *substitute* the compound coverage with
T1 (`list(s32)` element, `u32` result, no handle return), which exercises the
compound-element backend path without crossing the cycle. Lifting the limit needs
hoisting the element record into a shared types interface (a follow-up, not part
of this functor build).

== 4. Suite state — green across repeated runs

`cargo test` was run back-to-back *twice* to guard against temp-dir flakiness
(the per-build dir is keyed on `(pid, seq)`; see `build_component`). Both runs
were identical and fully green: *219 tests passed, 0 failed, 0 ignored*, 0
filtered — across all binaries, including the new six in `backend_functor.rs`.
No `#[ignore]`d tests remain (the spike is gone). *No divergence* between backend
and interpreter was observed on any shape.

== 5. For step 06 (docs / CHANGELOG)

The functor build path is now proven end-to-end for:

- *Multiple instantiations in one world* — newly supported (commit `0444f6a`,
  `set as <iface>-handle` aliasing). Two distinct-element `Set`s coexist and both
  return handles; worth a CHANGELOG line and a user-facing note.
- *Compound element types* — a `list` element is proven end-to-end (T1);
  `eq_raw` is structural for list/tuple/variant/flags/record/char, so other
  compound shapes dedup correctly by the same recursion.
- *Scalar elements* — both a primitive (`s32`) and a non-primitive scalar
  (`string`) are proven end-to-end, including handle return + host-side
  `size`/`contains`/drop.

Remaining documented limit to carry forward: a handle return whose element is a
*local record* (the `list<point>` worked example) still hits the WIT interface
cycle and is rejected honestly — note it as a known limitation with the
shared-types-interface follow-up.
