//! `wavelet repl` (§9): read a form, evaluate it, print the value.
//! Multi-line input is supported by continuing while the reader reports an
//! unexpected end of input.

use std::io::{BufRead, Write};
use std::rc::Rc;

use crate::interp::Interp;
use crate::reader::MacroTable;
use crate::value::{print_value, Env};

pub fn repl() -> Result<(), String> {
    let interp = Interp::new();
    let env = Env::root();
    crate::builtins::install(&env);
    let mut macros = MacroTable::core();

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
                    match interp.eval(&arena, root, &env) {
                        Ok(v) => println!("{}", print_value(&v)),
                        Err(e) => {
                            eprintln!("error: {e}");
                            break;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
