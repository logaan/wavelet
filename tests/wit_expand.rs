use std::path::PathBuf;
use std::process::Command;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wavelet-wit-expand-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

#[test]
fn wit_command_rejects_unimplemented_derivation() {
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
        !output.status.success(),
        "unsupported Derive succeeded: {stdout}"
    );
    assert!(stderr.contains("`Derive` is not implemented"), "{stderr}");
    assert!(
        stdout.is_empty(),
        "failed synthesis emitted partial output: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
