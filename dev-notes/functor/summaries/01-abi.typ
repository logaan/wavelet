// dev-notes/functor/summaries/01-abi.typ — hand-off from step 01 to step 02.
// The verified wit-component 0.251 ABI contract for a guest-IMPLEMENTED
// exported resource (`set`). Step 02 trusts this; do not re-investigate.

#set document(title: "Step 01 summary — resource-export ABI (verified)")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.62em)
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#set heading(numbering: none)
#show heading.where(level: 1): set text(size: 13pt)

= Step 01 summary — resource-export ABI (verified empirically)

All of this is *proven*, not assumed: the scratch test
`tests/functor_abi_spike.rs` (`#[ignore]`d) hand-authors the smallest core
module implementing a `set` resource, runs it through the *exact* pipeline
`emit_component` uses (`wit_component::embed_component_metadata` +
`ComponentEncoder::default().validate(true).module(&m).encode()`), then
instantiates it via `wavelet::host::HostComponent` and calls
ctor → size → add → contains → drop. It passes:
`cargo test --test functor_abi_spike -- --ignored`. The component
*validates*, *instantiates*, and methods *return* the expected values.

The reference world used (mirrors `wit::functor_interface` for element type
`s32`, the trivial flat case): package `demo:app@0.1.0`, interface `s32-set`
with `resource set { constructor(); add: func(value: s32); contains:
func(value: s32) -> bool; size: func() -> u32; }`, world `app { export
s32-set; }`. Resolved versioned interface: `demo:app/s32-set@0.1.0`.

== 1. Core EXPORTS the encoder requires (exact names)

The core module must export these funcs under these *exact* names (each
prefixed by the resolved `<versioned-iface>` = `demo:app/s32-set@0.1.0`,
formed by `wit::versioned_iface` exactly as ordinary exports are):

#table(
  columns: (auto, auto),
  [*core export name*], [*flat core signature*],
  [`<iface>#[constructor]set`],      [`() -> i32`  (returns OWN handle)],
  [`<iface>#[method]set.add`],       [`(i32, i32) -> ()`],
  [`<iface>#[method]set.contains`],  [`(i32, i32) -> i32`  (bool 0/1)],
  [`<iface>#[method]set.size`],      [`(i32) -> i32`  (u32)],
  [`<iface>#[dtor]set`],             [`(i32) -> ()`  (receives the REP)],
)

Spelling is literal: `[constructor]set`, `[method]set.add` (dot, not `#`),
`[dtor]set`. Plus the usual `memory` export. `cabi_realloc` is NOT required
for the all-`i32`/primitive case (the spike exports no realloc and validates);
it only becomes necessary if a method lifts/lowers a value needing allocation
(string/list element types — out of scope for s32, see §6).

== 2. Resource-intrinsic IMPORTS the encoder provides (exact triples)

The guest mints/inspects/drops handles through three intrinsics the encoder
synthesises. The core module *imports* them; the encoder wires them via a
shim/fixups table (`canon resource.new/rep/drop $set`). Exact triples
`(module, field, signature)` — module string is `[export]` + versioned iface:

#table(
  columns: (auto, auto, auto),
  [*module*], [*field*], [*signature*],
  [`[export]demo:app/s32-set@0.1.0`], [`[resource-new]set`],  [`(i32 rep) -> (i32 handle)`],
  [`[export]demo:app/s32-set@0.1.0`], [`[resource-rep]set`],  [`(i32 handle) -> (i32 rep)`],
  [`[export]demo:app/s32-set@0.1.0`], [`[resource-drop]set`], [`(i32 handle) -> ()`],
)

The module string is one constant per exported resource interface:
`format!("[export]{versioned_iface}")`. Field names are
`[resource-new]<res>`, `[resource-rep]<res>`, `[resource-drop]<res>`.

== 3. The handle/rep asymmetry — THE thing to get right

For a resource the guest *exports*, `own` and `borrow` are both a single
`i32` at the core boundary, but they carry DIFFERENT values:

- *Method receiver `self` (a `borrow<set>`) arrives as the REP directly.*
  The host/canonical layer already resolved the borrow to the rep before the
  call. So in `size`, returning `local 0` *is* returning the rep — the spike
  does exactly `(local.get 0)` and the host reads back the minted rep (42).
  Do NOT call `resource.rep` on `self`: `self` is not a handle, and
  `resource.rep(rep)` traps `unknown handle index`.

- *Constructor result (an `own<set>`) must be a HANDLE minted via
  `resource.new`.* The ctor body is `(call $resource.new (i32.const <rep>))`.
  Returning a bare rep instead traps at the boundary with
  `unknown handle index <rep>` — verified (the `SPIKE_RAW_CTOR=1` probe in the
  test). The encoder's `canon lift` of the `own` result expects a live handle.

Mnemonic for step 02: *methods receive reps; the constructor returns a handle.*
`resource.new : rep → handle` (ctor only). `resource.rep : handle → rep` is
NOT needed when the guest implements the resource and already gets reps in
methods — it is for the *opposite* direction (a guest holding an `own` handle
it must inspect). The spike uses `resource.rep` nowhere in the final bodies.

== 4. Dtor contract

