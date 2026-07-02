# Dropping the interpreter: implications and feasibility

## Suggested sequencing

1.  **Close backend parity gaps**, GC first — without it, compile-and-run leaks.
2.  **Land component macros** with an embedded runtime in `wavelet build`; keep
    the interpreter as the expander until that path is trusted, then switch
    `expand.rs` over and delete the interpreter's macro role.
3.  **Re-architect the test oracle**: move `tests/examples.rs` from "interp vs.
    emit" to "compiled output on a runtime vs. golden", proving the compiler can
    stand alone *before* the interpreter is removed.
4.  **Switch `wavelet run`/`repl`** to build+compose+run (design §9 already
    specifies the REPL form).
5.  **Playground last**, as a self-contained "compiler-to-wasm + library compose
    (no `wkg`) + `jco`-run" project — the one place all the browser blockers
    converge.
6.  **Then** delete `interp.rs`/`runner.rs`/`repl.rs` and the interpreter-only
    builtins.
