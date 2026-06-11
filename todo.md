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
- [ ] Core special-form arity table (the 17 forms, §4.2) seeded into reader
- [ ] `DefMacro` registration while reading top-to-bottom (define-before-use)
- [ ] `Quote` / `Quasi` / `Unquote` / `Splice` semantics over form trees
- [ ] `gensym`
- [ ] Expansion to fixpoint
- [ ] (later) macro components: instantiate wasm at compile time, `manifest`/
      `expand` interface, `Import {… macros: true}`

## Phase 3 — interpreter (validate semantics before emitting wasm)
- [ ] Value repr = WIT value space only (§3); structural `eq`
- [ ] Eval rules 1–4 (§4.1); Lisp-1 lexical env
- [ ] Special forms: `Def`, `Fn` (record/list/tuple/scalar payload binding §4.2),
      `If`, `Let` (sequential record bindings), `Do`, `Match` (patterns §4.2),
      `The`
- [ ] Tail-call elimination in the interpreter (trampoline) — §5 positions
- [ ] Builtin `wavelet:std/core` subset: eq/lt/…, add/sub/…, list ops, str ops,
      to-string/read, print/println, apply, form accessors
- [ ] Run the §1 and §8 examples end to end (single-component, no composition)

## Phase 4 — module/component model surface (§6.1)
- [ ] `Package`, `Target`, `Import`, `Export`, `DefType` forms parsed + checked
- [ ] WIT world synthesis from a file (typed `Fn` params, `Export` record form)
- [ ] Type ascription `The`, boundary type checks + coercions (§3)

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
