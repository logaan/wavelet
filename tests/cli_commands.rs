use std::io::Write;
use std::process::{Command, Output, Stdio};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wavelet"))
        .args(args)
        .output()
        .expect("run wavelet")
}

#[test]
fn version_aliases_and_usage_have_stable_exit_codes() {
    for alias in ["--version", "-V", "version"] {
        let output = cli(&[alias]);
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            format!("wavelet {}", env!("CARGO_PKG_VERSION"))
        );
    }
    for args in [vec![], vec!["unknown-command"]] {
        let output = cli(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("usage: wavelet"));
    }
}

#[test]
fn source_commands_report_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.wlt");
    for command in ["read", "expand", "wit"] {
        let output = cli(&[command, missing.to_str().unwrap()]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("cannot read"), "{stderr}");
        assert!(stderr.contains("missing.wlt"), "{stderr}");
    }
}

#[test]
fn read_accepts_stdin_and_reports_invalid_source() {
    for (source, expected_code) in [("[1 2 3]", 0), ("[", 1)] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_wavelet"))
            .arg("read")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(source.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(expected_code));
        if expected_code == 0 {
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "[1, 2, 3]");
        } else {
            assert!(String::from_utf8_lossy(&output.stderr).contains("<stdin>:"));
        }
    }
}

#[test]
fn source_commands_read_expand_and_synthesize_valid_programs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("demo.wlt");
    std::fs::write(
        &path,
        "Package \"demo:cli@0.1.0\"\nExport answer\nDef answer Fn {} The s32 42\n",
    )
    .unwrap();
    for command in ["read", "expand", "wit"] {
        let output = cli(&["--strict", command, path.to_str().unwrap()]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("answer"), "{stdout}");
        if command == "wit" {
            assert!(stdout.contains("answer: func() -> s32"), "{stdout}");
        } else {
            assert!(stdout.contains("42"), "{stdout}");
        }
    }
    std::fs::write(&path, "[").unwrap();
    for command in ["read", "expand", "wit"] {
        let output = cli(&[command, path.to_str().unwrap()]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("demo.wlt:"));
    }
}

#[test]
fn new_reports_invalid_arguments_without_creating_a_project() {
    for (args, diagnostic) in [
        (vec!["new", "demo", "--type"], "needs a value"),
        (vec!["new", "--unknown"], "unknown option"),
        (vec!["new", "demo", "extra"], "unexpected argument"),
        (vec!["new", "--type=cli"], "missing project name"),
        (vec!["new", "demo", "-t", "unknown"], "new:"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_wavelet"))
            .args(args)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(diagnostic), "{stderr}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
