// dev-notes/functor/summaries/02-bodies.typ — hand-off from step 02 to step 03.
// The emitted core `set` function bodies (rep layout, ABI, helper reuse) plus
// exactly how step 03 should wire them into `emit_component`. Trust this; the
// bodies are proven by `emit::set_resource_tests` (a `cargo test` unit test).

#set document(title: "Step 02 summary — set rep + core bodies (verified)")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.62em)
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#set heading(numbering: none)
#show heading.where(level: 1): set text(size: 13pt)

= Step 02 summary — the `set` rep and core function bodies (verified)

All of this is *proven* by the unit test `emit::set_resource_tests::
set_bodies_dedup_and_membership_s32` (in `src/emit.rs`). It drives the *real*
`emit_set_resource` bodies through the same `embed_component_metadata` +
`ComponentEncoder::default().validate(true)` pipeline `emit_component` uses,
instantiates via `wavelet::host::HostComponent`, and runs ctor → add (incl. a
duplicate) → size → contains → drop. It validates, dedups, and answers
membership exactly. Run with `cargo test --lib set_resource_tests`.

== 1. What was added (all in `src/emit.rs`, self-contained)

- `pub struct ResourceFns` — the deliverable handle for step 03.
- `fn emit_set_resource(em, inst, elem, new_import, rep_import, drop_import)
   -> Result<ResourceFns, String>` — appends the 5 bodies to `em.bodies`.
- three private body-builder helpers: `emit_empty_list_box`,
  `emit_list_contains`, `emit_list_append`.
- a `#[cfg(test)] mod set_resource_tests` driver/test.

`emit_component`'s early-return (`emit.rs:719`) and the export/import sections
are *untouched*. The new code is referenced only from the test, so a plain
`cargo build` reports it as dead code — that is the expected "not yet wired"
state; step 03 makes it live.

== 2. `ResourceFns` shape (what each field is)

```rust
pub struct ResourceFns {
    ctor: u32,        // [constructor]set  core sig () -> i32  (OWN handle)
    add: u32,         // [method]set.add   core sig (i32 self, <flat elem>) -> ()
    contains: u32,    // [method]set.contains (i32 self, <flat elem>) -> i32 (0/1)
    size: u32,        // [method]set.size  (i32 self) -> i32  (u32 count)
    dtor: u32,        // [dtor]set         (i32 rep) -> ()  (no-op)
    new_import: u32,  // import idx of [resource-new]set  (i32 rep) -> i32 handle
    rep_import: u32,  // import idx of [resource-rep]set  (UNUSED by bodies)
    drop_import: u32, // import idx of [resource-drop]set (UNUSED by bodies)
}
```

`ctor`/`add`/`contains`/`size`/`dtor` are *core function indices* (imports-first
index space). `new_import` is the only intrinsic the bodies call. `rep_import`
and `drop_import` are carried (not used by the bodies) so step 03 has every
index the encoder's intrinsic table needs in one place.

== 3. Rep layout + element-boxing convention (mirrors the interpreter)

- A `set` *rep* is a pointer to a one-word mutable CELL: `[i32 list-ptr]`
  (4 bytes, `alloc`ed by the ctor). The mutable cell is what gives the resource
  stable identity — `Value::Cell(Rc<RefCell<…>>)`. `add` overwrites the cell's
  word, so a later `size`/`contains` on the same handle sees the update.
  *Verified*: the test adds across multiple separate `call_instance`s and the
  growth persists.
- The cell's word points at the existing boxed-list layout
  `[TAG_LIST=3, len, elem-ptr…]` — tag i32 @0, `len` i32 @4, element box ptrs at
  `@8 + 4·i`. Same layout `list_box`/`seq_box` build.
- *Elements are stored as boxed values* (ordinary heap boxes). The incoming
  flattened `value` is boxed with `em.lift_flat(fx, elem, 1)` (the SAME helper
  the export wrappers use, `emit.rs:2779`) before it is stored/compared. This is
  what makes the bodies element-type-generic: record / string / primitive all
  become one box and compare structurally.

== 4. ABI invariants the bodies bake in (per summary 01 — and a correction)

- *Constructor mints the OWN handle*: `resource.new(cell_ptr)` (the
  `new_import`). Returning a bare rep traps.
