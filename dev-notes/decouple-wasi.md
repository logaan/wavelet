# Plan: decouple the compiler from WASI

Status: draft for review. Contains **open questions for Logan** (see the end, and
inline `❓` markers). Nothing here is implemented yet.

## Goal

The Wavelet language and compiler should know nothing about WASI. Today the
compiler hard-codes `wasi:cli/command`: it special-cases stdout, argv, and the
`run` export, and ships a trimmed copy of the WASI WIT inside `emit.rs`. There is
no general path for a component to import or export an *arbitrary* host-defined
WIT interface, so the `http` scaffold template — which is supposed to be "just
implement a WIT interface" — does not actually compile.

The end state:

- A user can write a component that targets any WIT world / implements any WIT
  interface (`wasi:cli/command`, `wasi:http/proxy`, `wasi:keyvalue/...`, a
  bespoke `acme:foo/bar`, …) **without the compiler knowing that interface
  exists**.
- WASI becomes "just some WIT packages the user imports," resolved through the
  same generic mechanism as a user's own `Import`s.
- The language still supports the *types* those interfaces require (resources,
  variants, results, etc.).
- A pure component that imports nothing compiles to a bare component with no WASI
  in its world.

This matches intent already recorded in `dev-notes/notes.md` ("By default
wavelet should be working with absolutely no wasi"; "Where is it getting the
definition of `wasi:cli/command` from?"; "If we weren't targeting wasi could we
compile to bare components?").

## What is special-cased today (the inventory)

All of this lives in `src/emit.rs` unless noted. The interpreter, lexer, reader,
and WIT-synthesis prose are already largely WASI-agnostic — the coupling is
concentrated in the wasm backend plus a few builtins.

### CLI hard-coding in `emit.rs`

- `is_command = info.target.as_deref() == Some("wasi:cli/command")` —
  computed in both `emit_core_module` (`src/emit.rs:1989`) and
  `synthesize_world_wit` (`src/emit.rs:3206`). This single magic string drives
  everything below.
- **`run` → `wasi:cli/run`**: when `is_command`, the exported `run` function is
  wrapped to the signature `wasi:cli/run@0.2.0#run: func() -> result` and
  exported under that name (`src/emit.rs:2193`–`2202`); it's also filtered out of
  the synthesized `api` interface (`src/emit.rs:3212`) and re-added as
  `export wasi:cli/run@0.2.0;` in the world (`src/emit.rs:3248`).
- **`print`/`println` → `wasi:cli/stdout` + `wasi:io/streams`**: the `Features`
  scan flips `needs_stdout` whenever it sees a `print`/`println` call
  (`src/emit.rs:343`). That triggers imports of `wasi:cli/stdout@0.2.0`
  `get-stdout` and `wasi:io/streams@0.2.0`
  `[method]output-stream.blocking-write-and-flush` (`src/emit.rs:2067`–`2076`),
  the `print_str`/`println_h` helper bodies (`src/emit.rs:3029`–`3059`), and
  `import wasi:cli/stdout@0.2.0;` in the world (`src/emit.rs:3233`).
- **`args` → `wasi:cli/environment`**: `needs_env` (`src/emit.rs:344`) triggers
  the `get-arguments` import (`src/emit.rs:2078`), the `get_args` helper body
  (`src/emit.rs:3061`+), and `import wasi:cli/environment@0.2.0;`
  (`src/emit.rs:3236`).
- **Vendored WIT text**: `WASI_PACKAGES` is a hand-written string holding trimmed
  `wasi:io@0.2.0`, `wasi:cli@0.2.0` WIT, appended to the synthesized world
  whenever any of the above features fire (`src/emit.rs:3151`–`3179`,
  `src/emit.rs:3253`).

### The import resolver only knows Wavelet files

`build_files` (`src/build.rs:40`–`56`) resolves every `Import` against *other
`.wvl` files in the build set* (`index.get(&imp.package)`), failing with
"import `…` is not satisfied by any file in the build set" otherwise. There is no
notion of an external/host-provided package whose WIT comes from somewhere other
than a sibling Wavelet source file. This is exactly why the `http` template
fails:

```
$ wavelet build src/*.wvl -o out
src/app.wvl: import `wasi:http/types` is not satisfied by any file in the build set
```

`emit_core_module`'s import loop (`src/emit.rs:2080`–`2109`) likewise only looks
in `deps`, which only ever contains sibling Wavelet packages.

### WASI-flavoured builtins in the interpreter

`print`, `println`, `read-line`, `args`, `env` are first-class builtins
(`src/builtins.rs:18`); `print`/`println` call `crate::emit_output`
(`src/builtins.rs:343`–`348`) and `args` returns a list (`src/builtins.rs:358`).
These are conceptually WASI (stdout / argv) even though the native interpreter
fakes them. They are the language-level surface of the CLI coupling.

### Type-system gaps that block real host interfaces

`WitTy` (`src/emit.rs:68`) supports only: bool, the integer widths, f64, string,
`list<_>`, fully-expanded records, `option<_>`, `result<_,_>`. The wasm backend
has **no resources**, no *named* variants/enums/flags across the boundary
(`dev-notes/todo.md:166`–`168`), no tuples-as-types. `wasi:http` is built almost
entirely from **resources** (`incoming-request`, `outgoing-response`, `fields`,
`output-stream`, `response-outparam`, …). So "support the types the WIT requires"
is a real, separable workstream — see Q5.

## Proposed architecture

Four pieces. (1) and (2) are the core decoupling; (3) removes the CLI magic; (4)
is the type work that makes non-trivial host worlds (http) actually usable.

### 1. A general source of external WIT packages

The compiler needs WIT for packages it does not compile itself (`wasi:io`,
`wasi:cli`, `wasi:http`, `wasi:keyvalue`, …). The Component Model already has a
convention: a `wit/` directory with `wit/deps/<pkg>/*.wit`. Proposal:

- Resolve an `Import` package by, in order: (a) a sibling Wavelet file in the
  build set (today's behaviour), else (b) an external WIT package found on a WIT
  search path (default `wit/deps`, overridable via a flag/env).
- Parse external WIT with `wit-parser` (already a dependency) and feed the
  resulting interfaces into the same `Dep`-like structure the emitter consumes,
  instead of `dep_package_wit` re-rendering Wavelet-authored WIT by hand.
- Delete `WASI_PACKAGES`; ship the canonical WASI 0.2 WIT as vendored
  `wit/deps` instead (or resolve it however Q1 decides).

❓ See Q1 for where these WIT files should come from and how they're discovered.

### 2. Generic import/export of host interfaces in `emit.rs`

- The import loop must accept packages whose `Dep` came from external WIT, not
  just Wavelet sources. Function signatures, resource methods, and types all come
  from the parsed WIT rather than from a sibling `FileInfo`.
- Exporting an arbitrary interface (e.g. `wasi:http/incoming-handler`) already
  has a syntax (`Export {iface: "wasi:http/incoming-handler" name: handle …}` in
  the http template). The emitter must emit an export wrapper for that interface
  using the *external* WIT's signature, the same way it does for the synthesized
  `api` interface — no `is_command` branch.

### 3. Remove the `wasi:cli/command` special case

Once (1) and (2) exist, the CLI is expressible by the user:

- `Target "wasi:cli/command"` becomes the generic "this component targets WIT
  world X" — emit `include wasi:cli/command;` (or the appropriate
  import/export expansion) by reading that world from external WIT, with no
  knowledge of what `run`/stdout/environment mean.
- The `run` → `wasi:cli/run` wrapping, the `print`/`args` feature flags, the
  helper bodies, and `WASI_PACKAGES` all go away. `Features` shrinks to just
  `dep_calls`.

This raises the question of what `print`/`println`/`args` *are* once the compiler
no longer wires them to WASI — Q2.

### 4. The types real host worlds need (resources first)

To make the `http` template genuinely compile/run end-to-end (the natural
acceptance test), the wasm backend needs resource handles at minimum, plus
list<u8> bodies and named variants across the boundary. This is the largest and
most separable chunk; it can land after the decoupling and may deserve its own
plan. See Q5 for whether http-actually-works is in scope or tracked separately.

## Downstream surfaces to update (per CLAUDE.md)

- **Scaffold** (`src/scaffold.rs`): the `cli` template should stop relying on
  compiler magic — it should `Import` the WASI CLI interfaces explicitly (or via
  a std module, per Q2) once they're no longer built in. The `http` template
  should finally build. Its tests (`src/scaffold.rs` `#[cfg(test)]`) assert
  template text, not builds — consider an integration test that actually builds
  the http template (it would have caught this).
- **Docs** (`docs/`, `docs/scripts/gen-examples.mjs`): any example using
  `print`/`args`/`Target` may change shape; regenerate `docs/examples.json` and
  re-lock `tests/examples.rs` via `./scripts/regen-examples.sh`.
- **Syntax highlighting** (Prism / Neovim / VS Code): only if token classes
  change (unlikely for this work, but `Target`/`Import` semantics shifting could
  touch prose).
- **LSP** (`tooling/`): import resolution / diagnostics may need to learn about
  external WIT packages.
- **CHANGELOG.md**: record the (breaking) change to `print`/`args`/`Target`
  semantics under `## [Unreleased]`.
- **design.md / notes.md**: fold the resolved questions back into the design;
  `notes.md` already poses several of them.

## Suggested sequencing

1. External WIT resolution + generic `Dep` from parsed WIT (1), with WASI WIT
   vendored, behind the existing import path. No behaviour change yet.
2. Generic host import/export emission (2); make a hand-written CLI component
   that imports `wasi:cli/*` explicitly build *alongside* the existing magic.
3. Delete the `is_command` path, `WASI_PACKAGES`, and the print/args feature
   wiring (3); migrate the `cli` template and examples to the explicit form.
4. Resources + remaining types (4); make the `http` template build and run on
   `wasmtime serve`.

Each step ends green (`cargo test`, and `./scripts/regen-examples.sh` once
examples move).

---

## Open questions for Logan

**Q1 — Where do external WIT definitions come from?**
Options: (a) vendor the canonical WASI 0.2 WIT into the repo and resolve a
`wit/deps`-style directory in each project (Component-Model standard, offline,
explicit); (b) keep a compiler-bundled WASI snapshot but expose it through the
generic resolver (less typing for users, still couples the *binary* to a WASI
version); (c) fetch from a registry (out of scope for now?). I lean (a) for
projects + maybe (b) as a built-in fallback for the well-known `wasi:*`
packages. What do you want the default project layout to look like?

**Q2 — What happens to `print` / `println` / `args` (and `read-line` / `env`)?**
Once the compiler is WASI-agnostic, these can't secretly wire to `wasi:cli`.
Options: (a) **remove them from the language** entirely — a CLI program imports
`wasi:cli/stdout` and calls it explicitly, like any other interface (maximally
decoupled, but `print("hi")` stops being a one-liner and every example/tutorial
changes); (b) move them into an **optional std module** (e.g.
`wavelet:std/io`) that is itself authored as a thin wrapper over the WASI
interfaces and only pulls them in when imported — keeps `print` ergonomic
without the *compiler* knowing WASI, but the *std library* then depends on WASI;
(c) keep them as language builtins but require the user's world to already import
the relevant WASI interface, and lower to it generically. Which trade-off do you
want? (This is the biggest user-facing decision.)

**Q3 — What should `Target` mean?**
Is `Target "wasi:cli/command"` just sugar for `include`-ing that WIT world (and
nothing else)? Should `Target` be able to name *any* world from external WIT?
Should a component with no `Target` produce a bare component world? `notes.md`
asks "Is `Target` something that exists in wit?" — I'd map it to WIT `include`.
Confirm that's the intended semantics, or describe what you want.

**Q4 — Interpreter behaviour with no built-in WASI.**
`interp.rs` is the semantics oracle and runs natively. Today `print`/`args` are
native fakes. If they're removed/moved (Q2), how should `wavelet run` behave for
a component that imports a host interface (e.g. `wasi:cli/stdout`)? Options:
(a) the interpreter grows a small set of native shims for well-known WASI
imports (so `wavelet run` of a CLI program still prints); (b) `wavelet run` only
supports pure programs and host-backed ones must be built + run on `wasmtime`.
The decoupling goal pushes toward (a)-as-an-opt-in shim that lives *outside* the
language core. Preference?

**Q5 — Is "the http template actually builds and runs" in scope here, or
separate?**
Making http real requires **resources** (and list<u8> bodies, named variants
across the boundary) in the wasm backend — a substantial workstream
(`todo.md:172`, `notes.md:132` "Can wavelet define resources?"). Do you want
this plan to cover only the decoupling architecture (steps 1–3), with resources
+ http tracked as a follow-up plan? Or should the deliverable be "http template
compiles and serves" (steps 1–4)?

**Q6 — How aggressive about breaking changes / versioning?**
This is pre-1.0 and several of these changes are breaking (`print` semantics,
`Target` meaning, project layout). Are you happy to break them in one release
(documented in CHANGELOG), or do you want a deprecation path (e.g. keep the magic
`wasi:cli/command` working while the explicit form is introduced)?
