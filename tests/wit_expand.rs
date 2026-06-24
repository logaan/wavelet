//! Regression test for §8: `wavelet wit` must expand macros before synthesizing
//! WIT, exactly as `wavelet build` does.
//!
//! The bug: `wit_cmd` synthesized straight from `read_file` output without
//! running expansion, so `Derive` (and any foreign macro) never ran on the
//! `wavelet wit` path. A `Derive {Eq} point` program that `wavelet build`
//! compiles failed under `wavelet wit` with `Export eq-point has no definition`,
//! because the derived `eq-point` `Def` was never produced. This drove the two
//! CLI subcommands to disagree about the same source.
//!
//! This runs the built `wavelet` binary (`CARGO_BIN_EXE_wavelet`) on a temp file
//! containing a `Derive` program and asserts the synthesized WIT contains the
//! derived `eq-point` — i.e. that expansion happened before synthesis.

use std::path::PathBuf;
use std::process::Command;

/// A fresh temp directory unique to this test, cleaned on entry.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wavelet-wit-expand-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

#[test]
fn wit_command_expands_derive_before_synthesizing() {
    let dir = scratch("derive");
    let file = dir.join("demo.wvl");
    std::fs::write(
        &file,
        r#"Package "demo:geo@0.1.0"
Export eq-point
DefType point {x: s32 y: s32}
Derive {Eq} point
"#,
    )
    .expect("write source file");

    let output = Command::new(env!("CARGO_BIN_EXE_wavelet"))
        .arg("wit")
        .arg(&file)
        .output()
        .expect("run `wavelet wit`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "`wavelet wit` failed (exit {:?}):\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );
    // The derived operation is only present if `Derive {Eq}` expanded before
    // synthesis; without expansion the export had no definition and the command
    // errored.
    assert!(
        stdout.contains("eq-point"),
        "expected synthesized WIT to contain the derived `eq-point`; got:\n{stdout}",
    );

    let _ = std::fs::remove_dir_all(&dir);
}
