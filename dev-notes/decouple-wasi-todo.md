# WASI-decoupling — step-by-step worklist

This is the execution checklist for `dev-notes/decouple-wasi.md`. The work is
split into small, self-contained steps, **one per subagent**. Read this file
*and* `decouple-wasi.md` before starting a step.

## How this worklist is driven

- **One subagent per step.** The orchestrator spawns a fresh agent for the next
  unchecked step, that agent does *only* that step, then stops. Do not run ahead
  into the next step — the boundaries are deliberate handoff points.
- **Each step branches from `origin/main` and must land on `origin/main` before
  the next agent starts.** A subagent's worktree is created fresh from
  `origin/main`, so it can only see prior steps that were actually pushed. The
  last thing every step does is push to `origin/main`.
- **Update this file as part of the step.** Tick the step's box, fill in its
  "Handoff notes" with anything the next agent needs (decisions made, surprises,
  follow-ups), and commit that change together with the step's work.
- **Every step ends green and never regresses http.** `cargo test` must pass, and
  the `http` template must still build and serve, at the end of *every* step
  until it is intentionally re-routed (Step 8) — the magic path stays in place
  and working until then.

## Rules every subagent must follow (paste verbatim into each subagent prompt)

`CLAUDE.local.md` is untracked and absent from worktrees, so relay these to every
subagent (and tell it to relay them onward if it spawns further agents):

- Before any edit, isolate with the **EnterWorktree** tool — your own worktree,
  your own branch. Never edit the shared checkout.
- Commit as you go: small, logical commits in the repo's style (`docs:`,
  `feat:`, `refactor:`, …), not one giant commit at the end.
- When the step is complete **and verified** (`cargo test`; plus
  `./scripts/regen-examples.sh` if you touched language behaviour or examples;
  plus an http template build+serve check if you touched emit/build), push to
  `origin/main`. If pushing would conflict with another agent, rebase and resolve
  cleanly; if you can't resolve safely, stop and report rather than force-push.
- Do exactly one step from this file. Tick its box and write its Handoff notes in
  the same commit. Do not start the next step.

## Verification quick-reference

- `cargo test` — always.
- `./scripts/regen-examples.sh` — after any language/example change (regenerates
  `docs/examples.json`, rebuilds the docs wasm, re-locks `tests/examples.rs`).

---

## Step 0 — Tooling: require and shell out to `wkg` / `wac`

- [x] Done

**Goal.** Make the two external CLIs available to the compiler and add a thin
internal wrapper for invoking them. No language/codegen change yet.

**Scope.**

- A small module (e.g. `src/tools.rs`) that locates `wkg` and `wac` on `PATH`,
  reports a clear actionable error if missing, and wraps the invocations the
  later steps need (`wkg wit fetch`, `wkg wit build`, `wac compose`, `wac plug`,
  `wac targets`).
- Homebrew formula: add `wkg` and `wac` as dependencies.
- A `dev-notes` or README note on the new external dependencies.

**Done when.** `cargo test` passes; the wrapper can run `wkg --version` /
`wac --version` and surfaces a helpful error when a tool is absent. Nothing else
calls the wrapper yet.

**Handoff notes.**

- **New module: `src/tools.rs`** (native-only, registered in `lib.rs` under the
  same `#[cfg(not(target_arch = "wasm32"))]` block as `emit`/`build`/`wit`). It
  is the single place that shells out to the external CLIs. Public surface:
  - `Tool::{Wkg, Wac}` enum with `.bin()`.
  - `run(tool, args)` / `run_in(tool, args, cwd)` — the generic runner. On
    success returns captured **stdout** as a `String`; on `ErrorKind::NotFound`
    returns an actionable `"<bin> was not found on PATH; install it with …"`
    error; on non-zero exit returns `"<bin> failed (<status>): <stderr>"`.
  - `version(tool)` — preflight check; runs `<bin> --version`.
  - Typed convenience wrappers for the invocations later steps need:
    `wkg_wit_fetch` (uses `--type wit`), `wkg_wit_build`, `wac_plug`,
    `wac_compose`, `wac_targets`. **None are called yet** — wire them in at the
    steps that need them (Step 2 → `wkg_wit_fetch`; Step 12 → the `wac_*`).
  - Error type is `Result<_, String>`, matching `build.rs`/`emit.rs` convention,
    so callers can `?`/`map_err(|e| format!("…: {e}"))` as usual.
- **Tests** (`src/tools.rs` `#[cfg(test)]`): `version_when_present` actually
  invokes the real `wkg`/`wac` when on PATH (they are, at `~/.cargo/bin` on the
  dev machine — `wkg 0.15.1`, `wac-cli 0.10.0`) and otherwise asserts the
  not-found message, so the unit suite stays hermetic in toolless CI.
- **Homebrew formula — NOT done here, by necessity.** The `wavelet` formula is
  *not in this repo*; it lives in the separate tap repo
  **`github.com/logaan/homebrew-tap`** (`brew install logaan/tap/wavelet`). This
  repo's `release.yml` only publishes binaries/tarballs; it does not own the
  `.rb`. So the `depends_on "wkg"` / `depends_on "wac"` lines must be added in
  the `homebrew-tap` repo's `wavelet.rb` — a separate edit outside this worktree.
  Whoever cuts the breaking release (Step 17) must make that tap change too, or
  Homebrew installs will lack the tools. The README now documents the dep and
  states the formula declares them, so the prose is ready for the tap to catch up.
- **README:** added a "External tools" subsection under Building documenting the
  `wkg`/`wac` runtime dependency and that `cargo test`/the interpreter don't need
  them.
- **CHANGELOG:** not touched — the dependency isn't user-visible yet (nothing
  calls the wrapper). Fold the `wkg`/`wac` requirement into `## [Unreleased]` at
  Step 16 (CHANGELOG & design notes), per the worklist.
- No language/codegen/example change, so `regen-examples.sh` was not needed;
  full `cargo test` is green (48 lib + examples + http, http template still
  builds).

---

## Step 1 — Consume external WIT from `wit/deps` (no behaviour change)

- [x] Done

**Goal.** Teach the import resolver to load a *parsed* external WIT package from
a project `wit/deps` directory and feed it into the existing `Dep`-shaped
structure the emitter consumes — as a fallback *after* sibling `.wvl` resolution.
The vendored `WASI_PACKAGES` / `wasi-http.wit` blobs stay as-is; this step only
adds a new source, it doesn't remove the old one.

**Scope.**

