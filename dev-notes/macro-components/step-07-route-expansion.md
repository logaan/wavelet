# Step 7 — Route expansion through the component's `expand`

- [x] Done

> **Read first:** `dev-notes/macro-components.md`, `dev-notes/design.md` §6.3 and
> §9 (the pipeline: "the expander runs macros to fixpoint, instantiating macro
> components on demand"). Base your worktree on the latest
> `origin/macro-components` (after Steps 1–6). **This is the keystone step** —
> after it, foreign macros actually run.

## Context you need

There are **two** expanders and both must learn about foreign macros:

1. **Ahead-of-time expander** — `src/expand.rs`. `expand_file` walks the form
   tree; `expand_form` (`src/expand.rs:42`–) sees a `Call` whose head `Sym` is a
   macro and, if it's a **local** `Value::Macro` in the env, calls
   `interp.expand_once(&mac, arena, payload)` and recurses to fixpoint
   (`src/expand.rs:55`–`60`). `quote-MACRO`/`quasi-MACRO` are skipped
   (`src/expand.rs:52`). This pass feeds WIT synthesis and the wasm emitter.
2. **Lazy interpreter expander** — `src/interp.rs` (`expand_once` ~`:300`,
   `expand_macro` ~`:286`, macro-call dispatch ~`:127`). Used by `wavelet run`
   and the `expand` builtin.

A foreign macro is **not** a `Value::Macro` in the env — it lives in an
instantiated `MacroComponent` (Steps 3/5) and is identified by the
`<name>-MACRO` head that Step 6 registered an arity for. Expanding it means:
marshal the call's argument forms (the `payload`) into a `Tree` (Step 1),
call `MacroComponent::expand(name, &tree)` (Step 3), lift the returned `Tree`
back into the arena, and recurse — to fixpoint, exactly like a local macro.

## Goal

When an expander hits a TitleCase/`-MACRO` head that resolves to a foreign macro
component rather than a local macro, it expands it by calling the component's
`expand`, splices the result back into the tree, and continues to fixpoint —
producing a tree indistinguishable from one a local macro would have produced.

## Scope

- **A lookup that unifies local and foreign macros.** Given a head name, the
  expander must find either the local `Value::Macro` (current behaviour) or the
  `MacroComponent` that owns that macro name (from the Step 5 resolver, keyed by
  the manifest names registered in Step 6). Decide where this registry lives so
  both `expand.rs` and (optionally) `interp.rs` can reach it. The ahead-of-time
  `expand.rs` is the **primary** target — it's what the build pipeline uses.
- **The foreign-expand path in `src/expand.rs`:**
  - convert `payload` (the args form) → `Tree` via Step 1,
  - call `MacroComponent::expand(name_without_suffix, &tree)`,
  - on `Ok(tree)`: convert back to `(arena, root)`, copy into the output arena,
    and **recurse** through `expand_form` so the expansion is itself expanded,
  - on `Err(msg)`: return an actionable error naming the macro (mirror the
    existing `format!("expanding `{}`: …", name.trim_end_matches("-MACRO"))`).
- **Quote/quasi still opaque.** Preserve the existing rule that forms under
  `quote-MACRO`/`quasi-MACRO` are not expanded (`src/expand.rs:52`).
- **The interpreter expander (`interp.rs`).** Decide whether `wavelet run` and the
  `expand` builtin also route through foreign components. The native `run` path
  benefits; the wasm playground can't (no runtime). Recommend: wire the
  ahead-of-time `expand.rs` fully (covers `build`), and make the interpreter path
  either delegate or degrade gracefully when no resolver is present. Document the
  decision; don't break the existing `expand` builtin tests
  (`src/lib.rs` `eval_expand_builtin`).
- **Tests:** an end-to-end-ish test in the build/expand layer — a file importing
  the fixture macro library, using a foreign macro, runs `expand_file` and
  produces the expected expanded tree; then it componentizes (reuse the existing
  `expand::expand_file` + componentize assertion pattern at the bottom of
  `src/lib.rs`). Include a macro whose expansion **contains another macro call**
  to prove fixpoint recursion. Cover the `expand` → `result::err` error path.

## Watch out for

- **Fixpoint & termination.** Foreign expansion can loop just like local
  expansion. Match the existing recursion structure; if you add a depth guard,
  keep it consistent with how local macros behave (today there's none — note if
  you add one).
- **Arena identity.** `expand_once` returns a *new* arena; the foreign path
  likewise yields a fresh `(arena, root)`. Follow `expand.rs`'s existing
  copy-into-`out` discipline (`copy_form`/`descend`) so node ids stay valid.
- **Native/wasm split** again: the foreign-expand path is native-only. In the
  playground, only local macros exist.

## Done when

`cargo test` passes; `expand_file` expands foreign macros from a fixture macro
component to fixpoint and the result componentizes; the error path is covered;
local-macro behaviour and the `expand` builtin are unchanged.

## Handoff notes

### Where the unified macro registry/resolver lives

There is no new central registry; the unification happens in two pieces wired
together by a wasm-safe seam:

- **`expand.rs` owns the dispatch.** A new trait `ForeignExpander` (wasm-safe,
  defined unconditionally — it speaks only `Arena`/`NodeId`, never the
  native-only `meta::Tree`) is the seam. `expand_file` gained a third parameter,
  `foreign: Option<&mut dyn ForeignExpander>`. Pass `None` for a
  local-macros-only expansion (the wasm playground; any file with no macro
  imports).
- **`macrodep.rs` owns the native implementation: `FileExpander`.** It is the
  unified resolver — it holds a `MacroResolver` (the per-build cache of
  instantiated components, from Step 5) plus the file's `macros: true` imports,
  parsed once via `parse_macro_import`. It implements `ForeignExpander`.

**Why re-resolve at expand time (Step 6's forward-note option (b)):** the
read-phase `MacroResolver` is local to `read_file_with_macros` and dropped after
reading, so instantiated components are not carried forward. `FileExpander::for_file`
re-scans the expanded file's roots for `Import {… macros: true}` forms and builds
a fresh `MacroResolver`. Because the resolver caches per *package*, re-resolution
re-instantiates nothing redundantly within the expand pass.

### Exactly how `expand.rs` dispatches local vs foreign

In `expand_form`, when the head is a `Sym`:

1. `quote-MACRO` / `quasi-MACRO` → copied verbatim, **not** expanded (opacity
   holds for both local and foreign macros — a foreign call under `Quote` is
   data).
2. **Local** macro: `env.lookup(name)` yields a `Value::Macro` → `expand_once`,
   then recurse via `expand_form` (unchanged behaviour).
3. **Foreign** macro: only if `foreign` is `Some`. Call
   `fx.expand_call(name_without_suffix, arena, call_id)`:
   - `None` → no foreign macro owns this head → fall through to `descend`
     (ordinary child recursion). This is also what happens for special-form
     heads like `if-MACRO`, which are neither local nor foreign macros.
   - `Some(Ok((arena, root)))` → lift into an `Rc<Arena>` and **recurse through
     `expand_form`** so the expansion is itself expanded (fixpoint).
   - `Some(Err(msg))` → wrap as
     ``format!("expanding `{macro_name}`: {e}")`` and bail.

`FileExpander::owner_of(name)` maps an unsuffixed macro name to the owning
import by querying each `macros: true` import's cached `manifest()`; the
name→owner answer (including "not owned by any") is memoised.

A borrow-checker note: `expand_form`/`descend` thread `foreign` as
`&mut Option<&mut dyn ForeignExpander>` (not by value) so each recursive call
reborrows for a fresh, shorter lifetime — passing the inner `&mut dyn` by value
ties every reborrow to the caller's full lifetime and the sequential per-child
recursion is rejected.

### PINNED args-tree contract passed to `expand` (matters for Steps 8/9)

`expand_call` ships the **WHOLE call form** as the `args` tree: a `tup` whose
element 0 is the macro head (still carrying its `-MACRO` suffix) and elements
1.. are the argument forms, marshalled via `meta::arena_to_tree(arena, call_id)`.
The guest reads `args.nodes[args.root]` as a `tup` and indexes its arguments from
element 1. This matches the Step 3 fixture
(`tests/fixtures/macros/src/lib.rs`, `arg_ids` = `Tup(items).skip(1)`). The
`name` passed alongside the tree is the *unsuffixed* manifest name (e.g.
`unless`, not `unless-MACRO`). **Step 9's producer must lower a Wavelet macro
body to a guest that reads args this same way.**

### interp.rs / `wavelet run` decision

`interp.rs` (lazy expander, `wavelet run` + the `expand` builtin) was **left
unchanged** — it degrades gracefully. The wasm playground has no component
runtime, and `run` already errors on an unbound foreign head, so foreign macros
are not routed through the interpreter. The ahead-of-time `expand.rs` path
(which `build` uses) is fully wired, which is the keystone the rest of the
feature needs. Wiring foreign expansion into native `run` is a possible later
refinement, but is deliberately out of scope here so the existing
`eval_expand_builtin` tests stay green (they do).

### Fixpoint / depth handling

No depth guard added — matches the existing local-macro recursion, which has
none. Both local and foreign expansions recurse through `expand_form`, so a
foreign result containing another (local or foreign) macro call is expanded to
fixpoint (covered by `foreign_macro_expansion_is_re_expanded_to_fixpoint`).

### Call-site threading

`expand_file`'s new parameter is supplied via
`FileExpander::for_file(root, &arena, &roots)` (returns `None` when the file
imports no macro library, so no runtime is instantiated for ordinary files) in:
`build.rs` (`build_files` and `populate_project_wit`) and the `wavelet expand`
CLI (`main.rs`, now reading through `read_file_with_macros`). All other callers
(`tests/wit_deps.rs`, `tests/wkg_populate.rs`, the `lib.rs` aot test) pass
`None`. The `wasm` lib still builds (`cargo check --target
wasm32-unknown-unknown --lib`).

### One extra emit fix (needed for componentization)

`emit::synthesize_world_wit` now skips `is_macro_only` imports when building the
world's import list — a pure macro import is compile-time only and must
contribute no runtime import. Without this, any file using a foreign macro (but
no runtime dep from that package) failed to componentize with "dependency … is
not in the build set". This mirrors `build`'s existing dep-resolution skip.

### Shape of the end-to-end tests (in `src/lib.rs`)

Against the checked-in `tests/fixtures/macros.wasm`, each via
`read_file_with_macros` + `FileExpander::for_file` + `expand_file`:

- `foreign_macro_expands_and_splices_to_fixpoint` — `Unless gt(n 0) 42` →
  `(if-MACRO, (gt, n, 0), {}, 42)`; args spliced, head gone.
- `foreign_macro_expansion_is_re_expanded_to_fixpoint` — `Unless gt(n 0)
  Identity add(n 1)`: after `unless` expands, the nested `identity` in its body
  is *also* expanded → `(if-MACRO, (gt, n, 0), {}, (add, n, 1))`. Proves the
  loop re-expands a foreign result that itself contains a macro call.
- `foreign_identity_macro_expands_and_componentizes` — `Identity add(n 1)` →
  `(add, n, 1)`, and the expanded file **componentizes** (`\0asm`). (Used
  `identity` not `unless` for the componentize check because `unless`'s `{}`
  false-branch is a flag literal the wasm backend doesn't yet emit.)
- `foreign_macro_error_surfaces_with_macro_name` — `Boom` →
  ``expanding `boom`: boom: this macro always fails``.
- `foreign_macro_under_quote_is_not_expanded` — `Quote Unless(false "ran")`
  leaves `unless-MACRO` verbatim.
