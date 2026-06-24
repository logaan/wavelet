//! Regression tests for §4: the type checker now runs on **every** path, not
//! only the playground (`eval_snippet`).
//!
//! Before this fix, `check::resolve_overloads` was invoked from exactly one
//! place — `eval_snippet`, the playground/wasm path. As a result:
//!
//!   * `wavelet run` never type-checked and bound same-named `Def`s by ordinary
//!     last-wins shadowing, so an overloaded call reached whichever def was read
//!     last and failed or silently ran the WRONG body.
//!   * `wavelet build` did no type checking, so an ill-typed program reached the
//!     emitter rather than being rejected up front.
//!   * `wavelet wit` did no type checking either.
//!
//! These tests drive the built `wavelet` binary (`CARGO_BIN_EXE_wavelet`) — the
//! same pattern `tests/compose.rs` and `tests/wit_expand.rs` use — and assert
//! that each subcommand now rejects an ill-typed program, that a well-typed one
//! still succeeds (guarding against a checker that rejects everything), and that
//! `wavelet run` dispatches an overloaded call to the *correct* member rather
//! than by last-wins shadowing.

use std::path::PathBuf;
use std::process::{Command, Output};

/// A fresh temp directory unique to this test, cleaned on entry.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wavelet-checker-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Write `src` to `<dir>/<name>` and return its path.
fn write_src(dir: &PathBuf, name: &str, src: &str) -> PathBuf {
    let file = dir.join(name);
    std::fs::write(&file, src).expect("write source file");
    file
}

/// Run `wavelet <subcommand> <file>` and capture its output.
fn run_wavelet(subcommand: &str, file: &PathBuf) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wavelet"))
        .arg(subcommand)
        .arg(file)
        .output()
        .unwrap_or_else(|e| panic!("run `wavelet {subcommand}`: {e}"))
}

// ---------------------------------------------------------------------------
// build
// ---------------------------------------------------------------------------

#[test]
fn build_rejects_an_ill_typed_program() {
    let dir = scratch("build-bad");
    // `add` of two strings has no WIT type. The body is uncalled, so before the
    // fix the program emitted a component; now the checker rejects it pre-emit.
    let file = write_src(
        &dir,
        "bad.wvl",
        r#"Package "demo:bad@0.1.0"
Export run
Def run Fn {} add("a" "b")
"#,
    );

    let out = run_wavelet("build", &file);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "`wavelet build` accepted an ill-typed program (exit {:?})\nstderr:\n{stderr}",
        out.status.code(),
    );
    assert!(
        stderr.contains("numeric"),
        "build error should name the type problem (numeric operands); got:\n{stderr}",
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn build_still_succeeds_on_a_well_typed_program() {
    let dir = scratch("build-good");
    let file = write_src(
        &dir,
        "good.wvl",
        r#"Package "demo:good@0.1.0"
Export shout
Def shout Fn {phrase: string}
  str-cat(upper(phrase) "!")
"#,
    );

    let out = run_wavelet("build", &dir.join("good.wvl"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "`wavelet build` wrongly rejected a well-typed program (exit {:?})\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status.code(),
    );
    let _ = file;
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// wit
// ---------------------------------------------------------------------------

#[test]
fn wit_rejects_an_ill_typed_program() {
    let dir = scratch("wit-bad");
    let file = write_src(
        &dir,
        "bad.wvl",
        r#"Package "demo:bad@0.1.0"
Export run
Def run Fn {} add("a" "b")
"#,
    );

    let out = run_wavelet("wit", &file);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "`wavelet wit` accepted an ill-typed program (exit {:?})\nstderr:\n{stderr}",
        out.status.code(),
    );
    assert!(
        stderr.contains("numeric"),
        "wit error should name the type problem (numeric operands); got:\n{stderr}",
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wit_still_succeeds_on_a_well_typed_program() {
    let dir = scratch("wit-good");
    let file = write_src(
        &dir,
        "good.wvl",
        r#"Package "demo:good@0.1.0"
Export shout
Def shout Fn {phrase: string}
  str-cat(upper(phrase) "!")
"#,
    );

    let out = run_wavelet("wit", &file);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "`wavelet wit` wrongly rejected a well-typed program (exit {:?})\nstderr:\n{stderr}",
        out.status.code(),
    );
    assert!(stdout.contains("shout: func(phrase: string) -> string"), "{stdout}");

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

#[test]
fn run_rejects_an_ill_typed_program() {
    let dir = scratch("run-bad");
    // `run` is a valid no-arg entry closure, but the uncalled `bad` def is
    // ill-typed. The checker rejects the whole module before evaluation.
    let file = write_src(
        &dir,
        "bad.wvl",
        r#"Package "demo:bad@0.1.0"
Def bad Fn {} add("a" "b")
Export run
Def run Fn {} ok(0)
"#,
    );

    let out = run_wavelet("run", &file);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "`wavelet run` accepted an ill-typed program (exit {:?})\nstderr:\n{stderr}",
        out.status.code(),
    );
    assert!(
        stderr.contains("numeric"),
        "run error should name the type problem (numeric operands); got:\n{stderr}",
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn run_resolves_an_overloaded_call_to_the_correct_member() {
    let dir = scratch("run-overload");
    // Two `show` overloads. The *first* takes an `s32` and is the identity; the
    // *second* takes a `string` and calls `upper`, which fails at runtime on a
    // non-string. `run` calls `show(5)`.
    //
    //   * Correct (argument-directed) resolution dispatches `show(5)` to the
    //     s32 member, which runs cleanly: `wavelet run` exits 0.
    //   * The old last-wins shadowing would bind `show` to the *string* member,
    //     so `show(5)` would evaluate `upper(5)` and fail at runtime: a non-zero
    //     exit. Success therefore proves the overload was resolved by type, not
    //     by which def was read last.
    let file = write_src(
        &dir,
        "overload.wvl",
        r#"Package "demo:over@0.1.0"
Def show Fn {x: s32} x
Def show Fn {x: string} upper(x)
Export run
Def run Fn {} Do [show(5) ok(0)]
"#,
    );

    let out = run_wavelet("run", &file);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "`wavelet run` did not resolve `show(5)` to the s32 member \
         (last-wins shadowing would call the string overload and fail) — exit {:?}\nstderr:\n{stderr}",
        out.status.code(),
    );

    let _ = std::fs::remove_dir_all(&dir);
}
