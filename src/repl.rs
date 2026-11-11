use std::rc::Rc;

use crate::form::{Arena, Node, NodeId};
use crate::interp::Interp;
use crate::printer::print;
use crate::reader::MacroTable;
use crate::value::{Env, print_value};

const PACKAGE: &str = "repl:session@0.0.0";
const IFACE: &str = "repl:session/api@0.0.0";
const MAIN: &str = "repl-eval";

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

fn history_path() -> Option<std::path::PathBuf> {
    if let Some(x) = std::env::var_os("XDG_STATE_HOME").filter(|s| !s.is_empty()) {
        return Some(std::path::PathBuf::from(x).join("wavelet").join("history"));
    }
    let home = std::env::var_os("HOME").filter(|s| !s.is_empty())?;
    Some(std::path::PathBuf::from(home).join(".local/state/wavelet/history"))
}

pub fn repl() -> Result<(), String> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        repl_interactive()
    } else {
        repl_piped()
    }
}

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
                    Err(e) if e.msg == "unexpected end of input" => continue,
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
            Err(e) if e.msg == "unexpected end of input" => continue,
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