- `src/build.rs` (`build_files`, `src/build.rs:42`–`56`): add the `wit/deps`
  lookup path. Parse with `wit-parser` (already a dependency).
- Whatever `Dep` construction the emitter expects, populated from parsed WIT.
- A fixture WIT package under a test `wit/deps` and a test proving an `Import`
  resolves to it and produces the same `Dep` shape as a Wavelet dep.

**Done when.** `cargo test` passes; an external WIT package placed in `wit/deps`
is parseable and resolvable; existing magic still primary and unchanged.

**Handoff notes.**

- **New module `src/witdep.rs`** (native-only, gated `#[cfg(not(target_arch =
  "wasm32"))]` like `build`/`emit`/`wit`). Public entry:
  `witdep::resolve_dep(deps_dir: &Path, package: &str) -> Result<Option<Dep>,
  String>`. It builds a `wit_parser::Resolve`, `push_path`-es every entry in
  `wit/deps` (a single `.wit`/`.wasm`/`.wat` file *or* a `ns-name/` package dir),
  finds the package whose `namespace:name` matches the versionless import, and
  projects it into `emit::Dep`. `Ok(None)` means "not here" (no dir, or package
  absent) → caller falls through to its normal unsatisfied-import error;
  `Err` only on a genuine parse failure.
- **Resolution order in `build_files`** is now: (0) `is_external_package` /
  `wasi:*` magic — *unchanged, still primary*; (a) sibling `.wvl` in the build
  set; (b) `wit/deps` via `witdep`. So the new source is strictly a fallback
  *after* sibling resolution, as specified. The unsatisfied-import error string
  changed to mention `wit/deps`.
- **`wit/deps` location**: derived in `build.rs::wit_deps_dir(paths)` as
  `<src-parent>/wit/deps` from the first source path (sources live in `src/`,
  `wit/` is a sibling). This matches the scaffold layout. If a future caller
  passes sources from somewhere other than `src/`, revisit this; for now it's the
  only layout `wavelet new` produces.
- **The `Dep` from WIT is byte-identical to the Wavelet-dep `Dep`** for an
  equivalent surface — verified in `tests/wit_deps.rs`
  (`external_wit_dep_matches_wavelet_dep_shape` compares `package`, `funcs`,
  `package_wit`, `types`). `package_wit_text` deliberately mirrors
  `emit::dep_package_wit`'s nested-package formatting (2-space interface indent,
  `record name { f: t, … }`), so the emitter sees one uniform shape.
- **Type-string mapping** (`witdep::type_string`): primitives → their WIT names
  (`s32`, `u32`, `string`, …); named types (record/variant/enum/resource/alias)
  → their bare name; anonymous compounds rendered structurally
  (`list<…>`/`option<…>`/`result<…>`/`tuple<…>`, `own<T>`/`borrow<T>`). Only
  *record* type decls + resource/alias decls are emitted in the nested package
  WIT today (`type_decl`); variant/enum/flags decls return an error rather than
  emit wrong WIT — none of Step 1's fixtures need them, and the generic-bridge
  steps (3–6) are where the richer types get real codegen. Extend `type_decl`
  when a later step needs those declared.
- **Known pre-existing limitation, NOT a regression**: actually *calling* an
  imported dep function from a body (string- or even s32-returning) fails at
  component encoding (`type mismatch: expected i32 but nothing on stack`) on the
  current emitter — this is identical for a sibling `.wvl` dep and a `wit/deps`
  dep (I checked both). That generic-call lowering is exactly Steps 3–6. So the
  e2e test (`build_resolves_import_from_wit_deps`) only asserts the build gets
  *past import resolution* (no "is not satisfied" error), not that it fully
  builds. When Step 3+ lands, that test can be tightened to a full build/serve.
- **No `wkg` invocation yet** — this step assumes `wit/deps` is already
  populated (tests write fixtures by hand). Step 2 wires `tools::wkg_wit_fetch`
  to populate it for real.

---

## Step 2 — `wkg` populates `wit/` + `wkg.lock`

- [x] Done

**Goal.** Use the Step 0 wrapper so `wavelet build` (and `wavelet new`) can
synthesize the project's own WIT into `wit/` and run `wkg wit fetch` (with
`--type wit`) to populate `wit/deps/` and write/update `wkg.lock`. Still behind
the scenes — the magic path remains the one actually used for codegen.

**Scope.**

- `wavelet build`: write the synthesized world into `wit/` (reuse the
  `wavelet wit` synthesizer so emitted and synthesized WIT stay identical), then
  invoke `wkg wit fetch`.
- `wavelet new`: scaffold `wit/` and fetch+lock deps.
- Tests that a built project ends up with a populated `wit/deps` and a
  `wkg.lock`. Keep the unit suite hermetic; make the live-fetch path an
  integration test, since it needs registry access the way CI may not have.

**Done when.** `cargo test` passes; a built sample project has `wit/deps` +
`wkg.lock`; no codegen behaviour change.

**Handoff notes.**

- **Behind-the-scenes, codegen untouched.** `build_files` runs exactly as before
  through `emit_component` (the magic path); the new `wkg` work happens *after*
  all components are written, and any failure is a `warning:` on stderr, never a
  build failure. So an offline / toolless environment (incl. CI) still builds.
  This is what keeps the hermetic suite green without `wkg`.

- **New WIT synthesis surface (`src/wit.rs`).** `synthesize` was refactored to
  delegate to a private `synthesize_info(arena, &FileInfo, host_only)`. Two new
  public fns:
  - `synthesize_fetch_world(arena, roots)` → the same WIT `synthesize` emits but
    with `host_only = true`: **only `wasi:*` imports/exports are kept.** Sibling
    (build-set) imports are *dropped* from the fetch world, because `wkg wit
    fetch` insists on a registry for *every* referenced package's namespace — a
    locally-present `demo:greeting` still makes it error `no registry configured
    for namespace "demo"`. Sibling packages are kind-(2) deps (wired by the build
    set / `wac`), not kind-(1) WIT fetched by `wkg`, so excluding them is correct,
    not a hack. The host-import WIT text is byte-identical to what `synthesize`
    emits, so the world `wkg` parses matches what the emitter componentizes
    against.
  - `has_host_deps(&FileInfo)` → whether a component references any host package
    (a `Target`, a `wasi:*` import, or an external-iface export). Only such
    components are fetched; a pure domain model (greeting) contributes nothing.