- Name `<iface>#[dtor]set`, signature `(i32) -> ()`. The param is the *rep*
  (same value a method's `self` carries), NOT a handle. Verified: the spike's
  dtor traps (`unreachable`) unless its arg equals the minted rep 42, and a
  host-side drop completes cleanly — so the dtor fires on the last drop and is
  handed the rep.
- *It can be a no-op.* The emitter's bump allocator never frees, so the real
  dtor body may be just `(end)` — an empty function. Nothing in the ABI forces
  it to release memory. (The spike only adds the rep-check to *prove* the
  contract; a production dtor is `Function::new([])` + `End`.)
- The dtor runs when the LAST owning handle is dropped (host drop, or a guest
  drop via `[resource-drop]set`). `HostComponent` never auto-frees a dynamic
  `Val::Resource`; step 01 added `HostComponent::drop_resource(Val)` so a host
  can run a guest dtor explicitly (used by the spike; reusable in step 05).

== 5. wit-component 0.251 quirks / gotchas observed

- *Shim/fixups indirection.* The encoder does not import the intrinsics into
  the core module directly; it builds a `wit-component-shim`/`-fixup`
  instance and an `$imports` table, and routes the core module's three
  imports through it. This is transparent to the guest — you still just
  *declare the three imports* with the names in §2 and *call them by index*.
- *No `cabi_post_*` needed* for primitive/`i32` element types. The spike
  exports none and validates + runs. (`cabi_post_<iface>#...` return-cleanup
  callbacks only appear when a lifted result owns memory; not the s32 case.)
- *`memory` export is required* (the canonical ABI needs it even here);
  `cabi_realloc` is only required once a boundary value needs allocation.
- *Import module string must be exactly* `[export]<versioned-iface>` — the
  same versioned iface string as the exports, with an `[export]` prefix.
  The versioned iface is produced by `versioned_iface(pkg, iface)`
  (`emit.rs:965`) → `demo:app/s32-set@0.1.0`.
- *Ordering:* the spike declares imports first (indices 0,1,2), then the five
  funcs (3..7), and the export-section order is `memory` then the five named
  funcs. The encoder is order-insensitive on names but the usual
  type/import/func/code index discipline applies. No special import-vs-export
  ordering requirement surfaced.
- The bool result of `contains` is a plain `i32` (0/1) at core level; `u32`
  size is a plain `i32`. The encoder's `canon lift` does the i32→bool / i32→u32.

== 6. Element-type flattening (what changes when `T` ≠ s32)

The spike used `s32`, so `<flattened T>` is a single `i32` and `value` adds
exactly one core param. For other element types, `add`/`contains` gain the
flattened core params of `T` *after* the `self` i32, in canonical-ABI flat
order. Reuse the backend's existing `flat`/`flat_checked` machinery (the same
`emit.rs` helpers the export wrappers use, around `emit.rs:3855`) to compute
the flat param list for `value: T`. `string`/`list` elements will additionally
require `cabi_realloc` (and possibly post-return) at the boundary — but the
interpreter oracle keeps membership structural (`eq_raw`), so the rep design in
step 02 must store/compare boxed `Value`s, not flattened scalars. (Numeric
elements like `s32`/`s64`/`f64` stay flat and need no realloc.)

== 7. Minimal known-good recipe (translate into real bodies in step 02)

Per exported `set` resource at iface `I` (= `demo:app/<elem>-set@0.1.0`):

```
imports (module "[export]I"):
  fn[0] resource.new   : (i32 rep)    -> i32 handle   field "[resource-new]set"
  fn[1] resource.rep   : (i32 handle) -> i32 rep      field "[resource-rep]set"
  fn[2] resource.drop  : (i32 handle) -> ()           field "[resource-drop]set"
  (rep/drop are usually unused by the guest body; declare them anyway only if
   you call them — the spike declares all three, uses only `new`.)

exports:
  "I#[constructor]set"     () -> i32:
      rep = <alloc + init a fresh set rep box>     ; an i32 (boxed Value set)
      return  resource.new(rep)                    ; MINT the own handle
  "I#[method]set.add"      (i32 self_rep, <flat T> ) -> ():
      ; self_rep IS the rep; reinterpret as the set box, mutate in place
      <set-add semantics, matching builtins.rs eq_raw membership>
  "I#[method]set.contains" (i32 self_rep, <flat T> ) -> i32:
      <return 1 if present else 0, structural eq via eq_raw>
  "I#[method]set.size"     (i32 self_rep) -> i32:
      <return element count as u32-in-i32>
  "I#[dtor]set"            (i32 rep) -> ():
      ; safe no-op (bump allocator never frees)
      (empty body)
```

Key invariants step 02 must preserve: ctor returns `resource.new(rep)`;
every method's first i32 IS the rep (cast it back to the box, do not call
`resource.rep`); the dtor takes the rep and may be empty. Element equality for
`add`/`contains` MUST use the existing `eq_raw` core helper (structural), per
the agent rules — never `Derive Eq`.

== 8. Build state left behind

- `emit.rs::emit_component` is UNCHANGED — `wavelet build` still hits the
  honest early-return for functors (`emit.rs:719`). The wasm backend does not
  yet emit functor components; step 03 wires that in.
- New: `tests/functor_abi_spike.rs` (`#[ignore]`d; not in CI). Run with
  `cargo test --test functor_abi_spike -- --ignored [--nocapture]`. Env probes:
  `SPIKE_DUMP=1` writes `/tmp/spike_{embedded_core,component}.wasm` for
  `wasm-tools print`; `SPIKE_RAW_CTOR=1` reproduces the ctor-must-mint failure.
- New, additive: `HostComponent::drop_resource(Val) -> Result<(), String>`
  (`src/host.rs`) to run a guest dtor through the dynamic API. No existing
  behaviour changed; host unit tests still pass.
- Spike commit: `bba3543` (test + host helper). This summary: a follow-up
  `docs(functor)` commit on `worktree-functor-build`.
