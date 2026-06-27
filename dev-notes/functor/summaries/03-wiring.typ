// dev-notes/functor/summaries/03-wiring.typ — hand-off from step 03 to step 04.
// How the `set` resource is wired into the wasm *build* path: exports, intrinsic
// imports, type_env, WIT synthesis, and the exact stub step 04 must replace to
// route `alias/op` calls. Trust this; the build state is reproduced below.

#set document(title: "Step 03 summary — wire set resource into emit_component")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.62em)
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#set heading(numbering: none)
#show heading.where(level: 1): set text(size: 13pt)

= Step 03 summary — the build path now emits + validates the `set` resource

`wavelet build` no longer rejects functor programs. For each `Set`
instantiation it emits the five core funcs (step 02 bodies), exports them under
the canonical resource-ABI names, declares the matching resource intrinsics as
imports, and synthesizes a WIT world the encoder accepts. *Proven*: a functor
program that derives an ordinary result builds, `wasm-tools validate` passes,
and `wasm-tools component wit` shows the specialized `point-set` interface with
the `set` resource and its four ops — pinned by
`tests/functor_runtime.rs::build_emits_a_validating_set_resource`.

== 1. Where the resource funcs are EXPORTED (names + site)

In `emit_core_module` (`src/emit.rs`), *after* the export-wrapper loop, a loop
over `info.functors.iter().zip(&functor_intrinsics)` (`emit.rs:4021`) calls
`emit_set_resource` (`:4028`) and pushes five exports (`:4030`–`4034`). Names use
`versioned_iface(&info.package, &inst.iface)` = `demo:geo/point-set@0.1.0`:

```
{iface}#[constructor]set      -> fns.ctor
{iface}#[method]set.add       -> fns.add
{iface}#[method]set.contains  -> fns.contains
{iface}#[method]set.size      -> fns.size
{iface}#[dtor]set             -> fns.dtor
```

These land in the existing `exports: Vec<(String, u32)>`, which the assembly's
`ExportSection` loop (`emit.rs:~4060`) writes verbatim — same path as ordinary
exports. `memory` and `cabi_realloc` are exported by the existing assembly.

== 2. Where the intrinsics are IMPORTED (names + site)

In the imports-first index block, after the `dep_calls` import loop, a loop over
`info.functors` (`emit.rs:3784`–`3793`) declares three imports *per
instantiation* under module string `[export]{versioned specialized iface}` (here
`[export]demo:geo/point-set@0.1.0`), via the existing `add_import` closure:

```
[resource-new]set   (i32) -> i32      // ctor calls this (mint handle)
[resource-rep]set   (i32) -> i32      // declared, unused by bodies
[resource-drop]set  (i32) -> ()       // declared, unused by bodies
```

Their indices are captured into `functor_intrinsics: Vec<(new,rep,drop)>`
(`:3784`) and threaded to `emit_set_resource`. They land in the `ImportSection`
via the existing `em.imports` loop. The encoder only synthesizes these
intrinsics because the world *exports* the resource interface (see §4).

*Index discipline*: imports are declared up front so the function index space
stays imports-first. `emit_set_resource` self-assigns its five func indices from
`em.imports.len() + em.bodies.len()` (step 02, `emit.rs:4263`), the same
invariant `take()` tracks for export wrappers, so calling it after the export
loop is correct with no `take()` bookkeeping (we `drop(take)` first, `:4007`).

== 2b. *Reuse note*: `emit_set_resource` used UNCHANGED

`emit_set_resource` and `ResourceFns` (step 02) are reused verbatim — no edits.
The only addition near it is the per-inst caller loop above.

== 3. `type_env` changes (how `set` → Resource, how `elem` resolves)

In `emit_core_module`, after records/dep type-defs populate `type_env`, a loop
over `info.functors` (`emit.rs:3645`–`3665`) inserts *two* `TypeDef::Resource`
entries into `type_env.defs` per inst:

- `"set"` — the bare name as it appears in method sigs (`add: func(value: point)`
  receiver). `wit_ty` maps bare `set` / `own<set>` / `borrow<set>` →
  `WitTy::Handle` (`emit.rs:177`–`184`, `:260`).
