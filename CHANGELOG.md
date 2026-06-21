# Changelog

All notable changes to Wavelet are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This file is the source of truth for GitHub release notes: on a `v*` tag the
release workflow extracts the matching version's section below and uses it as
the release body (see `.github/workflows/release.yml` and
`scripts/ci/changelog-section.sh`). Keep the `[Unreleased]` section up to date as
you work, and rename it to the new version when you cut a release.

## [Unreleased]

### Added

- **Macro components — run macros defined in other components.** `Import {pkg:
  "…" macros: true}` imports a *macro library*: a component exporting
  `wavelet:meta/macros@0.1.0` (`manifest()` → `(name, arity)` pairs, `expand(name,
  args)` → `result<tree, string>`). Wavelet instantiates it **at compile time**,
  registers its macro arities with the reader (so foreign TitleCase macros read
  paren-free, top-to-bottom, like local `DefMacro`s), and routes expansion
  through the component's `expand` to a fixpoint (design.md §6.2–§6.3). The
  library's `.wasm` is resolved from an explicit `from:` path or the conventional
  `wit/macros/<ns>-<name>.wasm` location. A `macros: true` import is
  compile-time-only: it is excluded from the synthesized runtime world, so the
  shipped component never imports `wavelet:meta/macros`. Because a macro library
  is an ordinary component, macros can be written in any language that compiles to
  one, and they run **sandboxed by construction**.
- **Qualified and aliased foreign macros.** Imported macros are available both
  bare (`Element`) and qualified by the import alias (`dsl/Element`, with
  `as:`). A bare TitleCase name provided by two imports — or by an import and a
  local `DefMacro` — is ambiguous and errors **only when used bare**, with a
  message suggesting you alias or qualify; the qualified spelling always
  resolves.
- **Build-time component runtime.** Running foreign macros at compile time needs
  a wasm runtime, so the compiler now depends on `wasmtime` (native targets
  only). It is gated out of the `wasm32` build: the browser-playground bindings
  moved behind a new default-on `playground` cargo feature, so the
  wasm-compiled interpreter (and the macro-library guest) build without
  `wasmtime` or `wasm-bindgen`. The playground therefore has **no** component
  runtime — foreign-macro examples run only in native `wavelet build`, not in
  the browser.
- **Produce macro components from Wavelet (`wavelet build`).** A `.wvl` file
  whose top level is a `Package` declaration plus `DefMacro`s only (no `Export`,
  no runtime defs) now compiles into a component exporting `wavelet:meta/macros`,
  so a macro library can be **written in Wavelet itself** and imported with
  `Import {… macros: true}` like any other macro component (design.md §6.3). The
  produced component bundles the Wavelet interpreter and runs the macros through
  it, so its expansions match local expansion exactly. Built for
  `wasm32-unknown-unknown` (no WASI), so it instantiates under the capability-free
  macro linker.

### Fixed

- **Compiled `add`/`sub`/`mul`/`div`/`rem`/`neg` and `lt`/`le`/`gt`/`ge` now
  match the interpreter on floats and strings.** The wasm backend previously
  unboxed every operand as an integer, so a compiled component that did `f64`
  arithmetic or compared `string`s built cleanly but trapped at runtime. These
  builtins now dispatch on the operand at runtime: arithmetic does `f64` math
  when an operand is a decimal (widening integers in mixed operands), and the
  comparisons order strings lexicographically and numbers as `f64` — the same
  results the interpreter (the semantics oracle) produces.
- **Compiled arithmetic is strictly binary.** `add`/`sub`/`mul`/`div`/`rem` in a
  compiled component now require exactly two arguments, as the interpreter
  already did, instead of silently folding three or more.
- **Compiled integer overflow is checked.** `add`/`sub`/`mul` overflow and the
  `div`/`rem` edge cases (divide-by-zero, `INT_MIN / -1`, `INT_MIN % -1`) now
  trap in a compiled component, matching the interpreter's checked-arithmetic
  errors instead of wrapping.
