# Dropping the interpreter: implications and feasibility

*Investigation note — 2026-06-15. Question posed: if/when component-based macros
land, is there anything left that genuinely needs the tree-walking interpreter?
Could we commit fully to the compiler, compile the compiler itself to a wasm
component, and switch the docs Playground and the REPL from interpret to
compile-and-run?*

The short answer: **almost everything the interpreter does can in principle be
taken over by the compiler plus an embedded wasm runtime — but the interpreter
is not just a runner, it is the project's *semantics oracle and test
substrate*, and the compiler is not yet at parity with it.** Dropping it is less
"delete `interp.rs`" and more "replace one self-contained reference
implementation with a hard dependency on a component-model runtime everywhere
the interpreter is used today, then re-architect the test and docs
infrastructure around running compiled output." That is a worthwhile direction,
but it is a multi-stage program with real costs, not a cleanup.

## 1. What the interpreter is used for today

The interpreter is `interp.rs` (599 lines) plus its support, `value.rs` (the WIT
value space) and `builtins.rs` (424 lines of primitive ops). It is wired into
**five distinct roles**, and they are not equally easy to retire:

1. **Semantics oracle for the compiler.** This is the load-bearing one.
   `CLAUDE.md` states it outright: *"`interp.rs` … defines the language's
   reference semantics. The wasm backend (`emit.rs`) is validated against it —
   the two must agree on every program."* The test suite is built on this
   differential discipline: `src/lib.rs` has paired tests that run a program
   through the interpreter *and* the wasm backend and assert equality
   (`lib.rs:555`, `lib.rs:592`). This is two independent implementations
   checking each other — exactly the structure that catches backend bugs.

2. **Compile-time macro expansion.** `expand.rs` (`expand_file`) constructs an
   `Interp`, installs builtins, evaluates `DefMacro` bodies, and runs every
   macro call to a fixpoint *before* WIT synthesis and emission ever see the
   tree (`expand.rs:17–33`, `interp.rs:300` `expand_once`). Macros are, today,
   **interpreted Wavelet running inside the Rust compiler.**

3. **`wavelet run`.** `runner.rs` (235 lines) is a multi-file interpreter: it
   resolves `Import` by package id across the files on the command line, honours
   `Export`/`as:`/`open:`, and calls the entry component's exported `run`. Its
   own doc comment calls it *"an interpreter stand-in for `wavelet compose`"*
   (`runner.rs:28`).

4. **`wavelet repl`.** `repl.rs` (56 lines) is a straight read-eval-print over
   the interpreter, with `DefMacro` arities persisted across lines.

5. **The docs Playground + the example test suite.** `eval_snippet` (`lib.rs:94`)
   is the single shared evaluation path. Native, it backs `tests/examples.rs`;
   compiled to `wasm32`, it backs the browser `<Playground>` and the
   `gen-examples.mjs` generator. Crucially, **the wasm build today ships *only*
   the interpreter** — `lib.rs:10–32` `#[cfg]`-gates the entire backend
   (`emit`, `build`, `wit`, `witdep`, `scaffold`, `tools`, `runner`, `repl`) out
   of `wasm32`, with the comment: *"the wasm build … only needs the
   interpreter."* `src/wasm.rs` is explicit that the Playground runs *"the same
   tree-walking interpreter that backs `wavelet run`, nothing simulated."*

## 2. Does component-based macros remove the need for the interpreter?

This is the crux of the question, and the answer is **yes for the macro role
specifically — but it replaces the interpreter with an embedded wasm runtime,
not with nothing.**

Per `design.md` §6.3, a macro library is a component exporting
`wavelet:meta/macros` with `manifest() -> list<(name, arity)>` and
`expand(name, args: tree) -> result<tree, string>`. `Import {… macros: true}`
*"instantiates that component **at compile time**, registers its manifest with
the reader, and routes expansion through `expand`."* The headline benefit is
sandboxing: an untrusted macro runs in a capability-gated wasm sandbox.

The consequence people miss: **to instantiate and call a component at compile
time, `wavelet build` needs a wasm runtime linked into it.** Today that runtime
*is* the tree-walker (`expand.rs` uses `Interp`). Under component macros it
becomes a real Component-Model engine — wasmtime embedded in the compiler, or in
the browser a transpiled (`jco`) component. So:

- A file-local `DefMacro … {…}` must be compiled to a (core-wasm or full)
  component and instantiated mid-build to expand its call sites. The
  "expand → emit" pipeline becomes self-referential: the expander runs the
  emitter on macro definitions, then runs the resulting wasm.
- Macros written in Rust (the `Re`-compiles-to-a-DFA example) already have no
  interpreter involvement — they were always going to be wasm.

