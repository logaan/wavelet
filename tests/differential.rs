//! F4 — the permanent differential harness for program *execution*.
//!
//! Every runnable documentation example (`docs/examples.json`, the corpus
//! `tests/examples.rs` already locks against the interpreter) is executed
//! **two** ways and the outcomes must agree:
//!
//! 1. the **interpreter** (`eval_snippet`) — the semantics oracle
//!    (`CLAUDE.md`), kept for the life of the project; and
//! 2. the **compiled artifact** — the snippet is wrapped into a self-contained
//!    component (declarations stay top-level; expression forms are sequenced
//!    into an exported `differential-main: func() -> string` whose body is
//!    `to-string(Do [ … ])`), built through the real emitter
//!    (`build::build_files`), instantiated in the capability-free `wasmtime`
//!    host, and called.
//!
//! Because the backend's `to-string` is the port of the interpreter's
//! `print_value` — the same printer that produced each example's recorded
//! `value` — agreement is **exact string equality** on the printed final value.
//! Error examples must fail on both sides (the compiled side at any stage:
//! type check, emit, instantiation, or a runtime trap).
//!
//! The wasm backend cannot express every example yet. Those live in [`SKIP`],
//! each with the reason it diverges today. Skipped examples still *run*: one
//! that starts agreeing fails the suite until its entry is removed, so the
//! list only ever shrinks. Sister harness: `tests/macro_differential.rs`
//! (macro *expansion*); this file is the execution half of F4.

use wavelet::form::{Arena, Node, NodeId};
use wavelet::host::{HostComponent, Val};
use wavelet::printer::print;
use wavelet::reader::read_file;

/// The backend's `to-string` now ports `print_value` over the box layout for
/// int/bool/string and every compound; only `float` and `char` values still
/// hit `unreachable` (Rust's `{f:?}` shortest round-trip and `{c:?}` UTF-8
/// escaping are not yet reimplemented in wasm). A float/char *anywhere* in the
/// value keeps the example on this SKIP.
const TOSTR_TRAP: &str = "backend `to-string` still traps on float/char values (5.6)";
/// A stdlib builtin the wasm backend does not implement yet (5.6/5.7).
const BUILTIN: &str = "stdlib builtin missing from the wasm backend (5.6/5.7)";
/// The stdlib constant `pi` has no module-level binding in the backend (5.7).
const PI: &str = "stdlib constant `pi` is not bound in the wasm backend (5.7)";
/// Flag literals are rejected on this emit path (5.4).
const FLAGS: &str = "flag literals rejected on this emit path (5.4)";
/// On the build path `expand` is confined to macro libraries (6.1.8).
const EXPAND: &str = "`expand` is only available inside a macro library when building (6.1.8)";

/// Examples the compiled artifact is known to disagree on, with the current
/// reason. Removing a fixed entry is part of fixing the backend gap.
const SKIP: &[(&str, &str)] = &[
    ("eval-apply-list", BUILTIN),     // apply
    ("macro-expand", EXPAND),
    ("sf-def", TOSTR_TRAP), // float result
    ("sf-let", PI),
    ("std-apply", BUILTIN),           // apply
    ("std-char-conv", BUILTIN),       // to-u32
    ("std-conv", BUILTIN),            // to-u8
    ("std-div-float", TOSTR_TRAP),    // float result
    ("std-map", BUILTIN),             // map
    ("std-pi", PI),
    ("std-read", BUILTIN),           // read
    ("std-seq-basics", BUILTIN),     // reverse
    ("std-seq-mutate", BUILTIN),     // get
    ("std-strings", BUILTIN),     // split
    ("std-zip", BUILTIN),         // zip
    ("values-atoms", FLAGS),
];

const PACKAGE: &str = "docs:snippet@0.1.0";
const IFACE: &str = "docs:snippet/api@0.1.0";
const MAIN: &str = "differential-main";

/// Top-level declaration heads (post-reader `-MACRO` spellings): forms that
/// declare rather than evaluate, and therefore stay top-level in the wrapped
/// component. Everything else is an expression and moves into the `Do`.
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

