# Plan: decouple the compiler from WASI

Status: draft for review, rewritten against **0.5.0** (which added the
`wasi:http/proxy` support). Contains **open questions for Logan** (see the end,
and inline `❓` markers). Nothing here is implemented yet.

## Goal

The Wavelet language and compiler should know nothing about WASI. Today the
compiler hard-codes two specific WASI worlds:

- **`wasi:cli/command`** — special-cases stdout, argv, and the `run` export, and
  ships trimmed WASI CLI WIT inside `emit.rs`.
- **`wasi:http/proxy`** (new in 0.5.0) — special-cases a fixed set of
  `http/<op>` intrinsics with hand-written canonical-ABI lowerings, an allowlist
  of WASI resource type names, and a vendored 1050-line `wasi-http.wit`.

There is still **no general path** for a component to import or export an
*arbitrary* host-defined WIT interface and call its functions/methods. Each new
WASI world is bolted on by hand. The 0.5.0 http work is the clearest evidence:
making http work meant writing a bespoke lowering for `outgoing-response`,
`body`, `path-with-query`, `set`, `write`, `finish` — not teaching the compiler
to call any imported WIT function.

The end state:

- A user can write a component that targets any WIT world / implements any WIT
  interface (`wasi:cli/command`, `wasi:http/proxy`, `wasi:keyvalue/...`, a
  bespoke `acme:foo/bar`, …) **without the compiler knowing that interface
  exists** — its signatures, types, and resource methods come from parsed WIT.
- WASI becomes "just some WIT packages the user imports," resolved through the
  same generic mechanism as a user's own `Import`s.
- The language still supports the *types* those interfaces require (resources,
  variants, results, …) generically, not via name allowlists.
- A pure component that imports nothing compiles to a bare component with no WASI
  in its world.

