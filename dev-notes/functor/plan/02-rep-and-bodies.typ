// dev-notes/functor/plan/02-rep-and-bodies.typ — Step 02: set rep + core bodies.

#set document(title: "Step 02 — set rep + core function bodies")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.62em)
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#set heading(numbering: none)
#show heading.where(level: 1): set text(size: 13pt)

= Step 02 — the `set` representation and core-function bodies

*First read `plan/00-agent-rules.typ`*, then `summaries/01-abi.typ` (the ABI
surface — trust it). Critical rules: branch `worktree-functor-build` via
`EnterWorktree path=.claude/worktrees/functor-build`; commit as you go with the
two trailers; no PR; the interpreter is the oracle; set membership uses `eq_raw`,
not `Derive Eq`.

== Goal

Emit the core wasm functions that implement `set`, expressed with helpers
`emit.rs` already has. Do NOT yet wire them into `emit_component`'s export path
or remove the early-return — that is step 03. Here you only *produce* the
functions (and prove them with the step-01 spike harness).

== The representation (mirror the interpreter exactly)

The interpreter backs a `set` with `Value::Cell(Rc<RefCell<Value::Lst>>)` — a
mutable cell holding a list, with dedup-on-add by `Value` equality. Mirror it in
linear memory:

- A `set` *rep* is a pointer to a one-word mutable CELL holding a boxed-list
  pointer. (The mutable cell gives the resource stable identity, so a later
  `contains`/`size` observes earlier `add`s — exactly the `RefCell` semantics.)
- The list is the existing boxed-list layout `[TAG_LIST, len, elem-ptr...]`
  (`TAG_LIST = 3` at `emit.rs:49`; built by `list_box`/`seq_box` at
  `emit.rs:1210`–`1217`; `len` is the i32 word at offset 4).
- Elements are stored as boxed values (the same heap boxes the rest of the
  backend uses), so `eq_raw` and `list_box` operate uniformly.

== Reuse these helpers (cited so you needn't search)

- Allocator: `em.h.alloc` / `em.h.realloc` (fields at `emit.rs:678`–`679`, set
  around `emit.rs:3711`). `alloc` bumps; nothing is ever freed.
- Structural equality: `em.h.eq_raw` (field `emit.rs:687`, set `emit.rs:3720`) —
  a core fn comparing two boxed values; this IS the interpreter's `Value`
  equality. Used all over `Match` (e.g. `emit.rs:2479`).
- Boxed sequences: `list_box`/`seq_box` (`emit.rs:1210`–`1217`).
- Box tags: `emit.rs:46`–`56` (TAG_INT=1, TAG_STR=2, TAG_LIST=3, TAG_REC=6, …).
- Value <-> memory at the canonical layout: `store_to_mem` (around `emit.rs:2898`)
  and the load inverse (around `emit.rs:3085`); flattening helpers `flat`,
  `flat_checked`, `lift_flat`, `lower` (used in the export loop `emit.rs:3834`–
  `3897`). The incoming `value` arg arrives flattened per its `WitTy`; box it
  into a heap box before storing/comparing.

== Bodies to emit

For one instantiation at element `WitTy` `elem`:

- *constructor* `() -> i32 handle`: `alloc` a 1-word cell; build an empty list
  box; store the list-box ptr into the cell; call the `resource.new` intrinsic
  (import index from summary 01) on the cell ptr → handle; return handle.
- *add(self, value) -> ()*: `resource.rep(self)` → cell ptr; load list-box ptr;
  box `value`; linear-scan the list comparing each element to the boxed value
  with `eq_raw` — if present, return; else build a NEW list box = old elements +
  boxed value (`seq_box`) and store its ptr back into the cell. Return unit.
- *contains(self, value) -> bool*: `resource.rep` → cell → list; box `value`;
  scan with `eq_raw`; return 0/1 (lower as bool per boundary).
- *size(self) -> u32*: `resource.rep` → cell → list; read the `len` word at
  offset 4; return as u32.
- *dtor(rep) -> ()*: no-op (bump allocator never frees). Confirm against summary
  01 that a no-op dtor is acceptable.

Make the element type generic: the body must handle `elem` being a record
(`point`), a string, or a primitive. Box the flattened incoming value uniformly,
and compare with `eq_raw` (which is type-generic). This is what makes "full
parity for any element type" cheap.

== Isolated area

Add a NEW function/section to `emit.rs`, e.g.:

```rust
struct ResourceFns {
    ctor: u32, add: u32, contains: u32, size: u32, dtor: u32,
    // intrinsic import indices used by the bodies:
    new_import: u32, rep_import: u32, drop_import: u32,
}
fn emit_set_resource(em: &mut Emitter, inst: &FunctorInst, elem: &WitTy)
    -> Result<ResourceFns, String> { ... }
```

Do NOT touch `emit_component`'s early-return or the export/import sections yet
(step 03). Keep this self-contained.

== Verify

Drive these real bodies from the step-01 spike harness (or a `#[ignore]`d test):
construct a set, `add` a few elements including a duplicate, assert `size`
dedups and `contains` is correct. This need not go through the full
`emit_component` path yet if the spike harness can call the bodies directly;
otherwise a minimal hand-wired module is fine.

== Write `summaries/02-bodies.typ` for step 03

- The `ResourceFns` shape and what each field is.
- The exact rep layout (cell → list box) and the element-boxing convention.
- Which helpers were used and any signature gotchas (e.g. arg/stack discipline
  for `eq_raw`, `seq_box`).
- How step 03 should call `emit_set_resource` per `FunctorInst`: where to get
  `elem: WitTy` for an instantiation (map the instantiation's element type
  through `wit_ty`/`TypeEnv`).
- The build state you leave behind (expected: not yet wired, so `wavelet build`
  still hits the early-return — note that).
