// dev-notes/functor/plan/03-wire-emit-build.typ — Step 03: wire into emit_component.

#set document(title: "Step 03 — wire the resource into emit_component")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.62em)
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#set heading(numbering: none)
#show heading.where(level: 1): set text(size: 13pt)

= Step 03 — wire the resource into the build path

*First read `plan/00-agent-rules.typ`*, then `summaries/02-bodies.typ` and
`summaries/01-abi.typ`. Critical rules: branch `worktree-functor-build` via
`EnterWorktree path=.claude/worktrees/functor-build`; commit as you go with the
two trailers; no PR; interpreter is the oracle.

== Goal

Make `wavelet build` on the worked example produce a *validating* component.
Runtime call-correctness (the `pts/new`, `pts/add` calls inside `nearest-set`)
is step 04 — here you wire the resource funcs from step 02 into the export path
and get a component that `wasm-tools validate` accepts and whose WIT shows the
`point-set` resource.

== Tasks

+ *Remove the early-return.* PR #22 added a guard at the top of `emit_component`
  that returns `Err("the wasm backend cannot yet build functor components …")`
  whenever `info.functors` is non-empty (it is the first thing in
  `emit_component`, just before `emit_core_module`). Replace it with the real
  path. (Keep the honest behaviour for anything still genuinely unsupported, if
  you discover such a case — but the worked example must now go through.)
+ *`type_env` wiring.* Ensure the boundary `TypeEnv` declares the synthesized
  `set` (per instantiation's interface) as `TypeDef::Resource`, so `wit_ty`
  maps a bare `set` / `own<set>` / `borrow<set>` to `WitTy::Handle` (the handle
  detection is at `emit.rs:177`–`184` and `emit.rs:260`). Make the element type
  resolvable through the same env. The element + resource type info comes from
  `info.functors` (`FunctorInst`) and the synthesized WIT.
+ *Emit + register per instantiation.* For each `FunctorInst` in
  `info.functors`: resolve `elem: WitTy`, call `emit_set_resource` (step 02),
  then register its core funcs as EXPORTS under the canonical names from summary
  01 (`<versioned-iface>#[constructor]set`, `#[method]set.add`,
  `#[method]set.contains`, `#[method]set.size`, and the dtor). Add the
  resource-intrinsic IMPORTS to `em.imports` so they land in the ImportSection
  (`emit.rs:3926`–`3930`); push the exports so they land in the ExportSection
  (`emit.rs:3988`–`3993`). Mirror how ordinary exports are pushed in the export
  loop (`emit.rs:3827`–`3909`).
+ *World/WIT.* `synthesize_world_wit` already declares the resource interface
  and the world already exports it (`wavelet wit` proves this). Confirm
  `select_world` + `embed_component_metadata` accept it unchanged. If the
  encoder needs the world to *export* the interface for the resource to be
  implementable (it should already), verify rather than add.
+ *Multiple instantiations.* Loop over ALL of `info.functors`, not just the
  first — full parity requires e.g. `point-set` and `string-set` in one world.
  Use the per-instantiation interface/element to key names and the element
  `WitTy`.

== Isolated area

`emit_component` (around `emit.rs:703`), the export/import assembly sections, and
the `type_env` construction. Reuse `emit_set_resource` from step 02 unchanged
where possible.

== Verify

- `wavelet build` on the worked example (the one in
  `docs/docs/language/type-system.mdx`: `DefType point`, `Derive`,
  `Import {pkg:"wavelet:coll/set" elem: point as: pts}`, `nearest-set`) produces
  an output file with no error.
- `wasm-tools validate <out>` passes.
- `wasm-tools component wit <out>` shows the `point-set` interface with the
  `set` resource and its four ops.
- Do NOT assert runtime call behavior yet (call routing is step 04). If
  `nearest-set`'s body fails to compile because `pts/new` etc. aren't routed,
  that's expected — coordinate the boundary with step 04: it is acceptable for
  this step to land with the resource exported and validating but the
  `nearest-set` body still unrouted, AS LONG AS you clearly state that in your
  summary. (If removing the early-return makes `emit_core_module` choke on the
  `pts/new` call, you may stub the routing minimally and hand the real routing
  to step 04 — your call; document it.)

== Write `summaries/03-wiring.typ` for step 04

- Exactly where and how the resource funcs are exported and the intrinsics
  imported (the names used, the code site).
- The `type_env` changes (how `set` becomes a `Resource`, how `elem` resolves).
- The build state: does the worked example validate? Does the `nearest-set`
  body compile, and if not, precisely what step 04 must hook into to route
  `pts/op` calls (the call site in `em.expr`, the head form `alias/op`).
