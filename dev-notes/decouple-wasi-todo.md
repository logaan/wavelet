# WASI-decoupling — step-by-step worklist

This is the execution checklist for `dev-notes/decouple-wasi.md`. The work is
split into steps **sized for one fresh subagent each**, so no single agent's
context has to hold the whole project. Read this file *and* `decouple-wasi.md`
before starting a step.

## How this worklist is driven

- **One subagent per step.** The orchestrator spawns a fresh agent for the next
  unchecked step, that agent does *only* that step, then stops. Do not run ahead
  into the next step — the boundaries are deliberate context-size cut points.
- **Each step branches from `origin/main` and must land on `origin/main` before
  the next agent starts.** A subagent's worktree is created fresh from
  `origin/main`, so it can only see prior steps that were actually pushed. The
  last thing every step does is push to `origin/main`.
- **Update this file as part of the step.** Tick the step's box, fill in its
  "Handoff notes" with anything the next agent needs (decisions made, surprises,
  follow-ups), and commit that change together with the step's work.
- **Every step ends green and never regresses http.** `cargo test` must pass, and
  the `http` template must still build and serve, at the end of *every* step
  until it is intentionally re-routed (Step 7) — the magic path stays in place
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

**Size.** Small. One module + formula + a couple of unit tests for the
missing-tool error path.

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

**Size.** Medium. The risk is matching the existing `Dep` representation exactly.

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
  `wkg.lock` (may need to gate on `wkg` presence / network — prefer a hermetic
  fixture or a feature-gated/integration test if registry access is unavailable
  in CI).

**Done when.** `cargo test` passes; a built sample project has `wit/deps` +
`wkg.lock`; no codegen behaviour change.

**Size.** Medium. Watch out for CI network access to the registry — keep the
unit suite hermetic and make the live-fetch path an integration test.

**Handoff notes.** _(fill in)_

---

## Step 3 — Generic canonical-ABI bridge: non-resource types

- [ ] Done

**Goal.** Add a general lowering that, given a WIT function signature from parsed
WIT, emits the core call for everything *except* resources: param flattening,
retptr results, records, tuples, lists, strings, options, results, enums,
variants, flags, primitives. This is the parameterised replacement for the
`match fname` hand-coding, built **alongside** the existing magic (not replacing
it yet).

**Scope.**
- New lowering in `src/emit.rs` driven by a parsed WIT signature.
- Prove it: make at least one real, non-resource WASI call (e.g. an
  `wasi:cli/environment#get-arguments`-style list-returning function, or a
  synthetic test interface) compile through the *generic* path and match the
  hand-coded output, behind a test or temporary flag. Do **not** delete the
  hand-coded path.

**Done when.** `cargo test` passes; a non-resource WIT call compiles via the
generic bridge and validates (`wit-component` re-encode succeeds); http/cli magic
untouched and still green.

**Size.** Large. This is the first half of the heart of the work. If it grows too
big, stop at a coherent green point and split the remainder into a new
"Step 3b" appended to this file with its own checkbox.

**Handoff notes.** _(fill in)_

---

## Step 4 — Generic bridge: resources, methods, own/borrow, drop

- [ ] Done

**Goal.** Extend the generic bridge with real resource support sourced from
parsed WIT: `WitTy::Handle` for any `resource`/`own`/`borrow`, resource method
calls, and drops — retiring the `is_resource_name` allowlist (`src/emit.rs:127`)
*for the generic path*. Still alongside the magic.

**Scope.**
- Resource handling in the generic lowering from Step 3.
- Prove it: the WASI-http operations currently hand-coded in `http_call`
  (`fields`, `outgoing-response`, `body`, `path-with-query`, `set`, `write`,
  `finish`) all compile through the *generic* path in a test, matching the magic
  output. Magic path still present.

**Done when.** `cargo test` passes; the http resource operations build through the
generic bridge in a test; existing http template still builds+serves via the
magic path (no regression).

**Size.** Large. The biggest single chunk. Split into "Step 4b" at a green point
if needed (e.g. own/borrow first, methods+drop second).

**Handoff notes.** _(fill in)_

---

## Step 5 — Generic export of arbitrary interfaces

- [ ] Done

