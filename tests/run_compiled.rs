use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

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

#[test]
fn single_file_runs_compiled() {
    let paths = stage(&[(
        "hello.wlt",
        "Package \"demo:hello@0.1.0\"\nExport run\nDef run Fn {}\n  add(1 2)\n",
    )]);
    wavelet::runner::run_files_compiled(&paths)
        .expect("a self-contained program runs on the compiled path");
}

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
             Def shout Fn {phrase: string}\n  str-cat(phrase \"!\")\n",
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

#[test]
fn dispatcher_reports_missing_export() {
    let paths = stage(&[(
        "norun.wlt",
        "Package \"demo:norun@0.1.0\"\nDef foo Fn {} 42\n",
    )]);
    let err = wavelet::runner::run(&paths).expect_err("a program with no `run` must fail");
    assert!(
        err.contains("run"),
        "expected missing run diagnostic; got: {err}"
    );
}

#[test]
fn dispatcher_surfaces_a_runtime_error() {
    let paths = stage(&[(
        "err.wlt",
        "Package \"demo:err@0.1.0\"\nExport run\nDef run Fn {}\n  div(1 0)\n",
    )]);
    let err = wavelet::runner::run(&paths).expect_err("division by zero must fail");
    assert!(
        err.contains("call") || err.contains("trap") || err.contains("division by zero"),
        "expected a readable arithmetic error; got: {err}"
    );
}

#[test]
fn dispatcher_runs_a_good_program() {
    let paths = stage(&[(
        "ok.wlt",
        "Package \"demo:ok@0.1.0\"\nExport run\nDef run Fn {}\n  add(2 3)\n",
    )]);
    wavelet::runner::run(&paths).expect("a good program runs");
}

#[test]
fn dispatcher_does_not_execute_an_unexported_entry() {
    let paths = stage(&[(
        "private.wlt",
        "Package \"demo:private@0.1.0\"\nDef run Fn {} 42\n",
    )]);
    assert!(wavelet::runner::run_files(&paths).is_err());
    assert!(wavelet::runner::run(&paths).is_err());
}
