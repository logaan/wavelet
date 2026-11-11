pub mod builtins;
pub mod check;
pub mod compiler;
pub mod expand;
pub mod form;
pub mod interp;
pub mod lexer;

pub mod macrolib;
pub mod printer;
pub mod reader;
mod templates;
pub mod value;

#[cfg(not(target_arch = "wasm32"))]
pub mod build;
pub mod emit;
#[cfg(not(target_arch = "wasm32"))]
pub mod host;
pub mod macrobuild;
#[cfg(not(target_arch = "wasm32"))]
pub mod macrodep;
#[cfg(not(target_arch = "wasm32"))]
pub mod macros;
pub mod meta;
#[cfg(not(target_arch = "wasm32"))]
pub mod repl;
#[cfg(not(target_arch = "wasm32"))]
pub mod runner;
#[cfg(not(target_arch = "wasm32"))]
pub mod scaffold;
#[cfg(not(target_arch = "wasm32"))]
pub mod staging;
#[cfg(not(target_arch = "wasm32"))]
pub mod tools;

pub mod wit;
pub mod witdep;

#[cfg(all(target_arch = "wasm32", feature = "playground"))]
pub mod wasm;

pub use form::{Arena, Node, NodeId};
pub use lexer::ReadError;
pub use printer::print;
pub use reader::read_file;

use std::cell::RefCell;

thread_local! {

    static OUTPUT_SINK: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn output_capture_start() {
    OUTPUT_SINK.with(|s| *s.borrow_mut() = Some(String::new()));
}

pub fn output_capture_take() -> String {
    OUTPUT_SINK.with(|s| s.borrow_mut().take().unwrap_or_default())
}

pub fn emit_output(text: &str, newline: bool) {
    OUTPUT_SINK.with(|s| {
        let mut slot = s.borrow_mut();
        if let Some(buf) = slot.as_mut() {
            buf.push_str(text);
            if newline {
                buf.push('\n');
            }
        } else if newline {
            println!("{text}");
        } else {
            print!("{text}");
        }
    });
}

#[derive(Debug, Clone)]
pub struct EvalOutcome {
    pub ok: bool,

    pub value: String,

    pub output: String,

    pub error: String,
}

pub fn eval_snippet(src: &str) -> EvalOutcome {
    use std::rc::Rc;
    use value::{Env, Value, print_value, unit};

    let (arena, roots) = match read_file(src) {
        Ok(pair) => pair,
        Err(e) => {
            return EvalOutcome {
                ok: false,
                value: String::new(),
                output: String::new(),
                error: e.to_string(),
            };
        }
    };

    let was_strict = check::strict();
    if was_strict {
        check::set_strict(false);
    }
    let resolved = check::resolve_overloads(arena, &roots);
    if was_strict {
        check::set_strict(true);
    }
    let (arena, roots) = match resolved {
        Ok(pair) => pair,
        Err(msg) => {
            return EvalOutcome {
                ok: false,
                value: String::new(),
                output: String::new(),
                error: msg,
            };
        }
    };

    {
        let copy = Arena {
            nodes: arena.nodes.clone(),
            spans: arena.spans.clone(),
        };
        if let Ok((xarena, xroots)) = expand::expand_file(copy, &roots, None)
            && let Err(msg) = check::check_program(&xarena, &xroots)
        {
            return EvalOutcome {
                ok: false,
                value: String::new(),
                output: String::new(),
                error: msg,
            };
        }
    }
    let arena = Rc::new(arena);

    let interp = interp::Interp::new();
    let env = Env::root();
    builtins::install(&env);

    output_capture_start();
    let mut last = unit();
    for root in roots {
        match interp.eval(&arena, root, &env) {
            Ok(v) => last = v,
            Err(e) => {
                let _ = interp.teardown();
                return EvalOutcome {
                    ok: false,
                    value: String::new(),
                    output: output_capture_take(),
                    error: e.to_string(),
                };
            }
        }
    }
    if let Err(error) = interp.teardown() {
        return EvalOutcome {
            ok: false,
            value: String::new(),
            output: output_capture_take(),
            error: error.to_string(),
        };
    }
    let output = output_capture_take();

    let value = if matches!(&last, Value::Rec(f) if f.is_empty()) {
        String::new()
    } else {
        print_value(&last)
    };
    EvalOutcome {
        ok: true,
        value,
        output,
        error: String::new(),
    }
}

#[cfg(test)]
mod tests;
