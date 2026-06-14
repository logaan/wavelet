# Plan: decouple the compiler from WASI

Status: draft, written against **0.5.0** (which added the `wasi:http/proxy`
support). Nothing here is implemented yet.

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
- External WIT and dependency components are fetched and composed with standard
  Component-Model tooling (`wkg` for packages, `wac` for composition) rather
  than vendored into the compiler binary.
- The language still supports the *types* those interfaces require (resources,
  variants, results, …) generically, not via name allowlists.
- A pure component that imports nothing compiles to a bare component with no WASI
  in its world.

This matches intent already recorded in `dev-notes/notes.md` ("By default
wavelet should be working with absolutely no wasi"; "Where is it getting the
definition of `wasi:cli/command` from?"; "If we weren't targeting wasi could we
compile to bare components?").

## Decisions driving this plan

- **No magic worlds.** Drop the `Target` form entirely. A Wavelet file declares
  the WIT worlds/interfaces it includes directly; there are no compiler-known
  worlds.
- **No WASI-flavoured builtins.** `print`, `println`, `args`, `read-line`, and
  `env` are removed from the language. Output, args, etc. become ordinary calls
  into imported WIT interfaces, provided by the wider ecosystem rather than the
  compiler.
- **External WIT comes from `wkg`.** The compiler does not vendor WASI WIT.
  Dependency WIT is fetched into a project `wit/` tree (with a `wkg.lock` lock
  file) by `wkg`, and parsed with `wit-parser`.
- **Composition uses `wac`.** A built project is assembled into a single final
  component by `wac`, wiring the project's own components (and any bundled
  dependency components) together.
- **No regression.** The http template must keep building and serving at every
  step; http must work through the *generic* WIT path before the hand-coded path
  is deleted.
- **Breaking is fine.** This is pre-1.0. Land it as one breaking release; no
  deprecation window for the magic worlds.

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
coupling and are being removed (see "Decisions").

### E. The interpreter is not the http oracle

`interp.rs` has no `http/*` intrinsic support — the http codegen in `emit.rs`
has **no interpreter counterpart**. Today this is a coupling smell, but the fix
is *deletion*, not adding http to the interpreter: once http is just generic
calls into an imported WIT interface (step 2), there are no http-specific
backend intrinsics left for the interpreter to disagree with. See "Interpreter
scope" below.

## Tooling and external dependencies

Two BytecodeAlliance CLIs become runtime dependencies of `wavelet`:

