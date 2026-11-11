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
#[derive(Default)]
struct CompiledSession {
    entries: std::collections::HashMap<String, crate::host::HostComponent>,
}

impl CompiledSession {
    fn evaluate(&mut self, program: &str) -> Result<String, String> {
        use crate::host::{HostComponent, Val};
        if !self.entries.contains_key(program) {
            let bytes = crate::staging::Project::compile(program).map_err(|e| e.to_string())?;
            let component =
                HostComponent::from_bytes(&bytes).map_err(|e| format!("instantiate: {e}"))?;
            if self.entries.len() >= 32 {
                self.entries.clear();
            }
            self.entries.insert(program.to_owned(), component);
        }
        let component = self.entries.get_mut(program).unwrap();
        let result = component.call_instance(IFACE, MAIN, &[]);
        match result {
            Ok(vals) => match vals.as_slice() {
                [Val::String(s)] => Ok(s.to_string()),
                other => Err(format!("call: unexpected result shape {other:?}")),
            },
            Err(e) => {
                self.entries.remove(program);
                Err(format!("call: {e}"))
            }
        }
    }
}

/// Evaluate every form read from one accepted input buffer. A declaration
/// accumulates into `defs` (and is evaluated in the interpreter to keep the
/// fallback env consistent, validate the form, and yield the unit echo); an
/// expression is compiled — the accumulated session plus a synthetic
/// `repl-eval` entry — and its `to-string` value printed, falling back to the
/// interpreter for that entry on any compiled-path failure. Shared by the
/// interactive and piped front-ends so both behave identically.
fn value_definition_name(arena: &Arena, root: NodeId) -> Option<&str> {
    let Node::Tup(items) = arena.node(root) else {
        return None;
    };
    if items.len() != 3 || !matches!(arena.node(items[0]), Node::Sym(s) if s == "def-MACRO") {
        return None;
    }
    if let Node::Tup(body) = arena.node(items[2])
        && body
            .first()
            .is_some_and(|head| matches!(arena.node(*head), Node::Sym(s) if s == "fn-MACRO"))
    {
        return None;
    }
    match arena.node(items[1]) {
        Node::Sym(name) => Some(name),
        _ => None,
    }
}

fn eval_forms(
    arena: &Rc<Arena>,
    roots: &[NodeId],
    defs: &mut Vec<String>,
    interp: &Interp,
    env: &Env,
    compiled: &mut CompiledSession,
) {
    for &root in roots {
        if is_decl(arena, root) {
            match interp.eval(arena, root, env) {
                Ok(v) => {
                    if let Some(name) = value_definition_name(arena, root) {
                        defs.retain(|source| {
                            crate::reader::read_file(source).ok().is_none_or(
                                |(old_arena, old_roots)| {
                                    old_roots
                                        .first()
                                        .and_then(|root| value_definition_name(&old_arena, *root))
                                        != Some(name)
                                },
                            )
                        });
                    }
                    defs.push(print(arena, root));
                    compiled.entries.clear();
                    println!("{}", print_value(&v));
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    break;
                }
            }
        } else {
            let expr_src = print(arena, root);
            let program = synth_program(defs, &expr_src);
            match compiled.evaluate(&program) {
                Ok(s) => println!("{s}"),
                Err(compiled_err) => {
                    eprintln!("error: {compiled_err}");
                    break;
                }
            }
        }
    }
}

/// Where the interactive REPL persists its input history. Follows the XDG state
/// convention (`$XDG_STATE_HOME/wavelet/history`, else `~/.local/state/wavelet/
/// history`); `None` when neither `XDG_STATE_HOME` nor `HOME` is set, in which
/// case history is kept in memory for the session only.
fn history_path() -> Option<std::path::PathBuf> {
    if let Some(x) = std::env::var_os("XDG_STATE_HOME").filter(|s| !s.is_empty()) {
        return Some(std::path::PathBuf::from(x).join("wavelet").join("history"));
    }
    let home = std::env::var_os("HOME").filter(|s| !s.is_empty())?;
    Some(std::path::PathBuf::from(home).join(".local/state/wavelet/history"))
}

/// `wavelet repl`: interactive line-editing + history when stdin is a terminal
/// (7.1), and a plain line-reader when it is not (a pipe or here-doc), so
/// scripted/piped sessions stay byte-for-byte what they were before readline was
/// added. Both front-ends share [`eval_forms`].
pub fn repl() -> Result<(), String> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        repl_interactive()
    } else {
        repl_piped()
    }
}

/// Interactive front-end: `rustyline` provides arrow-key line editing, a
/// persistent history file, Ctrl-C to abandon the current (possibly multi-line)
/// entry, and Ctrl-D to exit.
fn repl_interactive() -> Result<(), String> {
    use rustyline::error::ReadlineError;

    let interp = Interp::new();
    let env = Env::root();
    crate::builtins::install(&env);
    let mut macros = MacroTable::core();
    let mut defs: Vec<String> = Vec::new();
    let mut compiled = CompiledSession::default();

    let mut rl = rustyline::DefaultEditor::new().map_err(|e| e.to_string())?;
    let hist = history_path();
    if let Some(p) = &hist {
        let _ = rl.load_history(p);
    }

    eprintln!("wavelet repl — enter forms, Ctrl-D to exit");
    let mut buf = String::new();
    loop {
        let prompt = if buf.is_empty() { "> " } else { ". " };
        match rl.readline(prompt) {
            Ok(line) => {
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
                        let _ = rl.add_history_entry(buf.trim_end());
                        buf.clear();
                        let arena = Rc::new(arena);
                        eval_forms(&arena, &roots, &mut defs, &interp, &env, &mut compiled);
                    }
                }
            }
            // Ctrl-C abandons the current entry but keeps the session going;
            // Ctrl-D (EOF) exits.
            Err(ReadlineError::Interrupted) => {
                buf.clear();
            }
            Err(ReadlineError::Eof) => break,
            Err(e) => return Err(e.to_string()),
        }
    }

    if let Some(p) = &hist {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = rl.save_history(p);
    }
    Ok(())
}

/// Piped/non-terminal front-end: read stdin line by line with no editing (the
/// original REPL loop). Prompts go to stderr; results to stdout. Used for
/// here-docs, pipelines, and the behaviour tests.
fn repl_piped() -> Result<(), String> {
    use std::io::{BufRead, Write};

    let interp = Interp::new();
    let env = Env::root();
    crate::builtins::install(&env);
    let mut macros = MacroTable::core();
    let mut defs: Vec<String> = Vec::new();
    let mut compiled = CompiledSession::default();

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
                eval_forms(&arena, &roots, &mut defs, &interp, &env, &mut compiled);
            }
        }
    }
    Ok(())
}