So component macros **retire role #2**, but the cost is that `wavelet build`
acquires a hard dependency on an embedded component runtime, and the
expand/emit stages become mutually recursive. That is a real architectural shift
(and a classic bootstrapping wrinkle), not a deletion.

## 3. After that, is *anything* still interpreter-only?

Walking the five roles with component macros assumed done:

- **Role #2 (macros)** → embedded wasm runtime. Gone.
- **Role #3 (`wavelet run`)** → `build` + `compose` + run on the embedded
  runtime (or shell to `wasmtime`). `design.md` §9 already frames the REPL this
  way; `run` is the same move. Gone.
- **Role #4 (REPL)** → `design.md` §9 literally specifies the compile-and-run
  design: *"The REPL is a scratch component that is rebuilt and re-composed per
  definition; since values print as WAVE and code is WAVE, the REPL's output is
  always valid input."* The interpreter REPL was always a placeholder. Gone.
- **Role #5 (docs/tests)** → compile-and-run in the browser, contingent on §4
  below.
- **Role #1 (the oracle)** → **this is the one that does not have a
  drop-in replacement.** If the compiler *is* the only implementation, there is
  no second implementation to differentially test it against. You do not lose
  *correctness*, but you lose the *mechanism that currently establishes*
  correctness. The replacement is golden-output testing: compile each example,
  run it on a real runtime, assert the recorded value/output. That is heavier
  (every test needs componentize + a runtime) and weaker (a golden file only
  catches regressions against itself, not "these two independent
  implementations disagree"). The interpreter has been the cheap, fast,
  total reference; giving it up means the compiler must first *earn* that trust.

**Net:** nothing in the language *semantics* fundamentally requires a
tree-walker once macros are components and run/repl/docs compile-and-run. What
the interpreter uniquely provides is (a) a second, independent, total
implementation used as a test oracle, and (b) a fast, dependency-light way to
*execute* Wavelet without a wasm runtime in the loop. Both are methodological
conveniences, not semantic necessities — but they are valuable ones, and the
first is woven through the entire test architecture.

## 4. Can the compiler itself compile to a wasm component?

This is what the "compile in the browser, run the compiled code" plan needs, and
it is the hardest part. Three sub-questions:

**(a) Can the front+middle end compile to `wasm32`?** Plausibly yes. read →
expand → WIT-synthesis → core-wasm emit lean on `wasm-encoder`, `wit-parser`,
and `wit-component` — pure-Rust crates from the wasm-tools ecosystem that are
themselves designed to run inside wasm tooling. The arena/form representation and
`emit.rs` are ordinary Rust. The current `#[cfg(not(wasm32))]` gate on these
modules (`lib.rs:13–28`) is a *build-time convenience* (the wasm playground
didn't need them), not evidence they *can't* compile to wasm. This part is mostly
mechanical.

**(b) The external-CLI dependency is a real blocker.** `tools.rs` shells out to
two BytecodeAlliance CLIs via `std::process::Command` — `wkg` (WIT package
fetch/build/lock) and `wac` (composition). `Command` does not exist in a browser
wasm sandbox. Note the *current trajectory is moving toward* these subprocesses
(recent commits added the `wkg`/`wac` wrapper; `dropped http`), which makes the
browser story harder, not easier. To compile in the browser you would need:
  - **Composition** via the `wac-graph` *library* (already a dependency,
    `Cargo.toml`) instead of the `wac` CLI — feasible, the auto-plug path already
    uses the library.
  - **WIT fetching (`wkg`)** has no in-browser equivalent (it needs network +
    filesystem + a registry). For the **docs Playground specifically this is
    avoidable**: snippets are self-contained single components using only the
    std builtins, with no external WIT deps to fetch. For general
    project builds in the browser it is not avoidable without vendoring WIT.

**(c) Running the *output* in the browser is a second, bigger lift.** The
compiler emits a **Component-Model** component (often WASI-targeted for CLI
programs). Browsers run *core* wasm, not components. To "run the compiled code"
in the page you need either `jco transpile` (Component → JS + core-wasm shims) in
the browser, or a component runtime — neither is free, and WASI imports
(`wasi:cli/stdout`, etc.) need shimming to the existing output-capture sink
(`lib.rs` `OUTPUT_SINK`). Today the Playground sidesteps all of this by
interpreting; compile-and-run trades a single ~MB interpreter wasm blob for a
compiler-to-wasm *plus* a component-execution toolchain shipped to the browser.

So: compiling the compiler to wasm is *achievable* for the self-contained
Playground case (no `wkg`, library-based compose, GC backend + `jco` to run
output), and *not* achievable as a general "everything the native CLI does"
without solving registry fetch and subprocess use. The honest framing is that the
Playground could move to compile-and-run as a contained project; the full
`wavelet` CLI compiling to a browser component is a much larger claim.

## 5. The compiler is not yet at parity — gaps that block "fully commit" today

Dropping the interpreter now would be a capability *regression*, because the
backend is explicitly v0 with known holes (`todo.md` Phase 5, "v0 backend gaps
still open"):

- **No garbage collection — "leaks by design"** (linear-memory backend). The
  interpreter relies on Rust's `Rc`. A *compile-and-run REPL* — long-lived,
  redefining per entry — would leak on every definition. The GC story (or the
  wasm-GC backend) must land before compile-and-run is viable for anything
  long-running.
- option/result **params** with mismatched-arm flat shapes (needs the
  numeric-widening variant join); `>16`-flat param spill-to-memory; named
  3+-case `variant`/`enum` DefTypes across boundaries (the dynamic core has no
  constructor for user variant cases beyond `ok`/`some`/`err`/`none`).
- `--fuse`, `compose.wave` manifest, richer boundary coercions / `safely` are
  still open.

The interpreter handles all of the dynamic semantics fully today. Committing to
the compiler means first closing these so the compiler reaches interpreter
parity — otherwise you lose working behaviour.

Also note the **self-hosting plan** (`design.md` §9): *"reader and expander
rewritten in Wavelet early … backend last."* Until that backend is solid and
self-hosting, the Rust interpreter is the fast path that actually *runs* Wavelet
during bootstrap. Dropping it does not conflict with self-hosting, but it removes
the scaffolding you climb to get there.

## 6. What you gain vs. what you lose

**Gains**

- **One semantics, not two.** No more keeping `interp.rs` and `emit.rs` in lock-
  step — `CLAUDE.md` flags that sync burden as a standing rule ("update the
  interpreter first"). Whole classes of divergence bugs disappear because there
  is nothing to diverge.
- **Less code.** `interp.rs` (~600) + most of `builtins.rs` (~424) + `runner.rs`
  (~235) + `repl.rs` (~56) ≈ 1.3k LOC retireable (value.rs partly shared).
- **"The compiler is the language."** Simpler conceptual model; the Playground
  runs *exactly* what ships, not a parallel evaluator.
- **Macro sandboxing for free**, as a direct consequence of component macros.

**Losses / costs**

- **Loss of the differential-testing oracle** — the single biggest cost. The
  test suite must be re-architected onto golden outputs run on a real runtime
  (componentize + execute per example), which is slower and a weaker check.
- **Hard dependency on an embedded component runtime** at compile time (macros)
  and at run/repl/docs (execution). The browser variant additionally needs
  `jco`-style transpilation and WASI shimming.
- **Latency.** Interpreting a REPL line or a doc snippet is instant; compile →
  componentize → instantiate → run per keystroke-batch is not. Caching helps but
  the floor is higher.
- **Must close the backend gaps first** (esp. GC), or accept a regression.

## 7. Suggested sequencing (if pursued)

1. **Close backend parity gaps**, GC first — without it, compile-and-run leaks.
2. **Land component macros** with an embedded runtime in `wavelet build`; keep
   the interpreter as the expander until that path is trusted, then switch
   `expand.rs` over and delete the interpreter's macro role.
3. **Re-architect the test oracle**: move `tests/examples.rs` from "interp vs.
   emit" to "compiled output on a runtime vs. golden", proving the compiler can
   stand alone *before* the interpreter is removed.
4. **Switch `wavelet run`/`repl`** to build+compose+run (design §9 already
   specifies the REPL form).
5. **Playground last**, as a self-contained "compiler-to-wasm + library compose
   (no `wkg`) + `jco`-run" project — the one place all the browser blockers
   converge.
6. **Then** delete `interp.rs`/`runner.rs`/`repl.rs` and the interpreter-only
   builtins.

## 8. Bottom line

- **Will anything still *need* the interpreter once macros are components?** No,
  not for language *semantics* — macros move to an embedded wasm runtime, and
  run/repl/docs can all compile-and-run. The two things the interpreter uniquely
  provides are a *second, independent implementation used as a test oracle* and a
  *fast, runtime-free way to execute Wavelet*; both are methodological, both are
  replaceable, but the oracle is woven through the whole test/docs architecture
  and replacing it is the real work.
- **Can the compiler compile to a wasm component?** The front+middle end and
  in-process emit/compose: plausibly yes. The external `wkg`/`wac` CLIs and
  running a *component* in the browser (`jco` + WASI shims) are the blockers —
  solvable for the self-contained Playground, hard for the general CLI.
- **Should we do it now?** Not yet. The backend is explicitly v0 (no GC, several
  boundary gaps); dropping the interpreter today is a regression. It is a sound
  *destination* reachable by the sequence above, with "re-architect the test
  oracle" and "GC" as the gating prerequisites, not "delete the interpreter" as
  the first step.