- `"{iface}.set"` (e.g. `"point-set.set"`) — the dotted *return-type text* an
  export body gets for `alias/new()` (from `wit::functor_op_table`, which infers
  a `Handle` op to `format!("{}.set", iface)`). Without this, the export
  wrapper's `flat_result`/`wit_ty` reject `point-set.set` as an unknown type.

The element type resolves through the *same* `type_env`: a record element (e.g.
`point`) via `type_env.records` (already populated from this file's `DefType`s),
a primitive (`s32`/`string`/…) intrinsically. `WitTy::Handle` flattens to one
`i32`, lowers via `unbox_int`/`I32WrapI64`, lifts via `I64ExtendI32U`/`box_int`
(`emit.rs:2385`,`:2712`) — handles ride the existing scalar path.

== 4. World/WIT — what was ADDED (not just verified)

The brief said `synthesize_world_wit` "already" declares + exports the resource.
*It did not* — that is `wit::synthesize_info` (the `wavelet wit` path). The
*build* path uses `emit::synthesize_world_wit`, which had no functor handling.
Added there:

- Render each functor's specialized interface from the SAME source as
  `wavelet wit`: `wit::functor_interface` was made `pub(crate)` and is now called
  by both (`emit.rs:7622`, `wit.rs:850`) — the resource the backend implements
  and the WIT the encoder validates cannot drift.
- Export each `f.iface` from the world (`emit.rs:~7660`) — *required*, else the
  encoder never synthesizes the `[resource-new/rep/drop]set` intrinsics.

Two WIT-validity fixes (one source, fixes both `wavelet wit` and build):

- *`use api.{<elem>};`* in the specialized interface when the element is a local
  record (`wit::functor_interface`, `wit.rs:892`+, new `local_types` param). WIT
  cannot reference `point` inline across interfaces; it must be `use`-d. A
  primitive element needs no `use`.
- *`use <iface>.{set};`* in an interface whose export references a functor handle
  (`emit.rs:7600`), with the dotted `point-set.set` text rewritten to bare `set`
  (`:7609`). WIT forbids inline dotted type refs.

`select_world` + `embed_component_metadata` accept the result unchanged
(verified: the worked example componentizes + validates).

== 5. Build state — does the worked example BUILD + VALIDATE?

*Yes, for the resource itself.* This source:

```
Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Derive {Eq Ord Show} point
Import {pkg: "wavelet:coll/set" elem: point as: pts}
Export count-distinct
Def count-distinct Fn {ps: list(point)}
  Let {s: pts/new()} Do [ pts/add(s {x: 1 y: 2})  pts/size(s) ]
```

builds, `wasm-tools validate` passes, and `wasm-tools component wit` shows
`interface point-set { use api.{point}; resource set { constructor(); add: …;
contains: …; size: … } }`. All four ops present.

*The docs `nearest-set` shape does NOT build* — see §6. It is an honest error,
not a crash, and is the only part left for step 04 on the WIT side.

== 6. The two things step 04 must do

=== 6a. Route `alias/op` calls (the STUB to replace)

The qualified ops (`pts/new`, `pts/add`, `pts/contains`, `pts/size`) parse as
`Node::Qsym(alias, op)` and dispatch through `Emitter::call` →
`Emitter::dep_call` (`emit.rs:2272`). A functor alias is *not* a runtime import,
so the real `dep_call` path would fail `unknown import alias`. Step 03 added:

- A skip in the import-declaration loop for functor aliases (`emit.rs:3738`).
- A *STUB at the top of `dep_call`* (`emit.rs:2286`–`2295`): if `alias` names a
  `FunctorInst`, it evaluates+drops the args, emits `Unreachable`, then pushes a
  unit box (so the body type-checks and validates) — but *traps if reached*.
  Never a diverging value (the one hard rule).

*Step 04 replaces this whole `if let Some(inst) = …functors…` branch* with real
dispatch keyed on `(alias, op)`:
- `new`  → core `ResourceFns.ctor` (`call $ctor` → own handle i32, already a
  box-shaped i32? NO — it is a raw handle; box it as a `WitTy::Handle` via the
  `lift` path before returning, mirroring how `dep_call`'s `FlatRes::One` lifts).
- `add`/`contains` → lower each arg through the element `WitTy`, then call
  `ResourceFns.add`/`.contains`; the receiver `self` is the handle the caller
  holds. *Note the ABI asymmetry (summary 01 §3):* a method receives the REP,
  but the caller holds a handle minted by `resource.new`. Inside the *same*
  guest, the value the ctor returned IS what `add`/`contains` expect as `local 0`
  — step 02's bodies use `local 0` as the cell ptr directly, and the ctor returns
  `resource.new(cell)`. So intra-guest you must pass the *cell ptr* (rep), not the
  minted handle, to a method — i.e. step 04 should thread the rep, OR call
  `resource.rep` on the handle first. *This needs an ABI decision*; the spike
  (summary 01) only exercised the host→guest direction. Recommend: keep the rep
  (cell ptr) as the guest-side value of a `set`, mint a handle only at an export
  boundary (when returning `own<set>` to the host). That keeps methods callable
  with `local 0 = rep` and matches step 02's bodies with no `resource.rep` call.
- `size` → call `ResourceFns.size`, lift the i32 as `u32`.

=== 6b. ORDERING constraint (important)

`ResourceFns` (the ctor/add/… *func indices*) are produced by
`emit_set_resource`, which step 03 calls *after* the internal/export bodies are
emitted (`emit.rs:4021`). But `dep_call` runs *while emitting those bodies* —
i.e. *before* the resource funcs exist. So step 04 cannot just "look up the
indices" in `dep_call` as written. Options:

1. *Pre-assign* the five func indices per inst up front (they are deterministic:
   `n_imports + bodies_before + k`), store them in a new `Emitter` field
   (`HashMap<alias, ResourceFns>`), then emit the bodies later at those indices.
   `emit_set_resource` already self-indexes from `em.bodies.len()`, so the
   simplest is to *emit the resource bodies BEFORE the internal/export bodies*
   and record the `ResourceFns` in the `Emitter` for `dep_call` to read. Moving
   the `emit_set_resource` loop above the internal-body loop is the cleanest
   change; the export *registration* (pushing names into `exports`) can stay
   where it is or move with it — order of `exports` entries is name-keyed, not
   index-sensitive.
2. Keep current order and reserve indices analytically. More fragile.

Recommend option 1: hoist the `emit_set_resource` call loop to just after
`emit_helpers` (mirroring the step-02 test's order: helpers → set bodies), stash
`ResourceFns` per alias in the `Emitter`, then have `dep_call` route to them.

== 7. The WIT interface-cycle limit (the docs `nearest-set`)

An export that *returns* a `set` handle whose element is a *local record* makes
`api` `use point-set.{set}` while `point-set` `use api.{point}` — a mutual
dependency WIT forbids. Step 03 detects this and errors honestly
(`emit.rs:7556`+; message contains `cycle` + the export name), pinned by
`build_rejects_handle_returning_export_over_local_record`. This is the docs
`nearest-set: func(..) -> point-set.set` shape. *An export deriving an ordinary
result (`u32`/`bool`) from the set has no cycle and builds.* Lifting the limit is
follow-up: hoist the element record into the functor's interface (or a shared
types interface) so the dependency is one-directional. Not required for step 04's
*routing* work, but step 04/05 should know `nearest-set` cannot be built as-is.

== 8. Build state left behind / test state

- `emit_set_resource` + `ResourceFns` (step 02) reused UNCHANGED.
- New/changed: `emit_component` early-return removed; `emit_core_module`
  type_env + intrinsic-import + resource-emit wiring; `dep_call` functor stub;
  `emit::synthesize_world_wit` functor interfaces/exports/`use`s + cycle error;
  `wit::functor_interface` made `pub(crate)` + `use api.{elem}` for record
  elements (one new param).
- `cargo test` GREEN: 116 lib + all integration suites pass; the step-01 ABI
  spike stays `#[ignore]`d. The step-02 unit test
  (`set_resource_tests`) still passes. Functor build behaviour pinned by the two
  new tests in `tests/functor_runtime.rs`.
- Commits on `worktree-functor-build` (pushed to PR #24):
  `5aaaa66` (wiring scaffolding, inert behind guard),
  `19f67aa` (guard removal + stub + WIT fixes — the live build path),
  `ee859d6` (test contract update). This summary follows as a `docs(functor)`
  commit.
