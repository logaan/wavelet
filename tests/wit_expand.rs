use std::path::PathBuf;
use std::process::Command;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wavelet-wit-expand-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

#[test]
fn wit_command_expands_derive_before_synthesizing() {
    let dir = scratch("derive");
    let file = dir.join("demo.wlt");
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

    assert!(
        stdout.contains("eq-point"),
        "expected synthesized WIT to contain the derived `eq-point`; got:\n{stdout}",
    );

    let _ = std::fs::remove_dir_all(&dir);
}
