// dev-notes/functor/plan/01-abi-spike.typ — Step 01: resource-export ABI spike.

#set document(title: "Step 01 — resource-export ABI spike")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.62em)
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#set heading(numbering: none)
#show heading.where(level: 1): set text(size: 13pt)

= Step 01 — resource-export ABI spike

*First read `plan/00-agent-rules.typ` in full and follow it.* Critical points:
work on branch `worktree-functor-build` via
`EnterWorktree path=.claude/worktrees/functor-build`; commit as you go with the
two required trailers; do NOT open a PR; the interpreter is the oracle.

== Why this step exists

Everything else in this plan is mechanical *once one thing is known*: the exact
contract `wit-component` 0.251 imposes on a core wasm module that *implements an
exported resource*. `emit.rs` today only consumes *imported* host resources
(opaque `i32` handles it never inspects). Emitting a resource the guest itself
implements is new ground. This step nails that contract empirically so steps
02–04 can be written without guesswork. Produce production code only if trivial;
the deliverable is knowledge + a proof.

== Background you need (read these, nothing more)

- `emit.rs` `emit_component` (around `emit.rs:703`): the pipeline is
  hand-built core `Module` → `wit_component::embed_component_metadata` →
  `wit_component::ComponentEncoder::default().validate(true).module(&module).encode()`.
  Exports are core funcs named `"<versioned-iface>#<func>"` (see the export loop
  around `emit.rs:3827` and the `ExportSection` around `emit.rs:3988`). The
  encoder synthesises the canonical adapters around this core module.
- `wit.rs` functor synthesis: `SET_OPS` (around `wit.rs:108`), `functor_interface`,
  `parse_functor`, and `struct FunctorInst` (`kind`, `alias`, `iface`, around
  `wit.rs:77`). This is the WIT the world already contains for `set` — a
  `resource set { constructor(); add: func(value: T); contains: func(value: T)
  -> bool; size: func() -> u32; }` inside interface `<elem>-set`.
- `tests/backend_numeric.rs` — the pattern for building a component through the
  real emitter and then *executing* it in-process via
  `wavelet::host::HostComponent` (an empty, capability-free wasmtime host).
- Dep versions: `wit-component = 0.251`, `wasm-encoder = 0.251` (`Cargo.toml`).

== The task

Determine, for an interface the guest EXPORTS that declares the `set` resource
above, the exact ABI the encoder requires of the core module. Specifically:

+ *Core EXPORTS the encoder expects.* Confirm the canonical names. The expected
  shape (VERIFY against 0.251 — do not assume):
  - `"<versioned-iface>#[constructor]set"`
  - `"<versioned-iface>#[method]set.add"`
  - `"<versioned-iface>#[method]set.contains"`
  - `"<versioned-iface>#[method]set.size"`
  - a destructor: `"<versioned-iface>#[dtor]set"` (confirm exact spelling).
+ *Core IMPORTS the encoder provides* for the resource intrinsics, and under
  what module/field names and signatures: `resource.new`, `resource.rep`,
  `resource.drop` specialised to `set`. (These let the guest mint a handle from
  a rep `i32`, recover the rep `i32` from a handle, and drop a handle.) Record
  the exact import module string, field names, and each signature.
+ *Flat signatures at the core boundary* (VERIFY): constructor `() -> i32`
  (owned handle); method receiver `self` is a BORROWED handle passed as the
  first `i32` param; `add: (i32 self, <flattened T...>) -> ()`;
  `contains: (i32 self, <flattened T...>) -> i32` (bool); `size: (i32 self)
  -> i32` (u32); dtor: `(i32 rep) -> ()`.
+ *Handle representation* at the component boundary: own and borrow are both a
  single `i32` at the core level. Record which ops take a borrowed self and
  which return/produce an owned handle.

== Prove it (the smallest possible experiment)

Hand-author the SMALLEST core module that implements a trivial `set` and run it
through the *same* pipeline `emit_component` uses, then instantiate and call it:

- Backing rep can be trivial (e.g. an `i32` counter): `constructor` mints a
  handle over a fresh rep; `add` is a no-op; `size` returns a constant or the
  counter; `contains` returns `false`. Correctness of the set is NOT the point
  here — *the encoder accepting and wiring it* is.
- Build the core module with `wasm-encoder`, OR write `.wat` and compile with
  `wat::parse_str`, whichever is faster to iterate. Either is fine for a spike.
- Wrap it with `wit_component::embed_component_metadata` + `ComponentEncoder`
  `.validate(true)` exactly as `emit_component` does, against a tiny WIT world
  that exports an `xs` interface with the `set` resource.
- Instantiate the result via `wavelet::host::HostComponent::from_bytes` and call
  the constructor + `size`, asserting it returns. Mirror `backend_numeric.rs`.

You MAY leave the experiment as a `#[ignore]`d scratch test under `tests/`
(clearly named, e.g. `tests/functor_abi_spike.rs`) so it doesn't run in CI, or
keep it under `$CLAUDE_JOB_DIR/tmp`. Step 05 replaces it with real parity tests,
so a left-behind `#[ignore]`d spike is acceptable but not required.

== Definition of done

- A scratch component with an exported, guest-implemented resource builds,
  validates (`validate(true)` passes), instantiates in wasmtime, and a method
  call returns.
- `summaries/01-abi.typ` records the verified ABI surface.

== Write `summaries/01-abi.typ` for step 02

Include, concretely:
- The EXACT core export names for ctor/add/contains/size/dtor (the resolved
  `<versioned-iface>` form, e.g. `demo:geo/point-set@0.1.0#[constructor]set`).
- The EXACT resource-intrinsic IMPORT triples: `(module, field, signature)` for
  `resource.new` / `rep` / `drop` of `set`.
- The flat signature of every core function, and which take borrowed self vs
  return owned handle.
- The dtor contract: name, signature, and whether it can be a no-op given the
  emitter's bump allocator (it never frees).
- Any wit-component 0.251 quirks (required `cabi_post_*`, realloc/memory
  expectations, ordering of imports/exports, naming gotchas).
- A minimal known-good recipe (pseudo-ops) the next agent can translate into
  real bodies.
