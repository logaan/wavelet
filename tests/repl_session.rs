//! Behaviour tests for the compiled-first `wavelet repl` (goal 6, Step 2).
//!
//! These drive the built `wavelet` binary (`CARGO_BIN_EXE_wavelet`) with a
//! scripted stdin and check its stdout/stderr, exactly as a user would. The
//! REPL evaluates each bare expression by compiling the accumulated session and
//! running it in wasm (the guest's `to-string` renders the value), and prints a
//! definition's unit echo. The output must match what the interpreter REPL
//! always produced — that is the whole point of the flip — while the
//! decision-gated backend holes (here: float `to-string`) transparently fall
//! back to the interpreter for that entry, noted on stderr.

use std::io::Write;
use std::process::{Command, Stdio};

/// Feed `script` to `wavelet repl` on stdin and return `(stdout, stderr)`.
fn repl(script: &str) -> (String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_wavelet"))
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn `wavelet repl`");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(script.as_bytes())
        .expect("write repl script");
    let out = child.wait_with_output().expect("wait for repl");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Integers, strings, lists and records evaluate on the **compiled** path and
/// print exactly what the interpreter's `print_value` printed — no fallback.
#[test]
fn values_evaluate_on_the_compiled_path() {
    let (stdout, stderr) = repl("add(1 2)\nstr-cat(\"a\" \"b\")\n[1 2 3]\n{a: 1 b: 2}\n");
    assert_eq!(stdout, "3\n\"ab\"\n[1, 2, 3]\n{a: 1, b: 2}\n", "stdout");
    assert!(
        !stderr.contains("fallback"),
        "plain values must not fall back to the interpreter; stderr:\n{stderr}"
    );
}

/// A definition accumulates and echoes unit (`{}`); a later expression uses it,
/// still on the compiled path.
#[test]
fn definitions_accumulate_across_lines() {
    let (stdout, stderr) = repl("Def dbl Fn {x: s64} mul(x 2)\ndbl(21)\n");
    assert_eq!(stdout, "{}\n42\n", "stdout");
    assert!(
        !stderr.contains("fallback"),
        "an integer function call must not fall back; stderr:\n{stderr}"
    );
}

/// A macro defined on one line expands when invoked on a later line — the
/// accumulated `DefMacro` is compiled into the program's macro set on recompile
/// (6.4.5).
#[test]
fn a_macro_defined_earlier_expands_later() {
    let script = "DefMacro and2 {a b}\n  Quasi If Unquote(a) Unquote(b) false\nAnd2 lt(3 10) gt(3 0)\n";
    let (stdout, _stderr) = repl(script);
    assert_eq!(stdout, "{}\ntrue\n", "stdout");
}

/// A float result stays on the compiled path: the backend's `to-string` now
/// has a float arm (0.2), so what used to trap — and announce an interpreter
/// fallback — just prints.
#[test]
fn a_float_result_stays_on_the_compiled_path() {
    let (stdout, stderr) = repl("div(10.0 4.0)\n");
    assert_eq!(stdout, "2.5\n", "stdout");
    assert!(
        !stderr.contains("fallback"),
        "a float result must not fall back; stderr:\n{stderr}"
    );
}

/// An error line is reported on stderr and does not stop the session — a later
/// good line still evaluates.
#[test]
fn an_error_line_is_reported_and_the_session_continues() {
    let (stdout, stderr) = repl("add(1 2)\nnope(1)\nadd(3 4)\n");
    assert!(stdout.contains("3\n"), "first line: stdout:\n{stdout}");
    assert!(stdout.contains("7\n"), "line after the error: stdout:\n{stdout}");
    assert!(
        stderr.contains("error") || stderr.contains("fallback"),
        "the bad line should surface an error; stderr:\n{stderr}"
    );
}
