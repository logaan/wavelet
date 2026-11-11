use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

fn stage(name: &str, body: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "wavelet-shebang-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let path = src.join(name);
    std::fs::write(&path, body).unwrap();
    (dir, path)
}

#[test]
fn reader_skips_leading_shebang() {
    let src = "#!/usr/bin/env wavelet\nPackage \"demo:hi@0.1.0\"\nExport run\n\
               Def run Fn {}\n  add(1 2)\n";
    let (arena, roots) = wavelet::read_file(src).expect("shebang'd source reads");

    let first = wavelet::print(&arena, roots[0]);
    assert!(first.contains("package-MACRO"), "got: {first}");
}

#[test]
fn hash_bang_not_at_start_is_an_error() {
    let src = "Package \"demo:hi@0.1.0\"\n#!not a shebang\n";
    assert!(wavelet::read_file(src).is_err(), "mid-file #! must not lex");
}

#[test]
fn bare_wlt_argument_runs_the_script() {
    let (_dir, path) = stage(
        "hi.wlt",
        "#!/usr/bin/env wavelet\nPackage \"demo:hi@0.1.0\"\nExport run\n\
         Def run Fn {}\n  add(1 2)\n",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_wavelet"))
        .arg(&path)
        .output()
        .expect("spawn wavelet");
    assert!(
        out.status.success(),
        "bare script run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn bare_extensionless_argument_runs_the_script() {
    let (_dir, path) = stage(
        "deploy",
        "#!/usr/bin/env wavelet\nPackage \"demo:hi@0.1.0\"\nExport run\n\
         Def run Fn {}\n  add(1 2)\n",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_wavelet"))
        .arg(&path)
        .output()
        .expect("spawn wavelet");
    assert!(
        out.status.success(),
        "extensionless script run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn subcommand_wins_over_like_named_file() {
    let dir = std::env::temp_dir().join(format!(
        "wavelet-shebang-sub-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("repl"), "not a real script\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wavelet"))
        .arg("repl")
        .current_dir(&dir)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn wavelet");

    assert!(
        out.status.success(),
        "`wavelet repl` should reach the REPL, not run the `repl` file: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