- **`wkg`** (from <https://github.com/bytecodealliance/wasm-pkg-tools>) — WIT
  package management. Used to fetch dependency WIT into a project's `wit/` tree
  and maintain a `wkg.lock` lock file. Relevant subcommands:
  - `wkg wit fetch` — reads the world(s) defined in `wit/`, downloads their
    dependencies into `wit/deps/`, and writes/updates a lock file. Use
    `--type wit` so deps land as WIT text that `wit-parser` can read.
  - `wkg wit build` — builds the `wit/` directory into a single WIT package
    binary, fetching and embedding deps and generating a lock file. Useful when
    we want one self-contained WIT artifact rather than a `deps/` tree.
  - `wkg wit update` — refreshes the lock file to latest dependencies.
  - `wkg get` — fetch a single package (`wasi:http@0.2.0`, …) when needed.
- **`wac`** (`wac-cli`, from <https://github.com/bytecodealliance/wac>,
  `cargo install wac-cli`) — component composition. `wkg` has no compose command,
  so this is a separate tool. Relevant commands:
  - `wac plug <socket>.wasm --plug <impl>.wasm -o out.wasm` — plug a plug
    component's exports into a socket component's imports (simple case; more than
    one `--plug` allowed).
  - `wac compose [--deps-dir <dir>] [-d pkg=path] -o out.wasm composition.wac` —
    full composition driven by a `.wac` source file (the WAC language) for
    multi-component / transitive wiring. Deps resolve from a deps directory
    (default `deps/`, layout `deps/<namespace>/<package>.wasm`) or via
    `--dep name:pkg=path.wasm`. By default referenced deps are **embedded** in
    the output; `--import-dependencies` leaves them as imports instead.
  - `wac targets <component>.wasm <world>` — verify a built component targets a
    given world; useful as a build-time conformance check.

The Homebrew formula for `wavelet` must declare **`wkg` and `wac`** as
dependencies so they are present wherever `wavelet build` / `wavelet new` run.

### Two distinct kinds of "dependency"

The plan deliberately separates them:

1. **WIT interface definitions** (types and signatures) needed at *compile
   time* to know what an import/export looks like — e.g. `wasi:http` defines
   `incoming-handler`, `outgoing-response`, etc. These are fetched by `wkg` into
   `wit/deps/` and parsed by the compiler.
2. **Component implementations** to be linked into the *final artifact* — e.g. a
   sibling Wavelet component, or a third-party component. These are wired by
   `wac`.

A host interface such as `wasi:http` is kind (1) at compile time but is *not*
composed in: it stays an import in the final component, satisfied by the host
runtime (wasmtime, a server, etc.). `wac` composes only the project's own
components and any bundled dependency components.

## Proposed architecture

The throughline: replace every hand-coded WASI lowering with **one generic
canonical-ABI bridge** that can call/implement an arbitrary WIT function (incl.
resource methods), driven by parsed WIT. CLI and HTTP then stop being compiler
features and become ordinary uses of that bridge, with their WIT supplied by
`wkg` and their final wiring by `wac`.

### 1. A general source of external WIT packages (via `wkg`)

The compiler needs WIT for packages it does not compile (`wasi:io`, `wasi:cli`,
`wasi:http`, `wasi:keyvalue`, a third party's package, …).

- A project carries a `wit/` directory. The world(s)/interfaces a Wavelet file
  includes determine the package's dependencies; `wkg wit fetch` populates
  `wit/deps/` and a `wkg.lock`.
- Resolve an `Import` package by: (a) a sibling Wavelet file in the build set
  (today), else (b) an external WIT package found under `wit/deps`. Parse with
  `wit-parser` (already a dependency).
- Feed parsed external interfaces into the same `Dep`-shaped structure the
  emitter already consumes for Wavelet deps — so the import loop has one path,
  not an `is_external_package` fork.
- Delete the vendored `WASI_PACKAGES` and `wasi-http.wit` blobs; all dependency
  WIT now comes from `wit/deps` populated by `wkg`.

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
  allowlist (`src/emit.rs:127`). This is the largest chunk of work and must be
  delivered with the decoupling so http keeps working through the generic path
  (no regression).
- A `kv/get`, `http/write`, or `acme/frobnicate` call all compile the same way.

### 3. Generic export of arbitrary interfaces

Exporting `wasi:http/incoming-handler` or `wasi:cli/run` should use the parsed
WIT signature of the target interface, with no `is_command`/`is_http` branch.
The existing `is_external_iface` export naming (`src/emit.rs:2535`) generalises
cleanly; the `run`-specific `() -> result` wrapper goes away once `run` is just
"export this function into `wasi:cli/run` with its WIT signature."

### 4. Remove `Target`, the magic worlds, and the WASI builtins

- **Drop `Target`.** A Wavelet file declares the WIT worlds/interfaces it
  includes directly (the include set drives `wkg` dependency resolution and the
  synthesized world). There is no compiler-known target world.
- **Delete the special cases.** Remove `is_command`, `is_http`, the forced
  `wasi:io/streams` import, `WASI_PACKAGES`, `WASI_HTTP_WIT`, `http_imports`,
  `http_call`, and the duplicated target tests in `src/wit.rs`. `Features`
  shrinks to just cross-component call discovery.
- **Remove the WASI builtins.** Delete `print`, `println`, `args`, `read-line`,
  and `env` from `builtins.rs` and the interpreter. A program that wants stdout
  imports `wasi:cli/stdout` (or an ecosystem wrapper) and calls it through the
  generic bridge like any other interface.

### 5. Build and project workflow (`wavelet new` / `wavelet build`)

The compiler pipeline (`read → expand → interpret/analyze → emit →
componentize`) gains WIT resolution before emit and composition after
componentize:

- **`wavelet new`** scaffolds a project that includes the relevant WIT, then
  runs `wkg wit fetch` to populate `wit/deps/` and write `wkg.lock`, so a fresh
  project has its dependency WIT pinned.
- **`wavelet build`**:
  1. Synthesizes the project's own WIT (the world a Wavelet file implements plus
     its imports) into the `wit/` directory — the same world `wavelet wit`
     prints, so synthesized and emitted WIT stay identical.
  2. Ensures dependency WIT is present and locked via `wkg` (`wkg wit fetch`,
     respecting/updating `wkg.lock`).
  3. Parses the full WIT (project + `wit/deps`) and emits each component (core
     wasm → componentize against its world) through the generic bridge.
  4. Generates a `.wac` file describing how the project's components (and any
     bundled dependency components) wire together, and runs `wac` (`wac compose`,
     or `wac plug` for the simple single-plug case) to produce **one** final
     composed component artifact. Host imports (`wasi:*`) are left unsatisfied in
     that artifact for the runtime to provide. `wac targets` can verify the
     result against its intended world.

