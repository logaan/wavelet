//! Browser playground bindings.
//!
//! Compiled to `wasm32-unknown-unknown` with `wasm-bindgen` and loaded by the
//! documentation site's `<Playground>` component, so the examples throughout
//! the docs are editable and runnable in the reader's browser — the same
//! tree-walking interpreter that backs `wavelet run`, nothing simulated.
//!
//! The actual evaluation lives in [`crate::eval_snippet`], shared with the
//! documentation-example test suite (`tests/examples.rs`); this module only
//! adds the wasm boundary and JSON serialisation.

use wasm_bindgen::prelude::*;

/// Evaluate a snippet of Wavelet source and return a JSON object string:
///
/// ```json
/// {"ok": true, "value": "...", "output": "...", "error": ""}
/// ```
#[wasm_bindgen]
pub fn eval(src: &str) -> String {
    console_error_panic_hook::set_once();
    let r = crate::eval_snippet(src);
    format!(
        "{{\"ok\":{},\"value\":{},\"output\":{},\"error\":{}}}",
        r.ok,
        json_string(&r.value),
        json_string(&r.output),
        json_string(&r.error),
    )
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