- *Methods receive the REP directly in param 0.* `self` IS the cell ptr we
  passed to `resource.new`. The bodies use `local 0` verbatim and do `*self`
  (i32 load @0) to get the list ptr. *The brief's pseudocode said to call
  `resource.rep(self)` first — that is WRONG and would TRAP* ("unknown handle
  index"), exactly as summary 01 §3 warned. Summary 01 wins; the bodies call
  `resource.rep` NOWHERE. `rep_import` is declared only so the encoder's
  intrinsic table is complete.
- *`contains`/`size` return a bare core i32* (0/1 and the count). The encoder's
  `canon lift` does i32→bool / i32→u32, so the bodies do NOT call `lower` on the
  result — pushing the i32 is correct (host receives `Val::Bool`/`Val::U32`).
- *dtor is a no-op* `(i32 rep) -> ()` — an empty body (bump allocator never
  frees). Confirmed acceptable by summary 01 §4; the test drops the handle and
  the dtor runs cleanly.

== 5. Helpers reused + stack-discipline gotchas

- `em.h.alloc` (bump alloc), `em.h.eq_raw` (structural `Value` equality), and
  `em.lift_flat(fx, elem, base)` (flat → boxed value). Box tags / list layout
  from `emit.rs:46`–`56`.
- *`eq_raw` is `(box_a:i32, box_b:i32) -> i32` (0/1)* — push both box ptrs, call,
  branch on the i32 (`emit_list_contains` does this per element vs the needle).
- *`seq_box`/`list_box` take element *forms* (`NodeId`), NOT runtime stack
  values* (`emit.rs:1232`/`1238`). They are useless for building a list from
  runtime element pointers, so `emit_list_append` builds the new
  `[TAG_LIST,len,ptrs…]` box inline: `alloc(8 + 4·(n+1))`, copy the old `n`
  element ptrs, append the boxed `value`, write tag + new len. This is the only
  place the brief's "use `seq_box`" guidance does not literally apply — note it.
- *`lift_flat` allocates its own fresh `fx.local`s* for record/tuple/string
  element types; that is fine and composes with the locals the bodies allocate.
  Element value flats occupy param locals `1 .. 1 + flat_len(elem)`; `self` is
  local 0.
- `emit_list_contains` returns its i32 result via a `Block(Result(I32))` with
  inner `Loop`; the early "found"/"end" exits use `Br(2)` to that block. Keep the
  block/loop nesting if you ever refactor — the `Br` depths are load-bearing.

== 6. How step 03 calls `emit_set_resource` per `FunctorInst`

In `emit_component` (after deleting/loosening the early-return), for each
`inst` in `info.functors`:

+ *Get `elem: WitTy`*: `inst.elem` is the WIT type *text* (`"s32"`, `"point"`,
  `"string"`; field `FunctorInst.elem`, `wit.rs:83`). Map it through
  `wit_ty(&inst.elem, &em.type_env)?` — the same `TypeEnv` the rest of
  `emit_core_module` builds (records from this file's DefTypes + deps). For a
  record element the env must already hold the record's fields.
+ *Declare the three intrinsic imports BEFORE helper-index assignment.* Function
  index space is imports-first (`n_imports` then `take()`-ed helpers). Add, via
  the existing `add_import` closure, for module string
  `format!("[export]{}", versioned_iface(pkg, iface))` (here
  `[export]demo:app/s32-set@0.1.0`):
  - `[resource-new]set`  : `(i32) -> i32`
  - `[resource-rep]set`  : `(i32) -> i32`
  - `[resource-drop]set` : `(i32) -> ()`
  Capture their indices with `em.import_idx(module, field)`.
  (`versioned_iface` at `emit.rs:965`; `iface` is `inst.iface`, the specialized
  interface like `point-set`; `pkg` is `info.package`.)
+ *Call `emit_set_resource(&mut em, inst, &elem, new_i, rep_i, drop_i)?`* — at the
  same point the other bodies are pushed (after `emit_helpers`, alongside the
  internal/export bodies). It appends its 5 bodies to `em.bodies` and returns the
  indices.
+ *Export the 5 funcs* under the verified names (summary 01 §1), prefixed by the
  versioned specialized iface `versioned_iface(info.package, inst.iface)` =
  `demo:app/point-set@0.1.0`:
  - `<iface>#[constructor]set`  → `fns.ctor`
  - `<iface>#[method]set.add`   → `fns.add`
  - `<iface>#[method]set.contains` → `fns.contains`
  - `<iface>#[method]set.size`  → `fns.size`
  - `<iface>#[dtor]set`         → `fns.dtor`
  `cabi_realloc` is already exported by `emit_core_module`; for `string`/`list`
  element types it is *required* at the boundary (summary 01 §6) — it exists, so
  the missing piece there is only that the host writes the element bytes into
  guest memory before `lift_flat` reads them (canonical-ABI handles this for an
  exported method's incoming `value`, via the encoder's lowering glue). Numeric
  elements (`s32`/`u32`/`s64`/`f64`/`bool`/`char`) stay flat, no realloc concern.

The test's `build_core` is a faithful, minimal copy of this exact wiring
(imports up front → helper indices → `emit_helpers` → `emit_set_resource` →
assemble with the verified export names) — step 03 can read it as a worked
template.

== 7. The synthesized WIT must declare the resource intrinsics' world

The encoder only synthesises `[resource-new/rep/drop]set` imports when the world
*exports* the `set` resource. `synthesize_world_wit` already emits the
specialized `<elem>-set` interface for a functor (that is why `wavelet wit`
works); step 03's job is purely the core side. No WIT change is needed beyond
what synthesis already produces — the test reuses a hand-written equivalent WIT
and it matches.

== 8. Build state left behind

- `emit_component` UNCHANGED — `wavelet build` still hits the honest
  early-return (`emit.rs:719`) for any functor program. Pinned by the existing
  `tests/functor_runtime.rs::build_rejects_functor_programs_cleanly` (still
  green) and `tests/type_system.rs`.
- New, additive: `ResourceFns` + `emit_set_resource` + 3 builder helpers +
  `set_resource_tests` in `src/emit.rs`. Dead-code in a non-test build until
  step 03 wires it. Full `cargo test` is green (116 lib + all integration);
  the step-01 ABI spike stays `#[ignore]`d.