- **The `Target "wasi:cli/command"` translation (Step-2 glue, retires with
  `Target` in Step 11).** In the *fetch* world only, `include wasi:cli/command;`
  is replaced by `export wasi:cli/run@0.2.0;`. Reason: `wkg wit fetch` can't merge
  a world that `include`s a world whose package it hasn't fetched yet
  (chicken-and-egg → `package not found … include wasi:cli/command`). Referencing
  one concrete interface (`wasi:cli/run`) instead makes `wkg` pull the whole
  `wasi:cli` package + transitive deps into `wit/deps`. The http path needs no
  such translation — it already exports `wasi:http/incoming-handler` concretely.
  When `Target` goes away (Step 11), drop this special-case in `synthesize_info`.

- **Step 0 wrapper bugfix (`src/tools.rs::wkg_wit_fetch`).** As written in Step 0
  it ran `wkg` with the wit dir as cwd and the *default* `--wit-dir wit`, so it
  looked for `<wit>/wit` — wrong. Now it runs from the wit dir's **parent** and
  passes `--wit-dir <name>`, so `wit/deps` and `wkg.lock` land correctly (lock
  beside `wit/`, at the project root). Also: a bare relative `wit` has an empty
  `Path::parent()`, and an **empty `current_dir` makes the OS fail program lookup
  with a spurious "not found on PATH"** — normalized to `.` via the new
  `run_dir_for` (unit-tested). This bit hard: relative-path `wavelet build`
  silently warned "wkg not found" while absolute-path builds worked. If you add
  `wac` invocations (Step 12) that set `current_dir`, watch for the same trap.

- **Project layout / where files land.** `build.rs::project_root(paths)` =
  `<src-parent>` (the parent of `src/`), normalized to `.` when bare; `wit/` is
  its child. `wit_deps_dir` (Step 1) now derives from `project_root` too. For a
  multi-component project, each host-dep component's fetch world is written as the
  *single* root `wit/<world>.wit` (others first cleared via `clear_root_wit`,
  which only removes top-level `*.wit`, never `wit/deps`), then fetched; deps and
  the lock **accumulate** in the shared `wit/` across components. The templates
  have exactly one host-dep component each, so in practice one world file remains
  (`wit/main.wit` or `wit/app.wit`). If a future project has *two* host-dep
  components, only the last world file persists on disk — fine for fetching, but
  revisit if `wit/` must hold every component's world simultaneously.

- **`wavelet new` wiring lives in `main.rs::new_cmd`**, not `scaffold.rs`: after
  `scaffold::create`, it filters the written `.wvl` files and calls the new
  `build::populate_project_wit(root, &src_paths)` (a units-only variant of the
  build-time path that skips emit). Kept out of `scaffold` so the scaffold stays a
  pure file-writer and `cargo test`'s scaffold tests need no tools.

- **`.gitignore` unchanged** (still only `/out`), so a scaffolded project's
  `wit/` and `wkg.lock` are tracked — i.e. "ships with deps pinned." If a later
  step (docs, Step 13) decides `wit/deps` should be re-fetched rather than
  vendored-in-vcs, adjust the template `.gitignore` there.

- **Tests.** `tests/wkg_populate.rs`: hermetic assertions on the fetch worlds
  (`http_fetch_world_is_host_only`, `cli_fetch_world_references_wasi_cli_run`,
  `has_host_deps_only_flags_components_with_wasi`) + one **gated** live-fetch test
  (`build_populates_wit_deps_and_lock`) that skips unless `wkg` is present *and* a
  registry probe succeeds, then asserts a built cli project has `wit/deps/wasi-cli*`
  and `wkg.lock`. `src/tools.rs` gained `run_dir_for_normalizes_empty_parent`
  (pure, hermetic). Full `cargo test` green (49 lib + examples + http + wit_deps +
  4 new). No language/example change → `regen-examples.sh` not needed.

- **No-regression check done for real:** scaffolded an `--type=http` project,
  `wavelet build` + `wavelet compose`, then `wasmtime serve` — the page renders
  and echoes the request path. The http template still builds *and serves*. The
  served project also ended up with `wit/deps/wasi-http-0.2.0` + `wkg.lock`.

- **Verified locally with `wkg 0.15.1` at `~/.cargo/bin`.** The live test needs
  network to reach the default registry; it self-skips otherwise.

---

## Step 3 — Generic bridge: primitives, flattening, retptr, records, tuples

- [x] Done

**Goal.** Begin the generic canonical-ABI lowering that, given a WIT function
signature from parsed WIT, emits the core call — starting with: parameter
flattening, return via retptr, primitives (ints, floats, bool, char), records,
and tuples. Built **alongside** the existing magic (not replacing it yet) and
parameterised by the signature instead of by a `match fname`.

**Scope.**

- New lowering scaffold in `src/emit.rs` driven by a parsed WIT signature,
  covering the value kinds listed above.
- Prove it: a synthetic test interface whose functions take/return these kinds
  compiles through the *generic* path and re-encodes cleanly with
  `wit-component`. Do **not** delete any hand-coded path.

**Done when.** `cargo test` passes; functions over primitives/records/tuples
compile via the generic bridge and validate; http/cli magic untouched and green.

**Handoff notes.**

