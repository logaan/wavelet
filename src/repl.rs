//! `wavelet repl` (§9): read a form, evaluate it, print the value.
//! Multi-line input is supported by continuing while the reader reports an
//! unexpected end of input.
//!
//! ## Compiled-first evaluation (goal 6, Step 2)
//!
//! Each expression entry is evaluated the *accumulate-and-recompile* way: the
//! session keeps the accepted **definitions** (`Def`/`DefMacro`/`DefType`/…) as
//! canonical source, and a bare expression is compiled as a synthetic
//! single-file program — the accumulated definitions plus a synthetic
//! `Def repl-eval Fn {} to-string(<expr>)` exported as a `string` — built
//! through the real emitter (`build::build_files`), instantiated in the
//! capability-free `wasmtime` host (`host::HostComponent`), and called. The
//! guest renders its own result with the backend's `to-string` (the port of the
//! interpreter's `print_value`) and returns it as a WIT `string`, so no
//! linear-memory value reader is needed — this is the path the differential
//! harness (`tests/differential.rs`) proves agrees with the interpreter.
//!
//! The wasm backend is still a strict subset of the interpreter on a few
//! decision-gated behaviours (float/char `to-string`, runtime flag literals,
//! record-payload `apply`, `read`). When the compiled path cannot produce a
//! result for an expression — a build error, or a runtime trap — the REPL falls
//! back to the interpreter for **that entry** and notes it on stderr, so a line
//! that hits one of those holes still evaluates (as it did before) rather than
//! failing with a mystery trap. The interpreter therefore remains wired into the
//! REPL as the fallback until those decisions land; it is not yet fully retired
//! from this surface (goal 6.6).

use std::io::{BufRead, Write};
use std::rc::Rc;

use crate::form::{Arena, Node, NodeId};
use crate::interp::Interp;
use crate::printer::print;
use crate::reader::MacroTable;
use crate::value::{Env, print_value};

/// The synthetic package/interface/entry the accumulate-and-recompile REPL
/// wraps each expression into. Names mirror the differential harness's
/// self-contained component (`docs:snippet` / `differential-main`); WIT
/// identifiers are kebab-case, so the entry is `repl-eval`, not `__eval`.
const PACKAGE: &str = "repl:session@0.0.0";
const IFACE: &str = "repl:session/api@0.0.0";
const MAIN: &str = "repl-eval";

/// Top-level declaration heads (post-reader `-MACRO` spellings): forms that
/// declare rather than evaluate. A declaration accumulates into the session; any
/// other form is an expression and is the evaluation target. Mirrors
/// `tests/differential.rs`'s list.
const DECL_HEADS: &[&str] = &[
    "package-MACRO",
    "target-MACRO",
    "import-MACRO",
    "export-MACRO",
    "def-MACRO",
    "defmacro-MACRO",
    "deftype-MACRO",
    "derive-MACRO",
];

fn is_decl(arena: &Arena, root: NodeId) -> bool {
    let Node::Tup(items) = arena.node(root) else {
        return false;
    };
    let Some(&head) = items.first() else {
        return false;
    };
    matches!(arena.node(head), Node::Sym(s) if DECL_HEADS.contains(&s.as_str()))
}

/// Assemble the synthetic program compiled to evaluate one expression:
/// `Package` + the accumulated definitions + a `repl-eval` entry whose body is
/// `to-string(<expr>)`, exported as a `string`.
fn synth_program(defs: &[String], expr_src: &str) -> String {
    let mut out = format!("Package \"{PACKAGE}\"\n\n");
    for d in defs {
        out.push_str(d);
        out.push('\n');
    }
    out.push_str(&format!("\nExport {{name: {MAIN} result: string}}\n"));
    out.push_str(&format!("Def {MAIN} Fn {{}}\n  to-string({expr_src})\n"));
    out
}

/// Build the synthetic program through the real emitter and call its entry,
/// returning the printed value the guest's `to-string` produced. `Err` carries
/// the failing stage (build / instantiate / call / trap).
fn compiled_eval(program: &str) -> Result<String, String> {
    use crate::host::{HostComponent, Val};
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-repl-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).map_err(|e| format!("setup: {e}"))?;
    let path = src.join("entry.wlt");
    std::fs::write(&path, program).map_err(|e| format!("setup: {e}"))?;
    let out_dir = dir.join("out");

    let result = (|| {
        let outputs = crate::build::build_files(
            &[path.to_str().unwrap().to_string()],
            out_dir.to_str().unwrap(),
        )
        .map_err(|e| format!("build: {e}"))?;
        let bytes = std::fs::read(&outputs[0]).map_err(|e| format!("read artifact: {e}"))?;
        let mut component =
            HostComponent::from_bytes(&bytes).map_err(|e| format!("instantiate: {e}"))?;
        let vals = component
            .call_instance(IFACE, MAIN, &[])
            .map_err(|e| format!("call: {e}"))?;
        match vals.as_slice() {
            [Val::String(s)] => Ok(s.to_string()),
            other => Err(format!("call: unexpected result shape {other:?}")),
        }
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

pub fn repl() -> Result<(), String> {
    let interp = Interp::new();
    let env = Env::root();
    crate::builtins::install(&env);
    let mut macros = MacroTable::core();
    // Canonical source of every accepted definition, in entry order; the
    // recompile input for each expression line.
    let mut defs: Vec<String> = Vec::new();

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut buf = String::new();
    eprintln!("wavelet repl — enter forms, Ctrl-D to exit");
    loop {
        let prompt = if buf.is_empty() { "> " } else { ". " };
        eprint!("{prompt}");
        std::io::stderr().flush().ok();
        let Some(line) = lines.next() else { break };
        let line = line.map_err(|e| e.to_string())?;
        buf.push_str(&line);
        buf.push('\n');
        if buf.trim().is_empty() {
            buf.clear();
            continue;
        }
        match crate::reader::read_with(&buf, &mut macros) {
            Err(e) if e.msg == "unexpected end of input" => continue, // more lines
            Err(e) => {
                eprintln!("read error: {e}");
                buf.clear();
            }
            Ok((arena, roots)) => {
                buf.clear();
                let arena = Rc::new(arena);
                for root in roots {
                    if is_decl(&arena, root) {
                        // A declaration accumulates into the session. Evaluate it
                        // in the interpreter too, which keeps the fallback env
                        // consistent, validates the form, and yields the unit
                        // echo the REPL has always printed for a definition.
                        match interp.eval(&arena, root, &env) {
                            Ok(v) => {
                                defs.push(print(&arena, root));
                                println!("{}", print_value(&v));
                            }
                            Err(e) => {
                                eprintln!("error: {e}");
                                break;
                            }
                        }
                    } else {
                        // An expression is the evaluation target: compile the
                        // accumulated session with it as the `repl-eval` entry
                        // and print the guest's `to-string` result. On any
                        // compiled-path failure fall back to the interpreter for
                        // this entry (the decision-gated holes: float/char
                        // to-string, runtime flags, record apply, read).
                        let expr_src = print(&arena, root);
                        let program = synth_program(&defs, &expr_src);
                        match compiled_eval(&program) {
                            Ok(s) => println!("{s}"),
                            Err(compiled_err) => match interp.eval(&arena, root, &env) {
                                Ok(v) => {
                                    eprintln!(
                                        "(interpreter fallback: compiled path unavailable: \
                                         {compiled_err})"
                                    );
                                    println!("{}", print_value(&v));
                                }
                                Err(e) => {
                                    eprintln!("error: {e}");
                                    break;
                                }
                            },
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
