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
- http no-regression check — `wavelet new --template http <tmp>`, `wavelet build`
  it, and confirm the produced component serves (mirror the existing http build
  test added in commit `b86badb`).

---

## Step 0 — Tooling: require and shell out to `wkg` / `wac`

- [ ] Done

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

**Handoff notes.** _(fill in)_

---

## Step 1 — Consume external WIT from `wit/deps` (no behaviour change)

- [ ] Done

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

**Handoff notes.** _(fill in)_

---

## Step 2 — `wkg` populates `wit/` + `wkg.lock`

- [ ] Done

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

**Handoff notes.** _(fill in)_

---

## Step 3 — Generic bridge: primitives, flattening, retptr, records, tuples

- [ ] Done

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

**Handoff notes.** _(fill in)_

---

## Step 4 — Generic bridge: lists, strings, options, results, enums, variants, flags

- [ ] Done

**Goal.** Extend the Step 3 lowering to the remaining value types: lists and
strings (memory allocation/copy via `cabi_realloc`), `option`, `result`, `enum`,
`variant`, and `flags`. Still alongside the magic.

**Scope.**
- Add these kinds to the generic lowering.
- Prove it: extend the synthetic test interface to exercise each kind through the
  generic path; re-encode cleanly.

**Done when.** `cargo test` passes; functions over the full non-resource type set
compile via the generic bridge and validate; magic untouched and green.

**Handoff notes.** _(fill in)_

---

## Step 5 — Generic bridge: resource handles (own/borrow)

- [ ] Done

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

**Handoff notes.** _(fill in)_

---

## Step 6 — Generic bridge: resource methods + drop

- [ ] Done

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

**Handoff notes.** _(fill in)_

---

## Step 7 — Generic export of arbitrary interfaces

- [ ] Done

**Goal.** Export an arbitrary interface (e.g. `wasi:http/incoming-handler`,
`wasi:cli/run`) using the parsed WIT signature of the target, generalising
`is_external_iface` / `external_versioned` (`src/emit.rs:569`–`577`, `2535`) with
no `is_command`/`is_http` branch. Still alongside the magic.

**Done when.** `cargo test` passes; a hand-written component can export an
interface through the generic export path in a test; the `run`-specific
`() -> result` wrapper is reproducible as "export this function into
`wasi:cli/run` with its WIT signature." Magic untouched.

**Handoff notes.** _(fill in)_

---

## Step 8 — Cut http over to the generic path

- [ ] Done

**Goal.** Route the `wasi:http/proxy` template/components through the generic
import bridge + generic export end-to-end, with WIT coming from `wit/deps`
(`wkg`), while leaving the magic code physically present but unused for http.

**Done when.** `cargo test` passes; the http template builds **and serves**
through the generic path (this is the no-regression gate); the http magic is now
dead code reachable only by removal in Step 11.

**Handoff notes.** _(fill in)_

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

**Handoff notes.** _(fill in)_

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

**Handoff notes.** _(fill in)_

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

**Handoff notes.** _(fill in)_

---

## Step 12 — Composition workflow via `wac`

- [ ] Done

**Goal.** Make `wavelet build` produce **one** final composed artifact: generate
a `.wac` file describing how the project's components (and any bundled dependency
components) wire together, and run `wac compose` (or `wac plug` for the simple
single-plug case) via the Step 0 wrapper. Host imports (`wasi:*`) are left
unsatisfied for the runtime to provide. Optionally verify with `wac targets`.

**Scope.**
- `.wac` generation + `wac` invocation in `wavelet build`.
- Add the integration tests that actually **build and serve** both the `cli` and
  `http` templates end-to-end (today's template tests only assert text).
- Multi-component composition (the `demo-main` + `demo-shout` shape) covered by a
  test.

**Done when.** `cargo test` green including the new build-and-serve integration
tests; a multi-component project composes to a single component.

**Handoff notes.** _(fill in)_

---

## Step 13 — Docs prose & layout

- [ ] Done

**Goal.** Update the docs prose (`docs/`) for the new world: the project layout
(`wit/`, `wkg.lock`), the `wkg`/`wac` dependencies, explicit WIT includes, and
the removal of the builtins and `Target`.

**Done when.** `cargo test` and `./scripts/regen-examples.sh` green; docs prose
matches the new behaviour; no stale references to the removed builtins/`Target`
remain in `docs/`.

**Handoff notes.** _(fill in)_

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

**Handoff notes.** _(fill in)_

---

## Step 15 — LSP

- [ ] Done

**Goal.** Update the LSP (`tooling/`) so import resolution learns about external
WIT packages under `wit/deps`, and diagnostics/completion stop offering the
removed builtins.

**Done when.** The LSP no longer surfaces the removed builtins and resolves
`wit/deps` imports; `cargo test` green.

**Handoff notes.** _(fill in)_

---

## Step 16 — CHANGELOG & design notes

- [ ] Done

**Goal.** Record all breaking changes under `## [Unreleased]` in `CHANGELOG.md`,
and fold the decoupled design into `dev-notes/design.md` / `dev-notes/notes.md`.

**Done when.** CHANGELOG `## [Unreleased]` lists the removals (`Target`, the
builtins), the new `wkg`/`wac` dependencies, and the new project layout; design
docs reflect the decoupled architecture; `cargo test` green.

**Handoff notes.** _(fill in)_

---

## Step 17 — Cut the breaking release

- [ ] Done

**Goal.** Once every box above is ticked, cut the breaking release per
`CLAUDE.md`. The agent does this — no human is required.

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

**Handoff notes.** _(fill in)_
