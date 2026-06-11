# Wavelet implementation — todo

Tracking doc for implementing the Wavelet language (see `design.md`, draft 0.1).
Implementation language: **Rust** (decided 2026-06-12). Pipeline per §9:
read → expand → analyze → emit → componentize.

Keep this file updated: mark items `[x]` when done, add notes inline.

## Phase 0 — setup
- [x] Project folder + git repo
- [x] Copy design doc into repo as `design.md`
- [x] Install Rust toolchain (rustup 1.96.0, minimal profile, ~/.cargo/bin)
- [x] `cargo init` crate `wavelet` (lib + `wavelet` bin), builds clean

## Phase 1 — reader (all sugar dies here, §2 + Appendix A)
- [x] Lexer (`src/lexer.rs`): WAVE tokens, kebab idents (`%` escape), TitleCase
      idents, numbers, strings, chars, `//` comments, commas-as-whitespace
- [x] Form tree (`src/form.rs`) mirroring `wavelet:meta/code` (§6.2): arena
      (`Vec<Node>`) with parallel spans
- [x] Parser (`src/reader.rs`): atoms, lists, records, flags, tuples, `(x)`
      grouping transparency
- [x] Attachment rule: `f(…)`/`f[…]`/`f{…}` with no whitespace = call (§2.2)
- [x] Desugaring per §2.3 table incl. qualified calls `kv/get`
- [x] TitleCase macro sugar: kebab-ize + `-MACRO`; arity-driven reading with
      core-form arity table; `DefMacro` registers arity top-to-bottom
- [x] Explicit-payload override for TitleCase heads: `If(c t e)`, `Unquote(x)`
- [x] Canonical WAVE printer (`src/printer.rs`) — round trip stable (tested)
- [x] Reader unit tests covering every row of the §2.3 desugar table (11 tests)
- [x] `wavelet read <file>` CLI; parses §1 examples (`examples/*.wvl`) correctly
- [ ] `///` doc comments as metadata (currently skipped as plain comments)
- [ ] Qualified TitleCase macros `Dsl/Element` arity reading (parses, but arity
      lookup ignores the alias; revisit with macro imports in Phase 2)

## Phase 2 — expander (§2.4, §6.3)
- [x] Core special-form arity table (the 17 forms, §4.2) seeded into reader
- [x] `DefMacro` registration while reading top-to-bottom (define-before-use)
- [x] `Quote` / `Quasi` / `Unquote` / `Splice` semantics over form trees
- [x] `gensym`
- [x] Expansion (lazy, at eval time: a call whose head is bound to a Macro
      value expands and jumps into the result — fixpoint by re-evaluation)
- [ ] Nested `Quasi` depth handling (currently single-level, Clojure-style)
- [ ] Separate ahead-of-time expand pass (needed for the wasm backend)
- [ ] Macro components: instantiate wasm at compile time, `manifest`/`expand`
      interface, `Import {… macros: true}`

## Phase 3 — interpreter (validate semantics before emitting wasm)
- [x] Value repr = WIT value space (`src/value.rs`); structural `eq`,
      identity for closures/cells; canonical WAVE value printer
- [x] Eval rules 1–4 (§4.1); Lisp-1 lexical env (`src/interp.rs`)
- [x] Special forms: `Def`, `Fn`, `If`, `Let`, `Do`, `Match`, `Quote`, `Quasi`,
      `DefMacro`, `The` (primitive-type ascription checks)
- [x] §4.2 payload binding: record→by name, list/tuple→by order, sole param
      →direct; typed params checked at bind time
- [x] Tail-call elimination via Jump loop — verified with 200k-deep recursion
- [x] Pattern matching incl. payload-less variant cases matching by equality
      when the name is bound to one (e.g. `none`); bare names bind otherwise
- [x] Builtins (`src/builtins.rs`): predicates, arithmetic, sequences, strings,
      conversions, I/O, apply/gensym, form accessors, ok/err/some/none, cells
- [x] §7.2 `TryLet` macro works exactly as written in the spec (test)
- [x] Multi-file `wavelet run` (`src/runner.rs`): resolves `Import` by package
      id across files, honors `Export`/`as:`/`open:`, calls exported `run`
- [x] §1 example runs: `wavelet run examples/main.wvl examples/shout.wvl -- wasm`
      prints `WASM!`
- [ ] `expand` builtin (stub errors for now)
- [ ] Resource handles beyond `cell`; owned-handle drop semantics (§6.1)

## Phase 4 — module/component model surface (§6.1)
- [x] `Package`, `Target`, `Import`, `Export`, `DefType` forms parsed; handled
      by the runner (interp) and `src/wit.rs` (synthesis)
- [x] WIT world synthesis (`wavelet wit <file>`): typed `Fn` params, explicit
      `Export` record form, `DefType` records/variants/flags/aliases,
      best-effort result-type inference; shout.wvl reproduces §6.1 exactly
- [x] Type ascription `The` (primitive checks at eval time)
- [ ] Richer inference (across Defs, lists/options/results) — currently errors
      and asks for annotations when it cannot infer
- [ ] Boundary coercions + `safely` wrapper semantics (§3)
- [ ] Grouped exports `Export {iface: "render" ...}` (only default `api` now)

## Phase 5 — emit + componentize (§9)
- [ ] Pick wasm-tools crates: wasm-encoder, wit-parser, wit-component
- [ ] Analysis: binding resolution, tail-position classification, closure capture
- [ ] Emit core wasm (GC types; `return_call` for tail positions)
- [ ] Canonical-ABI lift/lower wrapping via wit-component
- [ ] `wavelet build` CLI
- [ ] `wavelet compose` (auto-plug + WAVE manifest, §6.5); `--fuse` later

## Phase 6 — beyond
- [ ] Closures across boundaries → resource lifting (§6.4)
- [ ] REPL (§9), registry fetch `wavelet add`, exhaustiveness lint, hygiene (§10)

## Notes / decisions log
- 2026-06-12: Rust from the start (user choice). Reader has zero deps to keep
  first builds fast on this Raspberry Pi; wasm-tools deps deferred to Phase 5.
