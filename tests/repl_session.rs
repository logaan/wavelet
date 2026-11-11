use std::io::Write;
use std::process::{Command, Stdio};

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

#[test]
fn values_evaluate_on_the_compiled_path() {
    let (stdout, stderr) = repl("add(1 2)\nstr-cat(\"a\" \"b\")\n[1 2 3]\n{a: 1 b: 2}\n");
    assert_eq!(stdout, "3\n\"ab\"\n[1, 2, 3]\n{a: 1, b: 2}\n", "stdout");
    assert!(
        !stderr.contains("fallback"),
        "plain values must not fall back to the interpreter; stderr:\n{stderr}"
    );
}

#[test]
fn definitions_accumulate_across_lines() {
    let (stdout, stderr) = repl("Def dbl Fn {x: s64} mul(x 2)\ndbl(21)\n");
    assert_eq!(stdout, "{}\n42\n", "stdout");
    assert!(
        !stderr.contains("fallback"),
        "an integer function call must not fall back; stderr:\n{stderr}"
    );
}

#[test]
fn a_macro_defined_earlier_expands_later() {
    let script =
        "DefMacro and2 {a b}\n  Quasi If Unquote(a) Unquote(b) false\nAnd2 lt(3 10) gt(3 0)\n";
    let (stdout, _stderr) = repl(script);
    assert_eq!(stdout, "{}\ntrue\n", "stdout");
}

#[test]
fn a_float_result_stays_on_the_compiled_path() {
    let (stdout, stderr) = repl("div(10.0 4.0)\n");
    assert_eq!(stdout, "2.5\n", "stdout");
    assert!(
        !stderr.contains("fallback"),
        "a float result must not fall back; stderr:\n{stderr}"
    );
}

#[test]
fn an_error_line_is_reported_and_the_session_continues() {
    let (stdout, stderr) = repl("add(1 2)\nnope(1)\nadd(3 4)\n");
    assert!(stdout.contains("3\n"), "first line: stdout:\n{stdout}");
    assert!(
        stdout.contains("7\n"),
        "line after the error: stdout:\n{stdout}"
    );
    assert!(
        stderr.contains("error") || stderr.contains("fallback"),
        "the bad line should surface an error; stderr:\n{stderr}"
    );
}

#[test]
fn repeated_expressions_reexecute_and_definitions_invalidate_cached_components() {
    let (stdout, stderr) = repl("Def n 2\nn\nn\nDef n 7\nn\n");
    assert_eq!(stdout, "{}\n2\n2\n{}\n7\n");
    assert!(!stderr.contains("error:"), "{stderr}");
}