### Interpreter scope

The interpreter remains the semantics oracle, but it is *not* given native WASI
shims. There is no longer any backend-only intrinsic (no `http/*` codegen
without an interpreter counterpart) for it to diverge from: after step 2 the
backend only lowers generic WIT calls. `wavelet run` evaluates pure programs and
generic logic; calls into *host* interfaces (WASI and friends) are out of scope
for `run` and are exercised by running the built component on a wasm runtime.
That the interpreter "can't run http" is expected and fine — the compiled code
has no built-in concept of http either.

## Downstream surfaces to update (per CLAUDE.md)

- **Scaffold** (`src/scaffold.rs`): the `cli` and `http` templates declare their
  WIT includes explicitly and rely on `wkg`-fetched deps instead of magic. Add
  integration tests that actually *build* both templates (today's tests only
  assert template text), exercising the full `wkg` + `wac` pipeline.
- **Docs** (`docs/`, `docs/scripts/gen-examples.mjs`): examples using
  `print`/`args`/`Target` change shape (those builtins and `Target` are gone);
  rewrite affected prose and examples, regenerate `docs/examples.json`, and
  re-lock `tests/examples.rs` via `./scripts/regen-examples.sh`. Document the new
  project layout (`wit/`, `wkg.lock`) and the `wkg`/`wac` dependencies.
- **`wavelet wit`** (`src/wit.rs`): must track the generic mechanism so the
  synthesized WIT and the emitted WIT stay identical; remove its duplicated
  target tests.
- **Homebrew formula**: add `wkg` and `wac` as dependencies.
- **Syntax highlighting** (Prism / Neovim / VS Code): update if removing
  `Target` and the WASI builtins changes any token classes (e.g. builtin/keyword
  lists). The `tooling/neovim` submodule must be committed/pushed and its pointer
  bumped if its grammar changes.
- **LSP** (`tooling/`): import resolution and diagnostics must learn about
  external WIT packages under `wit/deps`, and stop offering the removed builtins.
- **CHANGELOG.md**: record the (breaking) changes under `## [Unreleased]` —
  removal of `Target`, removal of `print`/`println`/`args`/`read-line`/`env`,
  the new `wkg`/`wac` dependencies, and the new project layout.
- **design.md / notes.md**: fold the resolved design into the language design.

## Suggested sequencing

Each step ends green (`cargo test`, plus `./scripts/regen-examples.sh` once
examples move).

1. **External WIT resolution.** Wire `wkg`-populated `wit/deps` into the import
   resolver and parse it into the `Dep` structure, behind the existing import
   path. Vendored `WASI_PACKAGES`/`wasi-http.wit` can stay temporarily as a
   fallback. No behaviour change.
2. **Generic bridge + generic export with real resource support** (steps 2–3).
   Prove it by making a hand-written http/cli component build through the
   *generic* path *alongside* the existing magic — http template still builds and
   serves (no regression).
3. **Delete the magic** (step 4): remove `is_command`/`is_http`, the vendored
   blobs, `Target`, and the WASI builtins; migrate templates and examples to the
   explicit form; de-duplicate `wit.rs`.
4. **Build/compose workflow** (step 5): `wavelet new` fetches via `wkg`;
   `wavelet build` synthesizes `wit/`, fetches/locks deps, emits, and composes
   the final artifact via a generated `.wac` and `wac`. Add the template
   build-and-serve integration tests.