- **`-inf` is matched as a whole word.** The lexer no longer splits a longer
  token beginning with `-inf` (e.g. `-info`, `-infinity`) into `-inf` plus a
  trailing fragment, mirroring the whole-word match the positive `inf`/`nan`
  literals already used.
- **`wavelet run` reports a missing entry point.** Running a file with no
  no-argument `run` closure now prints an actionable error instead of silently
  doing nothing.
- **`u64` parameters reject negatives.** The interpreter's dynamic type check for
  a `u64` parameter now rejects negative integers, consistent with the `to-u64`
  builtin.

## [0.7.0] - 2026-06-16

### Added

- **Call chaining `recv.name(args)`.** A `.` attached to a form, then a name and
  an attached `(`, rewrites the receiver into the call's first argument:
  `1.increment()` reads as `(increment, 1)`, and
  `foo(1 2 3).bar(4 5 6).baz(7 8 9)` as
  `(baz, (bar, (foo, 1, 2, 3), 4, 5, 6), 7, 8, 9)`. Chains fold left-to-right.
  This is pure reader rewriting, not method dispatch — `1.increment()` is
  exactly `increment(1)`. Like the attachment rule it is whitespace-sensitive:
  the `.`, name, and `(` must each abut the token before them. (`1.5` is still a
  decimal — a `.` is only a chain dot when no digit follows.)

### Changed

- **TitleCase macro sugar no longer spreads internal capitals.** A TitleCase
  head is now lower-cased *whole* before gaining its `-MACRO` suffix, instead of
  inserting a hyphen at each interior capital: `DefMacro` ↦ `defmacro-MACRO` and
  `DefType` ↦ `deftype-MACRO` (previously `def-macro-MACRO` / `def-type-MACRO`),
  while single-word heads like `If` ↦ `if-MACRO` are unchanged. A multi-word
  user macro must now be defined under the single lower-case word it is invoked
  as — e.g. define `DefMacro trylet …` and invoke it as `TryLet …` (previously
  `DefMacro try-let …`). Detection is unchanged: a token is a macro head when it
  starts with a capital and contains at least one lower-case letter.
- **Function calls are now WAVE tuples with the head first.** `foo(1 "baz")`
  reads and prints as `(foo, 1, "baz")` (previously the variant case `foo`
  carrying a payload). Special forms and macros share the shape: `If c t e` is
  `(if-MACRO, c, t, e)`, and `If(c t e)` reads identically. Evaluating any
  parenthesized form is a call — its head is resolved and applied to the bundled
  arguments (0 ⇒ the empty tuple, 1 ⇒ that value, ≥2 ⇒ a tuple) — so a literal
  tuple **value** now comes only from `Quote` or a builtin. `(foo)` is a
  zero-argument call (parenthesized grouping is gone), and `()` is the empty
  tuple (an error if evaluated). `form-kind` reports `tup` for a quoted call;
  `call` is now only a runtime variant carrying a payload (`ok(1)`).
- **`wavelet read` now defaults to stdin when given no file argument.** `echo
  '...' | wavelet read` reads the program from standard input; passing a path
  still reads that file. Previously stdin required an explicit `/dev/stdin`.

### Removed

- **List and record call sugar `foo[a b]` and `foo{k: v}`.** Write `foo([a b])`
  and `foo({k: v})` instead. Only `(` attaches to a name now; attaching `[` or
  `{` to a name is a read error that points at the new spelling. (Free-standing
  `[…]` list and `{…}` record/flag values are unaffected.)
- **`///` doc comments.** A `///` line is now an ordinary `//` comment: its text
  is discarded by the reader instead of attaching to the following form, and it
  no longer appears as a WIT doc comment in `wavelet wit` output or on LSP hover.

## [0.6.0] - 2026-06-15

WASI decoupling: Wavelet no longer special-cases any WASI interface. The
compiler vendors no WASI WIT and has no built-in `wasi:cli`/`wasi:http`
knowledge; a component declares the host interfaces it imports and exports
explicitly, and their WIT is fetched into the project by `wkg`. These are
breaking changes.

### Added

