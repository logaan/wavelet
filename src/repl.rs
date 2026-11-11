use crate::compiler::snippet::value_definition_name;
use crate::form::{Arena, NodeId};
use crate::printer::print;
use crate::reader::MacroTable;

#[derive(Default)]
struct CompiledSession {
    entries: std::collections::HashMap<String, CompiledEntry>,
}

struct CompiledEntry {
    component: crate::host::HostComponent,
    invocation: crate::compiler::Invocation,
}

impl CompiledSession {
    fn prepare(&mut self, source: &str) -> Result<(), String> {
        if self.entries.contains_key(source) {
            return Ok(());
        }
        let request = crate::compiler::Request::snippet(source);
        let compilation = crate::compiler::native::compile(&request, std::path::Path::new("."))
            .map_err(crate::compiler::native::display_diagnostics)?;
        let artifact = compilation
            .entry_artifact()
            .ok_or("compiler produced no entry artifact")?;
        let component = crate::host::HostComponent::from_bytes(&artifact.bytes)
            .map_err(|e| format!("instantiate: {e}"))?;
        let invocation = compilation
            .invocation
            .ok_or("compiler produced no snippet invocation")?;
        if self.entries.len() >= 32 {
            self.entries.clear();
        }
        self.entries.insert(
            source.to_owned(),
            CompiledEntry {
                component,
                invocation,
            },
        );
        Ok(())
    }

    fn evaluate(&mut self, source: &str) -> Result<String, String> {
        self.prepare(source)?;
        let entry = self.entries.get_mut(source).unwrap();
        let result = entry.component.call_instance(
            &entry.invocation.interface_name,
            &entry.invocation.function_name,
            &[],
        );
        match result {
            Ok(vals) => match vals.as_slice() {
                [crate::host::Val::String(value)] => Ok(value.to_string()),
                other => Err(format!("call: unexpected result shape {other:?}")),
            },
            Err(error) => {
                self.entries.remove(source);
                Err(format!("call: {error}"))
            }
        }
    }
}

fn eval_forms(
    arena: &Arena,
    roots: &[NodeId],
    defs: &mut Vec<String>,
    compiled: &mut CompiledSession,
) {
    for &root in roots {
        let result = if crate::compiler::snippet::is_declaration(arena, root) {
            add_definition(arena, root, defs, compiled).map(|()| "{}".to_owned())
        } else {
            let source = format!("{}\n{}", defs.join("\n"), print(arena, root));
            compiled.evaluate(&source)
        };
        match result {
            Ok(value) => println!("{value}"),
            Err(error) => {
                eprintln!("error: {error}");
                break;
            }
        }
    }
}

fn add_definition(
    arena: &Arena,
    root: NodeId,
    defs: &mut Vec<String>,
    compiled: &mut CompiledSession,
) -> Result<(), String> {
    let mut candidate = defs.clone();
    if let Some(name) = value_definition_name(arena, root) {
        candidate.retain(|source| {
            crate::reader::read_file(source)
                .ok()
                .is_none_or(|(old_arena, old_roots)| {
                    old_roots
                        .first()
                        .and_then(|root| value_definition_name(&old_arena, *root))
                        != Some(name)
                })
        });
    }
    candidate.push(print(arena, root));
    compiled.evaluate(&candidate.join("\n"))?;
    *defs = candidate;
    compiled.entries.clear();
    Ok(())
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
                        eval_forms(&arena, &roots, &mut defs, &mut compiled);
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
                eval_forms(&arena, &roots, &mut defs, &mut compiled);
            }
        }
    }
    Ok(())
}