This matches intent already recorded in `dev-notes/notes.md` ("By default
wavelet should be working with absolutely no wasi"; "Where is it getting the
definition of `wasi:cli/command` from?"; "If we weren't targeting wasi could we
compile to bare components?").

## What is special-cased today (the inventory)

All file/line references are against current `main` (0.5.0). The coupling is
concentrated in `src/emit.rs` (the wasm backend) and `src/wit.rs` (the
`wavelet wit` world synthesizer, which duplicates the same target tests), plus a
few builtins. The lexer, reader, and parser are WASI-agnostic.

### A. The `wasi:cli/command` special case

- `is_command = info.target.as_deref() == Some("wasi:cli/command")` — computed
  in `emit_core_module` (`src/emit.rs:2239`), `synthesize_world_wit`
  (`src/emit.rs:3478`), and again in `src/wit.rs`.
- **`run` → `wasi:cli/run`**: exported `run` is wrapped to
  `wasi:cli/run@0.2.0#run: func() -> result` (`src/emit.rs:2459`–`2467`),
  filtered out of the synthesized `api` interface (`src/emit.rs:3485`), and
  re-added as `export wasi:cli/run@0.2.0;` (`src/emit.rs:3538`).
- **`print`/`println` → `wasi:cli/stdout` + `wasi:io/streams`**: the `Features`
  scan sets `needs_stdout` on any `print`/`println` call (`src/emit.rs:379`),
  driving the `get-stdout` + `blocking-write-and-flush` imports
  (`src/emit.rs:2317`+), the `print_str`/`println_h` helper bodies
  (`src/emit.rs:3301`+), and `import wasi:cli/stdout@0.2.0;` (`src/emit.rs:3508`).
- **`args` → `wasi:cli/environment`**: `needs_env` drives the `get-arguments`
  import (`src/emit.rs:2328`), the `get_args` helper body (`src/emit.rs:3334`+),
  and `import wasi:cli/environment@0.2.0;` (`src/emit.rs:3511`).
- **Vendored WIT text**: `WASI_PACKAGES` is a hand-written string of trimmed
  `wasi:io`/`wasi:cli` WIT, appended when any CLI feature fires
  (`src/emit.rs:3423`+, used at `src/emit.rs:3544`).

### B. The `wasi:http/proxy` special case (new in 0.5.0)

- `is_http = info.target.as_deref() == Some("wasi:http/proxy")` — in
  `synthesize_world_wit` (`src/emit.rs:3479`) and again in `src/wit.rs:311`.
- **Hand-written intrinsic lowerings**: `http_imports(fname)`
  (`src/emit.rs:595`) is a fixed table mapping `http/<op>` to the canonical-ABI
  core import signatures for a closed set of operations (`fields`,
  `outgoing-response`, `body`, `path-with-query`, `set`, `write`, `finish`).
  `http_call(...)` (`src/emit.rs:1458`) emits the core instructions for each by
  hand. Anything else is rejected: "`http/<op>` is not a supported wasi:http
  operation". This is the bespoke path that a general WIT-function-call mechanism
  would replace.
- **External-package routing**: `is_external_package(pkg)` is literally
  `pkg.starts_with("wasi:")` (`src/emit.rs:563`). In the import loop
  (`src/emit.rs:2340`) and in `build.rs` (`src/build.rs:45`) a `wasi:*` import
  bypasses build-set dependency resolution and is sent to the `http_imports`
  table instead.
- **Resource handles by name allowlist**: `is_resource_name(s)`
  (`src/emit.rs:127`) is a hardcoded list of WASI resource type names
  (`incoming-request`, `output-stream`, `fields`, …); `wit_ty` maps any of them
  (or `own<…>`/`borrow<…>`) to `WitTy::Handle` (`src/emit.rs:152`). So the type
  system "supports resources" only for these specific WASI names.
- **Vendored WIT**: `WASI_HTTP_WIT = include_str!("wasi-http.wit")`
  (`src/emit.rs:559`), a 1050-line vendored copy of WASI 0.2.0 io+clocks+http,
  appended whenever `is_http` (`src/emit.rs:3543`). The world also force-imports
  `wasi:io/streams@0.2.0` when `is_http` (`src/emit.rs:3528`, `src/wit.rs:348`).
- **External interface export/import naming**: `is_external_iface` /
  `external_versioned` (`src/emit.rs:569`–`577`) handle exporting
  `wasi:http/incoming-handler` under its versioned name (`src/emit.rs:2535`,
  `src/emit.rs:3531`).

### C. The import resolver still can't load arbitrary external WIT

`build_files` resolves a non-`wasi:` import against sibling `.wvl` files in the
build set; a `wasi:` import is shunted to the hardcoded path above
(`src/build.rs:42`–`56`). There is no notion of loading a *parsed* external WIT
package (from a `wit/deps` directory or a registry) and calling its functions
generically. So a non-WASI host interface, or a WASI interface outside the
hand-coded http set, cannot be used at all.

### D. WASI-flavoured builtins in the interpreter

`print`, `println`, `read-line`, `args`, `env` are first-class builtins
(`src/builtins.rs:18`); `print`/`println` call `crate::emit_output`
(`src/builtins.rs:343`+). They are the language-level surface of the CLI
coupling.

### E. The interpreter is not the http oracle

`interp.rs` has no `http/*` intrinsic support — the http codegen in `emit.rs`
has **no interpreter counterpart**, so `wavelet run` cannot run an http
component (only `wavelet build` can emit one). This violates "the interpreter is
the semantics oracle" (CLAUDE.md): the wasm backend defines http behaviour with
nothing to validate it against. Decoupling should fix this asymmetry, not deepen
it.

## Proposed architecture

The throughline: replace every hand-coded WASI lowering with **one generic
canonical-ABI bridge** that can call/implement an arbitrary WIT function (incl.
resource methods), driven by parsed WIT. CLI and HTTP then stop being compiler
features and become ordinary uses of that bridge.

### 1. A general source of external WIT packages

The compiler needs WIT for packages it does not compile (`wasi:io`, `wasi:cli`,
`wasi:http`, `wasi:keyvalue`, a third party's package, …). The Component Model
convention is a `wit/` directory with `wit/deps/<pkg>/*.wit`. Proposal:

- Resolve an `Import` package by: (a) a sibling Wavelet file in the build set
  (today), else (b) an external WIT package found on a WIT search path (default
  `wit/deps`, overridable). Parse with `wit-parser` (already a dependency).
- Feed parsed external interfaces into the same `Dep`-shaped structure the
  emitter already consumes for Wavelet deps — so the import loop has one path,
  not an `is_external_package` fork.
- Delete the vendored `WASI_PACKAGES` and `wasi-http.wit` blobs; ship WASI WIT
  as resolvable `wit/deps` instead (or per Q1).

❓ See Q1 for where these WIT files come from and how they're discovered.

### 2. A generic canonical-ABI bridge (the heart of the work)

Replace `http_imports`/`http_call` and the CLI helper bodies with a general
lowering that, given a WIT function signature (from parsed WIT), emits the core
call: flatten params, handle retptr results, lower/lift records, lists,
options/results, **resources (as i32 handles), and resource methods** — exactly
what `http_call` does by hand today, but parameterised by the signature instead
of by a `match fname`. `wit-component` already re-validates core signatures
against the WIT at encode time, so this stays honest.

- Resource support becomes general: a `WitTy::Handle` is produced for any WIT
  `resource`/`own`/`borrow` from parsed WIT, retiring the `is_resource_name`
  allowlist (`src/emit.rs:127`).
- A `kv/get`, `http/write`, or `acme/frobnicate` call all compile the same way.

### 3. Generic export of arbitrary interfaces

Exporting `wasi:http/incoming-handler` or `wasi:cli/run` should use the parsed
WIT signature of the target interface, with no `is_command`/`is_http` branch.
The existing `is_external_iface` export naming (`src/emit.rs:2535`) generalises
cleanly; the `run`-specific `() -> result` wrapper goes away once `run` is just
"export this function into `wasi:cli/run` with its WIT signature."

### 4. Remove the `is_command` / `is_http` special cases

Once (1)–(3) exist, `Target "wasi:cli/command"` and `Target "wasi:http/proxy"`
are generic "this component targets WIT world X" — emit `include <world>;` from
parsed WIT and let the import/export machinery do the rest. Delete `is_command`,
`is_http`, the forced `wasi:io/streams` import, `WASI_PACKAGES`,
`WASI_HTTP_WIT`, `http_imports`, `http_call`, and the duplicated target tests in
`src/wit.rs`. `Features` shrinks to just cross-component call discovery.

This forces a decision on `print`/`println`/`args` (Q2) and on whether the
interpreter gains generic host shims (Q4).

### 5. Make the interpreter an oracle for host calls

Per CLAUDE.md, the interpreter must define the semantics the wasm backend is
validated against. Today it can't run http. Options are in Q4; whichever we
pick, the goal is that `wavelet run` and the wasm backend agree on a
host-importing program (or that we consciously scope `wavelet run` to exclude
host effects).

## Downstream surfaces to update (per CLAUDE.md)

- **Scaffold** (`src/scaffold.rs`): `cli` and `http` templates should import the
  WASI interfaces explicitly (or via a std module, per Q2) once the magic is
  gone. Add integration tests that actually *build* both templates (today's
  tests only assert template text).
- **Docs** (`docs/`, `docs/scripts/gen-examples.mjs`): examples using
  `print`/`args`/`Target` may change shape; regenerate `docs/examples.json` and
  re-lock `tests/examples.rs` via `./scripts/regen-examples.sh`.
- **`wavelet wit`** (`src/wit.rs`): currently duplicates the target tests; it
  must track the generic mechanism so synthesized WIT and emitted WIT stay
  identical.
- **Syntax highlighting** (Prism / Neovim / VS Code): only if token classes
  change (unlikely here).
- **LSP** (`tooling/`): import resolution / diagnostics must learn about
  external WIT packages.
- **CHANGELOG.md**: record the (breaking) changes under `## [Unreleased]`.
- **design.md / notes.md**: fold the resolved questions back in.

## Suggested sequencing

1. External WIT resolution + parsed `Dep` (1), WASI WIT vendored as `wit/deps`,
   behind the existing import path. No behaviour change.
2. Generic canonical-ABI bridge (2) + generic export (3); prove it by making a
   hand-written http/cli component build *alongside* the existing magic.
3. Delete the `is_command`/`is_http` paths and the vendored blobs (4); migrate
   templates and examples to the explicit form; de-duplicate `wit.rs`.
4. Interpreter oracle parity (5).

Each step ends green (`cargo test`, plus `./scripts/regen-examples.sh` once
examples move).

---

## Open questions for Logan

**Q1 — Where do external WIT definitions come from?**
(a) Vendor WASI 0.2 WIT into the repo and resolve a `wit/deps` directory per
project (Component-Model standard, offline, explicit); (b) keep a
compiler-bundled WASI snapshot exposed through the generic resolver (less typing,
but the binary stays pinned to a WASI version); (c) registry fetch (later?). I
lean (a) for projects, optionally (b) as a built-in fallback for well-known
`wasi:*` packages. What should the default project layout look like?

**Q2 — What happens to `print` / `println` / `args` (and `read-line` / `env`)?**
Once the compiler is WASI-agnostic these can't secretly wire to `wasi:cli`.
(a) Remove them from the language — a CLI program imports `wasi:cli/stdout` and
calls it like any interface (maximally decoupled; `print("hi")` stops being a
one-liner and every example changes); (b) move them into an optional std module
(e.g. `wavelet:std/io`) authored as a thin wrapper over the WASI interfaces, so
the *std library* depends on WASI but the *compiler* doesn't; (c) keep them as
builtins that lower generically but require the user's world to already import
the relevant interface. Which trade-off? (Biggest user-facing decision.)

**Q3 — What should `Target` mean?**
Is `Target "wasi:cli/command"` just sugar for `include`-ing that WIT world?
Should it name *any* world from external WIT? Should a `Target`-less component
produce a bare component world? I'd map it to WIT `include`. Confirm, or
describe what you want.

**Q4 — Interpreter behaviour with no built-in WASI.**
`interp.rs` is the oracle but already can't run http. For a component that
imports a host interface, should `wavelet run` (a) grow a small set of native
shims for well-known WASI imports (so CLI programs still print and http programs
can be exercised), living *outside* the language core; or (b) be scoped to pure
programs, with host-backed ones run only on `wasmtime`? The oracle principle
pushes toward (a). Preference?

**Q5 — How general should resource support get, and when?**
The generic bridge (step 2) needs real resource support (handles, methods,
own/borrow, drop) sourced from parsed WIT, replacing the `is_resource_name`
allowlist. This is the largest chunk. Do you want it delivered together with the
decoupling (so http keeps working through the *generic* path with no regression),
or staged — keep the hand-coded http path temporarily while the generic bridge
is built, then cut over? I recommend "no regression": the http template must
still build and serve at every step.

**Q6 — Breaking changes vs. deprecation.**
This is pre-1.0 and several changes are breaking (`print` semantics, `Target`
meaning, project layout, dropping the magic worlds). One documented breaking
release, or a deprecation window where the magic `wasi:cli/command` /
`wasi:http/proxy` keep working while the explicit form is introduced?
