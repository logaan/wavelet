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
- [x] `///` doc comments: lexed as tokens, attached to the following form
      (arena `docs` map), surfaced as `///` lines in `wavelet wit` output
      (docs on Export/Def/DefType key by defined name)
- [ ] Qualified TitleCase macros `Dsl/Element` arity reading (parses, but arity
      lookup ignores the alias; revisit with macro imports in Phase 2)

## Phase 2 — expander (§2.4, §6.3)
- [x] Core special-form arity table (the 17 forms, §4.2) seeded into reader
- [x] `DefMacro` registration while reading top-to-bottom (define-before-use)
- [x] `Quote` / `Quasi` / `Unquote` / `Splice` semantics over form trees
- [x] `gensym`
- [x] Expansion (lazy, at eval time: a call whose head is bound to a Macro
      value expands and jumps into the result — fixpoint by re-evaluation)
- [x] Nested `Quasi` depth handling (Scheme-style: Unquote/Splice fire at
      depth 1, rebuilt one level shallower otherwise)
- [x] Ahead-of-time expand pass (`src/expand.rs`): DefMacro forms evaluated
      and dropped, call sites rewritten to fixpoint; wired into `wavelet
      build`; `wavelet expand <file>` prints the expanded tree. Macro bodies
      see builtins + earlier macros only (not file-local fns yet)
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
- [x] `expand` builtin: one expansion step on a macro-call form value
      (macros looked up in the caller's env); pass-through otherwise
- [ ] Resource handles beyond `cell`; owned-handle drop semantics (§6.1)

## Phase 4 — module/component model surface (§6.1)
- [x] `Package`, `Target`, `Import`, `Export`, `DefType` forms parsed; handled
      by the runner (interp) and `src/wit.rs` (synthesis)
- [x] WIT world synthesis (`wavelet wit <file>`): typed `Fn` params, explicit
      `Export` record form, `DefType` records/variants/flags/aliases,
      best-effort result-type inference; shout.wvl reproduces §6.1 exactly
- [x] Type ascription `The` (primitive checks at eval time)
- [x] Cross-def inference: an export whose body calls another module-level
      `Def` follows the call (recursion-guarded; recursive fns still need the
      Export record form)
- [ ] Richer inference for lists/options/results — currently errors and asks
      for annotations when it cannot infer
- [x] Result inference through `Match` (unify all clause results; pattern-bound
      names left untyped) — unblocks exports whose body ends in a Match
- [ ] Boundary coercions + `safely` wrapper semantics (§3)
- [x] Grouped exports `Export {iface: "render" ...}`: wit synthesis, runner
      import filtering, and the wasm backend (per-iface export names + dep
      lookup); name-only record forms still get inferred signatures

## Phase 5 — emit + componentize (§9)
- [x] Crates: wasm-encoder 0.251, wit-parser 0.251, wit-component 0.251,
      wac-graph 0.10
- [x] `src/wit.rs` refactor: structured `FileInfo`/`FuncSig`/`ImportInfo`
      via `collect()` (synthesize() output unchanged)
- [x] Emit core wasm (`src/emit.rs`): boxed values in linear memory (tag
      i32: bool/int/str/list/dec), bump allocator + `cabi_realloc`, static
      string boxes in data section, `return_call` for tail positions
- [x] Canonical-ABI lift/lower wrappers (string/s64/bool/f64 sigs; string
      results via callee retptr area); vendored trimmed WASI WIT @0.2.0
      (io/streams, cli/stdout+environment+run); `Target "wasi:cli/command"`
      maps exported `run` to `wasi:cli/run@0.2.0#run`
- [x] Componentize: synthesized nested-package WIT → embed_component_metadata
      → ComponentEncoder (validated)
- [x] `wavelet build <files...> [-o dir]` — one component per file
- [x] `wavelet compose <entry> <plugs...> [-o app.wasm]` via wac-graph
      auto-plug (§6.5)
- [x] End-to-end §1 demo on wasmtime v45: `wavelet build && wavelet compose`,
      `wasmtime run out/app.wasm wasm` → `WASM!`; no args → usage line
- [x] Match in the wasm backend: literal/name/list patterns compiled to
      block-per-clause tests; no clause → trap (verified on wasmtime)
- [x] List literals (heap list boxes) and module-level value defs (lazily
      initialized globals, cycle-guarded) in the wasm backend
- [x] First-class closures in the wasm backend: `TAG_FN` boxes (funcref
      table slot + captures), uniform `(env payload) -> box` convention,
      `call_indirect`/`return_call_indirect`, capture of all visible
      locals, named defs as values via cached wrappers + static boxes;
      `to-string` helper (int/bool/str); verified on wasmtime against
      interpreter output (make-adder / twice / value-def closures)
- [x] Lists across boundaries: `list<T>` params/results lowered/lifted per
      the canonical ABI (strings, ints, bools, f64, nested lists); generic
      retptr path; verified composed on wasmtime (list<string>, list<s64>)
- [x] Records in the wasm backend: `TAG_REC` boxes `[tag, n, (key str box,
      value box)…]`, record literal construction, `rec_get` helper (field
      lookup by interned key, 0 when absent), and record patterns in Match
      (tag check + subset-of-fields, mirrors the interpreter); verified on
      wasmtime against interpreter output (`{x: 3 y: 7 label: "pt"}`)
- [x] Variants in the wasm backend: `TAG_VAR` boxes `[tag, case str box,
      payload box]`; `some`/`ok`/`err` constructors, static `none`; variant
      patterns in Match (`ok(x)`/`err(e)`/`some(x)`/bare `none`)
- [x] Tuples in the wasm backend: `TAG_TUP` (list layout, distinct identity);
      literal construction + tuple patterns in Match (shared with lists)
- [x] Records across component boundaries: `WitTy::Record` (named DefType
      resolution via a TypeEnv merging local + dep types), canonical field
      layout (`align_of`/`size_of`/offsets), record params lowered flattened
      and lifted via `lift_flat`, record results returned by retptr with
      `store_to_mem`/`load_from_mem` over the field layout. Fixed a latent WIT
      synthesis bug (record/variant/flags decls had an invalid trailing `;`).
      Verified composed on wasmtime: `make-point`/`sum-coords` and a mixed
      s32/bool/s64/f64 record both round-trip and match the interpreter.
- [x] option/result across component boundaries: `WitTy::Option`/`Result`
      with canonical 2-case variant layout (1-byte discriminant + aligned
      payload union). Returns go through the in-memory path (retptr +
      `store_to_mem`/`load_from_mem`), so mismatched arm shapes like
      `result<s64, string>` work; params are lowered flattened (disc + joined
      payload, `lower_variant_case`) / lifted (`lift_variant_case`) when the
      arms are flat-compatible. `flat_len` decides direct-vs-retptr without
      needing the variant-join. Verified composed on wasmtime:
      `option<s64>` (some/none) and `result<s64,string>` (ok/err) match the
      interpreter.
- [x] string fields inside boundary aggregates: a string in canonical memory is
      just `(ptr, len)`, so record/option/result payloads of type `string` now
      marshal (records with string fields verified composed on wasmtime).
- [ ] v0 backend gaps still open: `list` fields inside a boundary aggregate and
      `list<record/option/result>`; option/result *params* with mismatched arm
      flat shapes; >16-flat param spill-to-memory; general (3+ case, named)
      variant types across boundaries; GC (leaks by design), `compose.wave`
      manifest, `--fuse`

## Phase 6 — beyond
- [ ] Closures across boundaries → resource lifting (§6.4)
- [x] REPL (§9): `wavelet repl` — interpreter-backed, multi-line input,
      DefMacro arities persist across lines (`reader::read_with`)
- [ ] Registry fetch `wavelet add`, exhaustiveness lint, hygiene (§10)

## Notes / decisions log
- 2026-06-12: Rust from the start (user choice). Reader has zero deps to keep
  first builds fast on this Raspberry Pi; wasm-tools deps deferred to Phase 5.