- **The generic bridge already existed and works — Step 3 mostly *proved* and
  *completed* it.** The canonical-ABI lowering parameterised by a parsed
  `FuncSig` is `Emitter::dep_call` (`src/emit.rs`), backed by the type machinery
  `wit_ty` → `WitTy` and the per-shape functions `flat`/`flat_checked`/`flat_len`,
  `align_of`/`size_of`/`elem_size`/`record_field_offsets`, and the four codegen
  cores: `lower` (box → flats), `lift`/`lift_flat` (flats → box),
  `store_to_mem`/`load_from_mem` (box ↔ canonical memory). The matching **export**
  side is the wrapper loop in `emit_core_module` (search "`---- export
  wrappers`"). The import core-signature is built in the `feats.dep_calls` loop
  (params flattened via `flat_checked`, retptr `i32` appended when
  `flat_result == Retptr`). **This is the scaffold Steps 4–6 extend** — add a new
  `WitTy` arm and thread it through those same functions; there is no separate
  "bridge module."

- **What Step 3 added:** two value kinds the bridge couldn't carry — `WitTy::Char`
  (single i32 flat = u32 codepoint, boxed in the int box like a `u32`; parsed
  from `char`) and `WitTy::Tuple(Vec<WitTy>)` (anonymous positional aggregate;
  parsed from `tuple<...>` via `split_type_args`). A tuple's **memory layout is
  identical to a record** — `record_field_offsets` now handles both — so
  `size_of`/`align_of`/`store`/`load` share one path; only the *value-level* box
  differs (a `TAG_TUP` box, element ptrs at `@8+4i`, vs `TAG_REC`'s
  key/value pairs at `@8+8i`). Tuples were also added to the two **retptr
  aggregate** branches (in `dep_call` *and* the export wrapper) — a tuple result
  goes through the callee-owned memory-area path like a record, not the
  string/list `(ptr,len)` path.

- **The Step 1 handoff note's "calling a dep fails at component encoding (type
  mismatch: expected i32 but nothing on stack)" was a false alarm — NOT a real
  bridge bug.** It was a malformed test source: the explicit Export form wants
  `params:` as a **record** (`params: {n: s32}`), but that note's snippet used a
  *list* (`params: [{n: s32}]`), which `parse_explicit_sig` silently dropped
  (`Node::Rec` arm only), so the export was emitted with **zero** params while
  the core wrapper took one — hence the stack mismatch. With the record form,
  s32/string/record/tuple dep calls all build and validate cleanly on the
  generic path. So the Step 1 e2e note ("only asserts it gets past import
  resolution") can now be tightened: `tests/generic_bridge.rs` does a **full
  validated build** through `dep_call`.

- **Source-syntax gotchas for the next agent's tests** (all hit during Step 3):
  - Result-type inference (`wit::infer`) does **not** see through a dep call, so
    a function whose body is just `alias/fn(...)` needs the **explicit Export
    record form** with `result:` — `Export {name: f params: {…} result: T}` — or
    `collect` errors `cannot infer result type`.
  - `params:`/record types use the record form `{k: T …}`; `tuple[a b]` for tuple
    types; `DefType point {x: s32 y: s32}` (no `Rec` keyword in source).
  - Multi-arg dep calls use the list-payload call form `alias/fn[a b]`;
    single-arg uses `alias/fn(x)`.

- **`char` is a boundary type only; there is no `char` *value* in the language
  yet.** `Node::Char` still errors in `emit::expr` ("char values not supported")
  and the interpreter has no char literal. Step 3 makes a WIT signature that
  *mentions* `char` lower/lift correctly (a u32 carried in an int box), so a dep
  taking/returning `char` compiles — but you can't yet write a `char` literal to
  pass one. If a later step needs first-class chars, that's a language change
  (interpreter + examples + `regen-examples.sh`), out of this bridge's scope.

- **Floats (`f64`):** already handled by the pre-existing `WitTy::F64` (dec box);
  Step 3 didn't need to touch it. `s64`/`u64` use `WitTy::S64` (i64 flat). Step 3
  added no new `f32`/`s8`/`s16` widths — `wit_ty` maps `s8/s16/s32` → `IntS`,
  `u8/u16/u32` → `IntU` as before (so sub-i32 widths are boxed as i32; the
  component encoder accepts this for the flat ABI). If Step 4+ needs `f32`, add a
  `WitTy::F32` arm.

- **Test:** `tests/generic_bridge.rs` (2 tests) builds one-component projects that
  import a synthetic `acme:shapes` / `acme:pairs` interface from `wit/deps` (no
  compiler knowledge of them) and **fully build + `wit-component`-validate** them
  via the generic path — covering s32/bool/char primitives, a record, tuples
  (incl. a `tuple<s32, string>` with a heterogeneous string element, a `tuple<
  point, point>` of records, and a retptr `tuple<s32, s32>` result). It is the
  template to *extend* in Steps 4–6: add functions over the new kinds to a
  synthetic interface and assert the build validates.

- **Magic untouched.** No change to `http_call`/`http_imports`/`is_resource_name`
  or the cli helpers. Full `cargo test` green (49 lib + examples + http +
  wit_deps + wkg_populate + 2 new generic_bridge). No language/example behaviour
  change, so `regen-examples.sh` was not needed; the http template still builds
  and serves via the magic path (http suite green).

---

## Step 4 — Generic bridge: lists, strings, options, results, enums, variants, flags

- [x] Done

**Goal.** Extend the Step 3 lowering to the remaining value types: lists and
strings (memory allocation/copy via `cabi_realloc`), `option`, `result`, `enum`,
`variant`, and `flags`. Still alongside the magic.

**Scope.**

- Add these kinds to the generic lowering.
- Prove it: extend the synthetic test interface to exercise each kind through the
  generic path; re-encode cleanly.

**Done when.** `cargo test` passes; functions over the full non-resource type set
compile via the generic bridge and validate; magic untouched and green.

**Handoff notes.**

- **The full non-resource type set now flows through the generic bridge.** Lists,
  strings, `option`, and `result` already worked from Step 3; this step added the
  discriminated/bitset kinds: `WitTy::{Enum, Variant, Flags}` (`src/emit.rs:72`).
  - Variants use an N-case lower/lift/store/load that *generalises* the 2-case
    `option`/`result` path (see `cases()` at `src/emit.rs:114`, which maps
    option→`[none, some(t)]`, result→`[ok(t), err(e)]`, enum→payload-less cases,
    variant→its declared cases). Layout uses 1/2/4-byte discriminant sizing
    (`disc_size`) and a max-payload join. Enums are a payload-less variant
    (i32 discriminant ↔ `TAG_VAR` box); flags are an i32 bitset ↔ a record-of-bools
    box, with 1/2/4-byte flags-word sizing (`flags_align`/`flags_size`).
- **New `Dep.type_defs: Vec<(String, TypeDef)>` carrier** (`src/emit.rs:41`).
  Records keep their own existing map; enum/variant/flags type *declarations*
  travel via `type_defs` so the boundary `TypeEnv` can resolve a named type to a
  `WitTy` (`wit_ty` at `src/emit.rs:244`). `witdep` now projects enum/variant/flags
  into `type_defs` and renders them in the nested-package WIT text — `type_decl`
  no longer errors on them (the Step 1 limitation noted under Step 1 is resolved
  for these three kinds).
- **Any test or caller that constructs an `emit::Dep` by hand must set the new
  `type_defs` field** (e.g. `tests/wit_deps.rs` now passes `type_defs: Vec::new()`).
  Watch for this when adding fixtures in Steps 5–7.
- **Test source gotcha (still true):** inference can't see through a dep call, so
  each exported fn that forwards a dep value must use the explicit
  `Export {name: … params: {…} result: …}` record form with a primitive result.
  The Step 4 proof test (`generic_bridge_lowers_enum_variant_flags_lists_options`
  in `tests/generic_bridge.rs`) keeps the dep-defined `color`/`shape`/`perms` types
  *off* the app's own WIT by round-tripping each value entirely inside a body
  (`make-X` lifts, `X-code` lowers) — Wavelet source has no enum/variant/flags
  type syntax to re-declare them on an export, so this in-body round-trip is the
  way to exercise those lowerings until that syntax exists.
- **Next (Steps 5–6): resource handles/methods.** Add a `WitTy::Handle` arm and
  thread it through the same layout/lower/lift/store/load functions and the
  export-wrapper / import-signature loops — same extension shape as this step.
  The `is_resource_name` allowlist (`src/emit.rs:127`) is what Step 5 retires for
  the generic path.
- **Process note:** the agent that wrote `e60304e` (the enum/variant/flags
  codegen) was cut off by a session limit before committing the proof tests,
  ticking this box, or writing these notes; the orchestrator finished those.
  `cargo test` is fully green (incl. the http template — magic untouched).

---

## Step 5 — Generic bridge: resource handles (own/borrow)

- [x] Done

**Goal.** Add resource *handles* to the generic bridge: produce a `WitTy::Handle`
for any WIT `resource`/`own`/`borrow` from parsed WIT (passing/returning i32
handles), retiring the `is_resource_name` allowlist (`src/emit.rs:127`) *for the
generic path*. Resource *methods* and *drop* come in Step 6. Still alongside the
magic.

**Scope.**

- Handle typing + lowering/lifting in the generic bridge from parsed WIT.
- Prove it: a synthetic interface that passes own/borrow handles compiles through
  the generic path and validates.

**Done when.** `cargo test` passes; own/borrow handles flow through the generic
bridge from parsed WIT; magic untouched and green.

**Handoff notes.**

- **A handle is `WitTy::Handle` — a single i32 flat, carried in an int box.**
  `own<T>` and `borrow<T>` lower/lift *identically* (one i32; the canonical ABI
  doesn't distinguish them at the flat level), so there is one arm, not two. The
  full layout/codegen for `Handle` was already wired in by Step 3 (the enum arm
  existed): `flat_checked`/`flat_len` (1×i32), `align_of`/`size_of`/`elem_size`
  (4 bytes), and the four cores — `lower` boxes→i32 via `unbox_int`+`I32WrapI64`
  (`src/emit.rs` ~1727), `lift` i32→box via `I64ExtendI32U`+`box_int` (~2025),
  and `store_to_mem`/`load_from_mem` treat it as a 4-byte int. The import
  core-signature loop (`feats.dep_calls`) and the export-wrapper loop both run
  off `flat_checked`/`wit_ty` with no per-type branch, so handle params/results
  flowed the moment `wit_ty` produced `Handle`. **So Step 5 was mostly *typing*,
  not codegen.**

- **What Step 5 actually changed — retiring `is_resource_name` for the generic
  path.** `wit_ty` already mapped `own<...>`/`borrow<...>` → `Handle` (those win
  before the allowlist), so *those two already worked* from parsed WIT. The gap
  was a **bare resource-name** reference (a param typed just `widget`, not
  `own<widget>`), which previously only typed as a handle if the name was baked
  into the hardcoded `is_resource_name` allowlist (`src/emit.rs:174`, the wasi
  http names). Now:
  - **New `TypeDef::Resource` arm** (`src/emit.rs`, the `TypeDef` enum) — carried
    on `Dep.type_defs` like enum/variant/flags, lands in `TypeEnv.defs`.
  - **`witdep::resolve_dep`** projects `TypeDefKind::Resource` → `TypeDef::Resource`
    into `type_defs` (the nested-package WIT text already emitted `resource name;`
    via `type_decl`, unchanged).
  - **`wit_ty`** now resolves a bare name to `Handle` when `env.defs` says it's a
    `TypeDef::Resource`, *before* falling back to `is_resource_name`. The allowlist
    is kept only as the magic-http fallback (that path has no `type_defs` for the
    vendored wasi resources), and is otherwise untouched — Step 11 deletes it.

- **Proof test:** `generic_bridge_passes_resource_handles_own_borrow` in
  `tests/generic_bridge.rs`. A synthetic `acme:res` dep declares `resource widget`
  and three fns: `open -> own<widget>`, `tag(borrow<widget>) -> s32`,
  `peek(widget) -> s32` (bare-name param — the case that *requires* the new
  `TypeDef::Resource` typing; `widget` is **not** in `is_resource_name`). Two
  exports round-trip a handle entirely inside a body (`r/tag(r/open(n))`,
  `r/peek(r/open(n))`) so `widget` never lands on the app's own exported WIT —
  same Step-4 trick (inference can't see through dep calls → explicit
  `Export {name … params {…} result …}` record form with a primitive result). The
  built component fully re-encodes/validates with `wit-component`.

- **Step 6 (resource methods + drop) hooks here.** Handles are represented as
  `WitTy::Handle` (i32 in an int box). Method lowering should hook into
  `Emitter::dep_call` (the same generic call path `dep_calls` drives), the way the
  hand-coded `http_call` (`src/emit.rs` ~1600–1720) special-cases the wasi-http
  resource ops today: a `[method]res.op` / `[static]res.op` / `[constructor]res` /
  `[resource-drop]res` WIT function is *already* a `FuncSig` with handle params
  (the `self`/`this` arg is an `own`/`borrow` handle), so the existing lower/lift
  threading carries the handle args — what Step 6 adds is (a) recognising those
  method-name shapes in the parsed WIT and (b) emitting the `[resource-drop]`
  import + call. The synthetic-WIT + in-body-round-trip test pattern extends
  directly: declare a `resource` with methods in the dep WIT and call them.

- **No language/example/behaviour change** (the bridge is a parallel path; no
  `Node`/interpreter change), so `regen-examples.sh` was **not** run. Full
  `cargo test` green (49 lib + 4 generic_bridge + examples + http + wit_deps +
  wkg_populate); the http template still builds via the untouched magic path.

---

## Step 6 — Generic bridge: resource methods + drop

- [x] Done

**Goal.** Complete the generic bridge with resource method calls (`[method]`,
`[static]`, `[constructor]`) and resource `drop`. Still alongside the magic.

**Scope.**

- Method/constructor/static/drop lowering in the generic bridge.
- Prove it: the WASI-http operations currently hand-coded in `http_call`
  (`fields`, `outgoing-response`, `body`, `path-with-query`, `set`, `write`,
  `finish`) all compile through the *generic* path in a test, matching the magic
  output. Magic path still present.

**Done when.** `cargo test` passes; the http resource operations build through the
generic bridge in a test; the existing http template still builds+serves via the
magic path (no regression).

**Handoff notes.**

- **The generic *import* bridge is now complete.** Every WIT value kind *and*
  every resource-operation kind lowers/lifts through `Emitter::dep_call` driven
  by a parsed `FuncSig`. As Step 5 predicted, the codegen needed almost no new
  work — a resource op's `self`/`this` handle is just its first param (parsed WIT
  prepends `self: borrow<T>` for methods), so the existing `Handle` lower/lift
  threading already carried it. Step 6 was **name resolution + WIT rendering +
  synthesizing drop**, not new ABI codegen.

- **How a source op name resolves to a (possibly mangled) WIT function**
  (`src/emit.rs`, `dep_func_op` + `resolve_dep_func`, just below
  `versioned_iface`). `wit-parser` names resource ops `[constructor]res`,
  `[method]res.op`, `[static]res.op`; the implicit drop is `[resource-drop]res`.
  The source reaches each by a **bare op name**, exactly like the magic's
  `http/<op>`:
  - `[constructor]res`   → source `r/res`        (e.g. `http/fields`)
  - `[method]res.op`     → source `r/op`         (e.g. `http/body`)
  - `[static]res.op`     → source `r/op`
  - `[resource-drop]res` → source `r/drop-res`   (the `drop-` prefix keeps it from
    colliding with the resource's own constructor, whose op name is `res`).
  `resolve_dep_func` matches a freestanding name directly, else any func whose
  `dep_func_op` equals the source name, and **errors on ambiguity** (two ops in
  one interface sharing a bare op name — not yet disambiguable from source, since
  names are kebab-only and can't spell the mangled form). Both `dep_call` and the
  import-signature loop (the `feats.dep_calls` loop, ~`src/emit.rs:2920`) use this
  resolver, and both key the host import by the **mangled** `sig.name` (what
  `wit-component` re-validates against the WIT).

- **`[resource-drop]` is synthesized, not parsed.** It is *not* a WIT
  `function`, so `witdep::resolve_dep` now pushes a synthetic `FuncSig`
  `[resource-drop]<res>` (params `[self: own<res>]`, no result) for every
  `TypeDefKind::Resource`. That makes drop a normal `dep.funcs` entry — the
  generic path emits the implicit `[resource-drop]res` import + call with zero
  special-casing. (It is deliberately *not* rendered into the package-WIT text;
  the component model adds the drop import implicitly from the `resource` decl.)

- **`witdep` package-WIT rendering now nests resource operations.** Previously
  `package_wit_text` emitted every `iface.functions` entry flat, which produced
  invalid WIT like `[constructor]packet: func(...);`. It now: (a) emits only
  *freestanding* funcs at interface scope; (b) renders each resource's
  constructor/method/static **inside** its `resource name { … }` block
  (`resource_func_decl` un-mangles the op name and drops the implicit `self` for
  methods). `type_decl` gained the `iface` arg to find a resource's ops by
  `func.kind.resource() == Some(id)`. Async/freestanding-on-resource kinds are
  rejected loudly. This is the bit that bit me: the *lowering* worked
  immediately; the synthesized-WIT-text round-trip is what failed to parse until
  this fix.

- **Proof test:** `generic_bridge_lowers_resource_methods_static_constructor_drop`
  in `tests/generic_bridge.rs`. A synthetic `acme:wire` interface mirrors the
  exact function-kinds + handle/retptr shapes `http_call` lowers by hand (a
  doc-comment table maps each synthetic op to its http counterpart:
  constructor↔`fields`/`outgoing-response`, retptr `result<own<T>>` method↔`body`,
  retptr `option<string>` method↔`path-with-query`, method+list↔`write`, static
  with a `result` arg↔`set`, static-over-`own`↔`finish`, drop↔the
  `[resource-drop]output-stream` inside `write`). Each op is driven through the
  generic path inside a body returning a primitive (so the resources stay off the
  app's own WIT), and the component fully re-encodes/validates with
  `wit-component`. **The magic `http_call` is untouched** and the http template
  still builds *and serves* (verified for real: scaffolded `--type=http`, built +
  composed + `wasmtime serve`, `GET /` returns the rendered page echoing the
  path).

- **One generic-vs-magic gap that is *not* a bug, but Step 8 must know about it.**
  The generic `result` lowering requires **both arms typed** (`result<T, E>`); a
  single-arm `result<own<T>>` errors `only result<T, E> … is supported`. The real
  wasi-http `body`/`finish` return single-arm `result<own<T>>` /
  `result<_, error-code>`, and the magic `http_call` sidesteps the generic
  `result` path entirely — it hand-reads the `ok` handle at the payload offset and
  discards the error. So **before Step 8 can route http through the generic
  bridge, the generic `result` lowering must learn the single-arm forms**
  (`result<T>`, `result<_, E>`, bare `result`), or http's WIT must be adapted.
  My test uses `result<own<box>, s32>` (both arms) to stay inside today's generic
  support while still exercising the retptr-result-method path. This is the main
  carry-forward for Step 8.

- **No language/example/behaviour change** (parallel path; no `Node`/interpreter
  change), so `regen-examples.sh` was **not** run. Full `cargo test` green (49 lib
  + 5 generic_bridge + examples + http + wit_deps + wkg_populate).

---

## Step 7 — Generic export of arbitrary interfaces

- [x] Done

**Goal.** Export an arbitrary interface (e.g. `wasi:http/incoming-handler`,
`wasi:cli/run`) using the parsed WIT signature of the target, generalising
`is_external_iface` / `external_versioned` (`src/emit.rs:569`–`577`, `2535`) with
no `is_command`/`is_http` branch. Still alongside the magic.

**Done when.** `cargo test` passes; a hand-written component can export an
interface through the generic export path in a test; the `run`-specific
`() -> result` wrapper is reproducible as "export this function into
`wasi:cli/run` with its WIT signature." Magic untouched.

**Handoff notes.**

- **The generic export path already mostly existed — Step 7 closed the three
  gaps that kept it from carrying `wasi:cli/run`'s `() -> result` and an
  arbitrary versioned interface.** The export-wrapper loop in `emit_core_module`
  (search "`---- export wrappers`", ~`src/emit.rs:3076`) already lifts each
  export's params and lowers its result *entirely off the parsed `FuncSig`*, and
  already routes an external iface (`is_external_iface`, i.e. `iface.contains(':')`)
  to its own versioned export name. So a `FuncSig` whose `iface` is e.g.
  `wasi:cli/run` (set via the explicit `Export {iface: "…" …}` record form,
  parsed in `wit::parse_explicit_sig`) flows through the generic wrapper with
  **no `is_command`/`is_http` branch**. The hand-coded `run` special-case
  (`if is_command && sig.name == "run"`, ~`src/emit.rs:3083`) is **untouched and
  still present** — it only fires for the `wasi:cli/command` *target*, which the
  generic path never sets.

- **Gap 1 — single-arm / bare `result` (the Step 6 carry-forward).** `run`'s
  signature is `func() -> result` (a bare `result`, no arms). `wit_ty`
  (`src/emit.rs`) previously errored on anything but `result<T, E>`. Now it parses
  `result<T>`, `result<_, E>`, `result<T, _>`, and bare `result` into a 2-case
  `ok`/`err` `WitTy::Variant` where a missing/`_` arm is payload-less — reusing
  the whole general variant lower/lift/store/load machinery (the canonical-ABI
  flattening of a payload-less 2-case variant is a single i32 discriminant,
  *identical* to `result<_, _>`). **`result<T, E>` with both arms typed is
  unchanged** — still `WitTy::Result`, byte-for-byte. The case names stay `ok`/
  `err` so `Match [(ok …)(err …)]` still resolves, and a unit `ok`/`err` is
  built in source by `ok(0)` (the payload is dropped for a payload-less arm).
  **This resolves the Step 6 gap** noted under Step 6 ("the generic `result`
  lowering must learn the single-arm forms before Step 8 can route http"). The
  real wasi-http `body`/`finish` return `result<own<T>>` / `result<_, error-code>`
  — both now lower through the generic path (proven for `result<own<box>, s32>`
  in Step 6 already, and `result<own<…>>`-style single-arm now too).

- **Gap 2 — versioning external exports by their *resolved* package, not the
  hardcoded WASI version.** `external_versioned(path)` hardcodes `@0.2.0`
  (`WASI_VERSION`), which is wrong for any non-WASI package (the test deps are
  `@0.1.0`). New `external_versioned_in(path, deps)` (`src/emit.rs`, beside
  `external_versioned`) looks up the iface's package in the `deps` map and uses
  *its* version (`Dep.package` carries the full `ns:name@ver`), falling back to
  `external_versioned` (the WASI default) when there's no dep — i.e. the magic
  http/cli path, which has no `Dep` for its vendored interfaces, is unaffected.
  Both export callsites now use it: the world-export line in
  `synthesize_world_wit` and the export-wrapper name in `emit_core_module`.

- **Gap 3 — making the exported interface's WIT available to the encoder.**
  `wit-component` validates the export wrapper against the real interface WIT,
  so the exported external package must be in the synthesized world. `deps` was
  only populated from a component's *imports*; `build_files` now also resolves
  each *external export* iface's package (`sig.iface` split at `/`, e.g.
  `wasi:cli/run` → package `wasi:cli`) from `wit/deps` via `witdep::resolve_dep`
  into the same `deps` map (`src/build.rs`, right after the import loop). Its
  `package_wit` is then appended by the existing `for dep in deps.values()` tail
  in `synthesize_world_wit`. An export-only dep produces **no spurious import**
  (the world's import lines come from `info.imports`, not from `deps`).

- **Proof tests** (`tests/generic_bridge.rs`, two new):
  - `generic_bridge_exports_arbitrary_interface` — exports `greet` into a
    synthetic `acme:greet/greeter` (string + record params, retptr-string
    result) via the explicit `Export {iface: …}` form, WIT from `wit/deps`, no
    compiler knowledge of `acme:greet`; the component re-encodes/validates.
  - `generic_bridge_exports_run_style_unit_result` — the `wasi:cli/run` shape
    reproduced: exports `run: func() -> result` into a synthetic `acme:cli/run`,
    body returns `ok(0)`; the wrapper lowers it to the single-i32 `result`
    discriminant off the parsed signature. This is the literal "`run` is just
    'export this function into `wasi:cli/run` with its WIT signature'" check.

- **For Step 8 (cut http to the generic path end-to-end).** The generic *import*
  (Steps 3–6) and *export* (this step) bridges are both complete and proven on
  synthetic WIT. To drive http through them:
  - **Export** `wasi:http/incoming-handler#handle` via `Export {name: handle
    iface: "wasi:http/incoming-handler" params: {…} result: …}` with the real
    handler signature; `build_files` will pull `wasi:http`'s WIT from `wit/deps`
    (already fetched by Step 2's `wkg`) and `external_versioned_in` will version
    it correctly. **Import** the http resource ops as a normal `Import {pkg:
    "wasi:http/types" as: …}` dep and call them by bare op name (Step 6's
    `dep_func_op` resolver), e.g. `http/body`. No more `http_call`/`http_imports`.
  - **The single-arm `result` gap is closed** — http's `result<own<T>>` /
    `result<_, error-code>` now lower generically. The one remaining thing to
    confirm in Step 8 is that the *real* `wasi:http` WIT (with its `error-code`
    variant and `wasi:io/streams` use) round-trips through `witdep` cleanly; the
    synthetic tests use simplified arms (`result<own<box>, s32>`) and the magic
    still force-imports `wasi:io/streams` (`is_http`), which Step 8/11 retires.
  - **`is_external_package`/`is_external_iface` are still magic-flavoured.**
    `is_external_package` is literally `starts_with("wasi:")`; the build skips a
    Dep for those imports (`build.rs` line ~51) because the magic vendors their
    WIT. For Step 8's http imports you'll want `wasi:http/types` resolved as a
    real `Dep` from `wit/deps` (so `dep_call` has its `FuncSig`s) rather than
    skipped — i.e. that `is_external_package` skip is the next thing to retire on
    the http import side. Step 7 left it alone (only the *export* side needed
    the dep) to keep the magic path green.

- **No language/example/behaviour change** (the export path is parameterised by
  the parsed signature; no `Node`/interpreter change, no new source syntax), so
  `regen-examples.sh` was **not** run. Full `cargo test` green (49 lib + 7
  generic_bridge + examples + http + wit_deps + wkg_populate); the http template
  still builds via the untouched magic path (http suite green).

---

## Step 8 — Cut http over to the generic path

- [ ] Done

**Goal.** Route the `wasi:http/proxy` template/components through the generic
import bridge + generic export end-to-end, with WIT coming from `wit/deps`
(`wkg`), while leaving the magic code physically present but unused for http.

**Done when.** `cargo test` passes; the http template builds **and serves**
through the generic path (this is the no-regression gate); the http magic is now
dead code reachable only by removal in Step 11.

**Handoff notes.** *(fill in)*

---

## Step 9 — Cut cli over to the generic path

- [ ] Done

**Goal.** Route the cli template through the generic import bridge + generic
export, with WIT coming from `wit/deps`, leaving the cli magic physically present
but unused. The `print`/`println`/`args` builtins still exist at this point —
they are removed in Step 10 — so this step keeps them working but compiled via
the generic path where it already covers them, or via the magic until Step 10.

**Done when.** `cargo test` passes; the cli template builds and runs through the
generic path; the cli magic is now dead code reachable only by removal in
Step 11.

**Handoff notes.** *(fill in)*

---

## Step 10 — Remove the WASI builtins and migrate examples

- [ ] Done

**Goal.** Remove `print`/`println`/`args`/`read-line`/`env` from the language
(`src/builtins.rs:18`, `:343`+ and the interpreter). Output/args now go through
explicitly-imported WASI interfaces (or an ecosystem wrapper) via the generic
bridge.

**Scope.**

- Remove the builtins from `builtins.rs` and `interp.rs`.
- Migrate the `cli` template and every doc/example that used them; regenerate
  examples (`./scripts/regen-examples.sh`) and re-lock `tests/examples.rs`.

**Done when.** `cargo test` and `./scripts/regen-examples.sh` both green; no
references to the removed builtins remain.

**Handoff notes.** *(fill in)*

---

## Step 11 — Delete the magic and `Target`

- [ ] Done

**Goal.** Now that nothing uses them, delete the special cases: `is_command`,
`is_http`, the forced `wasi:io/streams` import, `WASI_PACKAGES`, `WASI_HTTP_WIT`
(+ `wasi-http.wit`), `http_imports`, `http_call`, `is_resource_name`,
`is_external_package`, and the `Target` form itself (wavelet files now declare
their WIT includes directly). De-duplicate the target tests in `src/wit.rs` so
synthesized WIT and emitted WIT share one path.

**Done when.** `cargo test` and `./scripts/regen-examples.sh` green; `rg` finds no
remaining references to the deleted symbols or to `Target`; `wit.rs` no longer
duplicates target logic.

**Handoff notes.** *(fill in)*

---

## Step 12 — Composition workflow via `wac`

- [ ] Done

**Goal.** Make `wavelet build` produce **one** final composed artifact: generate
a `.wac` file describing how the project's components (and any bundled dependency
components) wire together, and run `wac compose` via the Step 0 wrapper. Host
imports (`wasi:*`) are left unsatisfied for the runtime to provide. Optionally
verify with `wac targets`.

**Scope.**

- `.wac` generation + `wac` invocation in `wavelet build`.
- Add the integration tests that actually **build and serve** both the `cli` and
  `http` templates end-to-end (today's template tests only assert text).
- Multi-component composition (the `demo-main` + `demo-shout` shape) covered by a
  test.

**Done when.** `cargo test` green including the new build-and-serve integration
tests; a multi-component project composes to a single component.

**Handoff notes.** *(fill in)*

---

## Step 13 — Docs prose & layout

- [ ] Done

**Goal.** Update the docs prose (`docs/`) for the new world: the project layout
(`wit/`, `wkg.lock`), the `wkg`/`wac` dependencies, explicit WIT includes, and
the removal of the builtins and `Target`.

**Done when.** `cargo test` and `./scripts/regen-examples.sh` green; docs prose
matches the new behaviour; no stale references to the removed builtins/`Target`
remain in `docs/`.

**Handoff notes.** *(fill in)*

---

## Step 14 — Syntax highlighting (Prism / Neovim / VS Code)

- [ ] Done

**Goal.** Drop `Target` and the removed builtins from the three highlighting
grammars' token/keyword lists where present, keeping them in sync with the lexer.

**Scope.**

- `docs/src/prism/wavelet.js`, `tooling/neovim/syntax/wavelet.vim`,
  `tooling/vscode/`.
- The `tooling/neovim` submodule is a separate repo (`logaan/wavelet.nvim`):
  ensure it's checked out (`./scripts/init-submodules.sh`), edit inside
  `tooling/neovim/`, commit **and push** there, then bump the submodule pointer
  here (`git add tooling/neovim`).

**Done when.** All three grammars match the current lexer; the submodule pointer
is bumped; `cargo test` green.

**Handoff notes.** *(fill in)*

---

## Step 15 — LSP

- [ ] Done

**Goal.** Update the LSP (`tooling/`) so import resolution learns about external
WIT packages under `wit/deps`, and diagnostics/completion stop offering the
removed builtins.

**Done when.** The LSP no longer surfaces the removed builtins and resolves
`wit/deps` imports; `cargo test` green.

**Handoff notes.** *(fill in)*

---

## Step 16 — CHANGELOG & design notes

- [ ] Done

**Goal.** Record all breaking changes under `## [Unreleased]` in `CHANGELOG.md`,
and fold the decoupled design into `dev-notes/design.md` / `dev-notes/notes.md`.

**Done when.** CHANGELOG `## [Unreleased]` lists the removals (`Target`, the
builtins), the new `wkg`/`wac` dependencies, and the new project layout; design
docs reflect the decoupled architecture; `cargo test` green.

**Handoff notes.** *(fill in)*

---

## Step 17 — Cut the breaking release

- [ ] Done

**Goal.** Once every box above is ticked, cut the breaking release per
`CLAUDE.md` — this is an ordinary subagent step like the others, spawned and run
by an agent.

**Scope.**

- Rename `## [Unreleased]` to `## [X.Y.Z] - <date>` and add a fresh empty
  `## [Unreleased]`.
- Bump the version in `Cargo.toml` *and* `tooling/wavelet-lsp/Cargo.toml` to
  match.
- Update the compare-link footnotes at the bottom of `CHANGELOG.md`.
- Confirm `scripts/changelog-section.sh vX.Y.Z` prints the right section before
  tagging.
- Tag `vX.Y.Z` and push the tag so the `Release` workflow publishes.

**Done when.** `cargo test` green; `scripts/changelog-section.sh vX.Y.Z` prints
the new section; the `vX.Y.Z` tag is pushed.

**Handoff notes.** *(fill in)*
