//! Tests for the compiled `wavelet run` path (goal 6.5).
//!
//! `runner::run_files_compiled` builds the program through the real emitter,
//! composes any runtime imports, instantiates the artifact in the
//! capability-free host, and calls the exported `run` — the wasm analogue of the
//! interpreter's "look up `run`, apply it". `runner::run` wraps it with an
//! interpreter fallback so a program the backend cannot yet run still behaves as
//! it did before. The interpreter path (`runner::run_files`) is retained as the
//! semantics oracle and the fallback; these tests pin the compiled path directly
//! and the dispatcher's fallback contract.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Stage `files` (name, source) into a fresh temp `src/` and return their paths,
/// entry first. The caller keeps the returned dir alive; it is cleaned by the OS
/// temp reaper (each test uses a unique dir).
fn stage(files: &[(&str, &str)]) -> Vec<String> {
    let dir = std::env::temp_dir().join(format!(
        "wavelet-run-test-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    files
        .iter()
        .map(|(name, body)| {
            let p: PathBuf = src.join(name);
            std::fs::write(&p, body).unwrap();
            p.to_string_lossy().into_owned()
        })
        .collect()
}

/// A single self-contained program runs on the compiled path (no fallback).
#[test]
fn single_file_runs_compiled() {
    let paths = stage(&[(
        "hello.wlt",
        "Package \"demo:hello@0.1.0\"\nExport run\nDef run Fn {}\n  add(1 2)\n",
    )]);
    wavelet::runner::run_files_compiled(&paths)
        .expect("a self-contained program runs on the compiled path");
}

/// A multi-file program with a runtime import is composed and run on the
/// compiled path — the wasm analogue of `run_files`' interpreter stand-in for
/// `wavelet compose`. (Composition needs `wac`; if it is absent the build leaves
/// no `app.wasm` and this returns an error, which is why the dispatcher falls
/// back — but in this repo's toolchain `wac` is present.)
#[test]
fn multi_file_import_runs_compiled() {
    let paths = stage(&[
        (
            "main.wlt",
            "Package \"demo:main@0.1.0\"\n\
             Import {pkg: \"demo:shout/api\" as: sh}\n\
             Export run\n\
             Def run Fn {}\n  str-cat(sh/shout({phrase: \"hello\"}))\n",
        ),
        (
            "shout.wlt",
            "Package \"demo:shout@0.1.0\"\n\
             Export shout\n\
             Def shout Fn {phrase: string}\n  str-cat(upper(phrase) \"!\")\n",
        ),
    ]);
    match wavelet::runner::run_files_compiled(&paths) {
        Ok(()) => {}
        Err(e) if e.contains("wac") => {
            eprintln!("skipping: composition tool unavailable: {e}");
        }
        Err(e) => panic!("multi-file compiled run failed: {e}"),
    }
}

/// The dispatcher falls back to the interpreter for a program the backend cannot
/// run, preserving today's exact error: a `run`-less program reports "nothing to
/// run" and fails.
#[test]
fn dispatcher_reports_missing_run_via_fallback() {
    let paths = stage(&[(
        "norun.wlt",
        "Package \"demo:norun@0.1.0\"\nDef foo Fn {} 42\n",
    )]);
    let err = wavelet::runner::run(&paths).expect_err("a program with no `run` must fail");
    assert!(
        err.contains("nothing to run"),
        "expected the interpreter's clear diagnostic; got: {err}"
    );
}

/// A genuine runtime error (division by zero) surfaces the interpreter's clear
/// message rather than a bare wasm trap — the compiled path traps, the fallback
/// re-runs and returns the readable error (F5).
#[test]
fn dispatcher_surfaces_a_runtime_error() {
    let paths = stage(&[(
        "err.wlt",
        "Package \"demo:err@0.1.0\"\nExport run\nDef run Fn {}\n  div(1 0)\n",
    )]);
    let err = wavelet::runner::run(&paths).expect_err("division by zero must fail");
    assert!(
        err.contains("division by zero") || err.contains("overflow"),
        "expected a readable arithmetic error; got: {err}"
    );
}

/// A program that runs fine on both paths succeeds through the dispatcher.
#[test]
fn dispatcher_runs_a_good_program() {
    let paths = stage(&[(
        "ok.wlt",
        "Package \"demo:ok@0.1.0\"\nExport run\nDef run Fn {}\n  add(2 3)\n",
    )]);
    wavelet::runner::run(&paths).expect("a good program runs");
}