- **`wkg` and `wac` are now runtime dependencies of `wavelet build`/`wavelet
  new`.** `wkg` (the WebAssembly package tooling) fetches host WIT, and `wac`
  (the WebAssembly composition tool) composes components. Both must be on
  `PATH` (the Homebrew formula declares them; or `cargo install wkg wac-cli`).
  The interpreter (`wavelet run`) and `cargo test` do not need them.
- **Project layout with a `wit/` directory.** A project now carries its WIT
  package and fetched dependencies on disk: `wit/` holds the synthesized world,
  `wit/deps/` holds the host/dependency WIT that `wkg` fetched, and `wkg.lock`
  pins the dependency versions. `wavelet new` scaffolds `wit/` and runs `wkg
  wit fetch` to populate `wit/deps/` and write `wkg.lock`.
- **`wavelet build` now composes into a single artifact.** It generates a
  `.wac` describing how the project's components wire together and runs `wac
  compose` to produce one composed `out/app.wasm`, with host (`wasi:*`) imports
  left unsatisfied for the runtime to provide. `wavelet compose` remains as the
  manual/explicit alternative.
- **Output and arguments now go through explicitly-imported WASI interfaces.**
  A program that wants stdout/args imports `wasi:cli/stdout`,
  `wasi:cli/environment`, and `wasi:io/streams` (as ordinary `Import` forms)
  and drives them through the generic canonical-ABI bridge, exactly as the
  `http` template imports `wasi:http/types` + `wasi:io/streams`. The cli
  template was migrated to this shape.
- The generic canonical-ABI bridge now lowers/lifts every non-resource WIT
  value kind and resource handles/methods/drop driven by a parsed WIT
  signature, so an arbitrary host or third-party interface can be imported and
  exported without compiler-side special-casing.

### Removed

- **The `Target` special form is gone.** A file no longer adopts a host world
  with `Target "wasi:cli/command"`; instead it exports that world's interface
  directly, e.g. `Export {iface: "wasi:cli/run" name: run result: result}`.
  A source file using `Target` now fails to read.
- **The `print`, `println`, `args`, `read-line`, and `env` builtins are gone.**
  There is no built-in I/O path; output and argument access happen by importing
  and calling the relevant WASI interfaces (see Added). `wavelet run`
  interprets pure cross-component logic only and produces no program output.
- The vendored WASI WIT (`src/wasi-http.wit`) and all the hand-coded WASI
  magic (the `http/*` intrinsics, the `wasi:cli/command` target translation,
  the forced `wasi:io/streams` import) were removed; host WIT now comes from
  `wit/deps`.

## [0.5.0] - 2026-06-14

### Added

- **WASI HTTP support.** A Wavelet component can now implement the
  `wasi:http/proxy` interface and be served by `wasmtime serve`. Targeting
  `wasi:http/proxy` and exporting `wasi:http/incoming-handler` synthesizes a
  world that imports the host `wasi:http/types` (+ `wasi:io/streams`) and
  exports the handler; the released WASI 0.2.0 WIT (io + clocks + http) is
  vendored in `src/wasi-http.wit`.
- Resource handles (`own<T>`/`borrow<T>` and the wasi resource types) in the
  wasm backend, carried as opaque i32 handles across the canonical ABI.
- `http/*` intrinsics wrapping the wasi:http response pipeline — `fields`,
  `outgoing-response`, `body`, `write` (write + flush + drop the child stream),
  `set`, `finish`, and `path-with-query` — so the source reads like ordinary
  calls.
- The `--type=http` template now builds and runs end to end: a stateless page
  that greets via the `greeting` domain component (across the boundary) and
  echoes the request path. `scripts/serve.sh` serves it with `wasmtime serve`.

### Changed

- The `http` template's domain model is the shared `greeting` component
  (`src/greeting.wvl`), replacing the previous (non-building) counter.

## [0.4.0] - 2026-06-14

### Added

- `wavelet new --type=cli` scaffolds a `wasi:cli/command` program: `src/main.wvl`
  exports `run` and greets its first argument, delegating to the pure `greet`
  function in `src/greeting.wvl`, with `scripts/build.sh` + `scripts/run.sh`.

### Changed

- `wavelet new` now defaults to `--type=cli` (was `--type=http`).

## [0.3.0] - 2026-06-14

### Added

- `wavelet new <name>` scaffolds a new project: a `.gitignore`, a `src/` with a
  `wasi:http/incoming-handler` front end and the domain model it imports, and
  `scripts/build.sh` + `scripts/serve.sh`. `--type=http` selects the template
  and is the default.

## [0.2.5] - 2026-06-14

### Added

- Release builds now publish the `wavelet` CLI as well (previously only
  `wavelet-lsp`), for macOS (arm64 and x86_64) and Linux (x86_64 and arm64),
  plus a per-platform `wavelet-<target>.tar.gz` bundle consumed by the Homebrew
  formula.

### Changed

- The Homebrew formula now installs prebuilt binaries instead of building from
  source, so `brew install logaan/tap/wavelet` no longer fetches a Rust
  toolchain (`--HEAD` still builds from source).
- The release workflow no longer fails the whole release when one target's
  build leg fails; it publishes whatever binaries succeeded.

## [0.2.4] - 2026-06-14

### Added

- Homebrew install path: `brew install logaan/tap/wavelet`, documented in the
  README.

## [0.2.3] - 2026-06-14

### Added

- `--version` flag on both `wavelet` and `wavelet-lsp`.
- `scripts/coverage.sh` for `cargo-llvm-cov` test-coverage reports.
- `scripts/install.sh` to symlink `wavelet` and `wavelet-lsp` into `~/bin` for
  local development.

### Changed

- Synced the `wavelet` and `wavelet-lsp` crate versions to 0.2.3.
- Bumped the GitHub Actions runners to the Node 24 action versions.

## [0.2.2] - 2026-06-14

### Added

- `scripts/init-submodules.sh` to check out the `tooling/neovim` submodule on a
  fresh clone.
- MIT license.

### Changed

- Moved the Neovim plugin out into the standalone `logaan/wavelet.nvim`
  repository, tracked here as the `tooling/neovim` submodule.
- The docs site now also deploys on `v*` tags.

## [0.2.1] - 2026-06-13

### Changed

- Build the docs site only for releases rather than on every push.
- Dropped the Apple x86_64 target from the release matrix.

## [0.2.0] - 2026-06-13

### Added

- `wavelet-lsp` language server providing diagnostics, completion, and hover
  backed by the interpreter's reference semantics.
- The language server is bundled into both the VS Code and Neovim editor
  packages, and published as per-platform standalone binaries on each release.

## [0.1.0] - 2026-06-13

Initial release.

### Added

- The full `read → expand → interpret/analyze → emit → componentize` compiler
  pipeline: WAVE lexer/reader/desugarer with a canonical printer, ahead-of-time
  macro expansion to fixpoint, a tree-walking interpreter (the language's
  reference semantics) with macros and multi-file runs, WIT world synthesis
  (`wavelet wit`), and wasm emission + componentization + composition.
- Interpreter-backed REPL.
- Canonical-ABI emission across component boundaries for records, variants,
  tuples, `option`, `result`, `list<T>`, and string fields in aggregates.
- First-class closures via a funcref table, plus a `to-string` builtin.
- `expand` builtin (one macro-expansion step on a form value).
- `///` doc comments that attach to the following form.
- Grouped exports landing in a named interface.
- Editor syntax-highlighting tooling for Vim and VS Code, published as release
  artifacts.
- Docusaurus documentation site with a live, wasm-compiled `<Playground>`.

[Unreleased]: https://github.com/logaan/wavelet/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/logaan/wavelet/compare/v0.6.0...v0.6.0
[0.5.0]: https://github.com/logaan/wavelet/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/logaan/wavelet/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/logaan/wavelet/compare/v0.2.5...v0.3.0
[0.2.5]: https://github.com/logaan/wavelet/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/logaan/wavelet/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/logaan/wavelet/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/logaan/wavelet/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/logaan/wavelet/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/logaan/wavelet/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/logaan/wavelet/releases/tag/v0.1.0