**Goal.** Export an arbitrary interface (e.g. `wasi:http/incoming-handler`,
`wasi:cli/run`) using the parsed WIT signature of the target, generalising
`is_external_iface` / `external_versioned` (`src/emit.rs:569`–`577`, `2535`) with
no `is_command`/`is_http` branch. Still alongside the magic.

**Done when.** `cargo test` passes; a hand-written component can export an
interface through the generic export path in a test; the `run`-specific
`() -> result` wrapper is reproducible as "export this function into
`wasi:cli/run` with its WIT signature." Magic untouched.

**Size.** Medium.

**Handoff notes.** _(fill in)_

---

## Step 6 — Cut http over to the generic path

- [ ] Done

**Goal.** Route the `wasi:http/proxy` template/components through the generic
import bridge + generic export end-to-end, with WIT coming from `wit/deps`
(`wkg`), while leaving the magic code physically present but unused for http.

**Done when.** `cargo test` passes; the http template builds **and serves**
through the generic path (this is the no-regression gate); the http magic is now
dead code reachable only by removal in Step 8.

**Size.** Medium–large. This is where Steps 1–5 get proven together on the real
http template.

**Handoff notes.** _(fill in)_

---

## Step 7 — Cut cli over; remove the WASI builtins

- [ ] Done

**Goal.** Route the cli template through the generic path, and remove
`print`/`println`/`args`/`read-line`/`env` from the language
(`src/builtins.rs:18`, `:343`+ and the interpreter). CLI output now goes through
an explicitly-imported `wasi:cli/stdout` (or ecosystem wrapper) via the generic
bridge.

**Scope.**
- Remove the builtins from `builtins.rs` and `interp.rs`.
- Migrate the `cli` template and every doc/example that used them; regenerate
  examples (`./scripts/regen-examples.sh`) and re-lock `tests/examples.rs`.

**Done when.** `cargo test` and `./scripts/regen-examples.sh` both green; cli
template builds and runs through the generic path; no references to the removed
builtins remain.

**Size.** Large (touches the language surface + every example). Split docs-heavy
fallout into "Step 7b" if needed.

**Handoff notes.** _(fill in)_

---

## Step 8 — Delete the magic and `Target`

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

**Size.** Medium. Mostly deletion, but expect compile-error fallout to chase.

**Handoff notes.** _(fill in)_

---

## Step 9 — Composition workflow via `wac`

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

**Size.** Medium–large.

**Handoff notes.** _(fill in)_

---

## Step 10 — Downstream surfaces & release prep

- [ ] Done

**Goal.** Bring the remaining tracked surfaces in line and prepare the breaking
release.

**Scope (per CLAUDE.md).**
- Docs prose (`docs/`): document the new project layout (`wit/`, `wkg.lock`), the
  `wkg`/`wac` dependencies, explicit WIT includes, and the removal of the
  builtins/`Target`. Examples already regenerated in earlier steps — re-run
  `./scripts/regen-examples.sh` to be sure.
- Syntax highlighting (Prism / Neovim / VS Code): drop `Target` and the removed
  builtins from token/keyword lists if present. Remember the `tooling/neovim`
  submodule must be committed+pushed in `wavelet.nvim` and its pointer bumped
  here.
- LSP (`tooling/`): import resolution learns about `wit/deps`; stop offering the
  removed builtins.
- `CHANGELOG.md`: record all breaking changes under `## [Unreleased]`.
- `design.md` / `notes.md`: fold the decoupled design into the language design.

**Done when.** `cargo test` and `./scripts/regen-examples.sh` green; highlighting
grammars and LSP no longer surface removed symbols; CHANGELOG + design docs
updated.

**Size.** Medium, but spread across many files — split into per-surface sub-steps
("Step 10b", …) if any one agent's context gets tight.

**Handoff notes.** _(fill in)_

---

## When all boxes are ticked

Cut the breaking release per `CLAUDE.md` (rename `## [Unreleased]`, bump
`Cargo.toml` + `tooling/wavelet-lsp/Cargo.toml`, update compare-link footnotes,
confirm `scripts/changelog-section.sh vX.Y.Z`, then tag). This is a separate,
human-initiated step — do not tag a release as part of an ordinary step.