/// Wrap a documentation snippet into a self-contained component source.
///
/// The reader's printed forms are canonical and re-readable, so the wrapped
/// program is reconstructed from the parsed tree: declarations in original
/// order, then the expression forms sequenced in a `Do` inside the exported
/// entry point. A snippet whose *last* form is an expression reports
/// `to-string` of the `Do`'s value; one that ends in a declaration reports
/// `""`, matching `eval_snippet`'s suppressed unit.
fn wrap_snippet(code: &str) -> Result<String, String> {
    let (arena, roots) = read_file(code).map_err(|e| format!("read: {e}"))?;
    let mut decls = Vec::new();
    let mut exprs = Vec::new();
    for &root in &roots {
        if is_decl(&arena, root) {
            decls.push(print(&arena, root));
        } else {
            exprs.push(print(&arena, root));
        }
    }
    let ends_in_expr = roots.last().is_some_and(|&r| !is_decl(&arena, r));

    let mut out = format!("Package \"{PACKAGE}\"\n\n");
    for d in &decls {
        out.push_str(d);
        out.push('\n');
    }
    out.push_str(&format!("\nExport {{name: {MAIN} result: string}}\n"));
    out.push_str(&format!("Def {MAIN} Fn {{}}\n"));
    let body = if ends_in_expr {
        format!("to-string(Do [{}])", exprs.join("\n    "))
    } else if exprs.is_empty() {
        "\"\"".to_string()
    } else {
        // Effects (and errors) of the expressions still run; the snippet's
        // final value is the trailing declaration's unit, printed as "".
        format!("Do [{}\n    \"\"]", exprs.join("\n    "))
    };
    out.push_str(&format!("  {body}\n"));
    Ok(out)
}

/// Build the wrapped program through the real emitter and execute its entry
/// point, returning the printed final value. `Err` carries the failing stage.
fn compiled_eval(id: &str, program: &str) -> Result<String, String> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "wavelet-differential-{}-{n}-{id}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).map_err(|e| format!("setup: {e}"))?;
    let path = src.join("snippet.wlt");
    std::fs::write(&path, program).map_err(|e| format!("setup: {e}"))?;

    let out_dir = dir.join("out");
    let result = (|| {
        let outputs = wavelet::build::build_files(
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

/// One example's differential outcome.
enum Outcome {
    /// Both sides agree (matching value, or both failing).
    Agree,
    /// The sides disagree; the string says how.
    Disagree(String),
}

fn run_example(id: &str, code: &str, expect_error: bool) -> Outcome {
    let interp = wavelet::eval_snippet(code);
    assert_eq!(
        interp.ok, !expect_error,
        "`{id}`: interpreter outcome drifted from examples.json — \
         tests/examples.rs should be failing too"
    );

    let compiled = wrap_snippet(code).and_then(|program| compiled_eval(id, &program));

    match (expect_error, compiled) {
        (true, Err(_)) => Outcome::Agree,
        (true, Ok(v)) => Outcome::Disagree(format!(
            "interpreter errors (`{}`) but the compiled artifact succeeds with {v:?}",
            interp.error
        )),
        (false, Err(e)) => Outcome::Disagree(format!(
            "interpreter succeeds with {:?} but the compiled artifact fails at {e}",
            interp.value
        )),
        (false, Ok(v)) => {
            // `eval_snippet` suppresses a final unit as ""; the compiled side
            // prints it as "{}". The two spell the same value.
            if v == interp.value || (interp.value.is_empty() && v == "{}") {
                Outcome::Agree
            } else {
                Outcome::Disagree(format!(
                    "value mismatch\n    interpreter: {:?}\n    compiled:    {v:?}",
                    interp.value
                ))
            }
        }
    }
}

#[test]
fn every_documented_example_agrees_with_the_compiled_artifact() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/examples.json");
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    let examples: serde_json::Value =
        serde_json::from_str(&text).expect("docs/examples.json is not valid JSON");
    let map = examples
        .as_object()
        .expect("examples.json must be an object");
    assert!(!map.is_empty(), "no examples found");

    let mut ids: Vec<&String> = map.keys().collect();
    ids.sort();

    let mut divergences = Vec::new();
    let mut ratchet = Vec::new();
    for id in ids {
        let entry = &map[id.as_str()];
        let code = entry["code"].as_str().unwrap_or_else(|| {
            panic!("example `{id}` has no string `code`");
        });
        let expect_error = entry.get("error").is_some();
        let skip = SKIP.iter().find(|(s, _)| s == id).map(|(_, why)| *why);

        match (run_example(id, code, expect_error), skip) {
            (Outcome::Agree, None) => {}
            (Outcome::Agree, Some(_)) => ratchet.push(id.clone()),
            (Outcome::Disagree(_), Some(_)) => {} // known gap, documented in SKIP
            (Outcome::Disagree(how), None) => {
                divergences.push(format!("`{id}`: {how}"));
            }
        }
    }

    let mut report = String::new();
    if !divergences.is_empty() {
        report.push_str(&format!(
            "\n{} example(s) diverged between the interpreter and the compiled artifact:\n\n{}\n",
            divergences.len(),
            divergences.join("\n\n"),
        ));
    }
    if !ratchet.is_empty() {
        report.push_str(&format!(
            "\n{} SKIP-listed example(s) now agree — remove them from SKIP in {}:\n  {}\n",
            ratchet.len(),
            file!(),
            ratchet.join("\n  "),
        ));
    }
    assert!(report.is_empty(), "{report}");
}
