pub mod builtins;
pub mod check;
pub mod compiler;
pub mod expand;
pub mod form;
pub(crate) mod interp;
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
pub mod metadata;
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

#[cfg(not(target_arch = "wasm32"))]
pub fn eval_snippet(src: &str) -> EvalOutcome {
    match evaluate_component(src) {
        Ok(value) => EvalOutcome {
            ok: true,
            value,
            output: String::new(),
            error: String::new(),
        },
        Err(error) => EvalOutcome {
            ok: false,
            value: String::new(),
            output: String::new(),
            error,
        },
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn evaluate_component(src: &str) -> Result<String, String> {
    let compilation =
        compiler::native::compile(&compiler::Request::snippet(src), std::path::Path::new("."))
            .map_err(compiler::native::display_diagnostics)?;
    let artifact = compilation.entry_artifact().ok_or("no entry artifact")?;
    let invocation = compilation
        .invocation
        .as_ref()
        .ok_or("no snippet invocation")?;
    let mut component = host::HostComponent::from_bytes(&artifact.bytes)
        .map_err(|error| format!("instantiate: {error}"))?;
    let values = component
        .call_instance(&invocation.interface_name, &invocation.function_name, &[])
        .map_err(|error| format!("call: {error}"))?;
    match values.as_slice() {
        [host::Val::String(value)] => Ok(value.clone()),
        other => Err(format!("unexpected snippet result: {other:?}")),
    }
}

#[cfg(test)]
mod tests;
