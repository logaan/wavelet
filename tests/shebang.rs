//! Shebang support (7.2): a `.wlt` file may begin with `#!/usr/bin/env wavelet`
//! so it can be marked executable and run directly. Two guarantees are pinned:
//! the lexer skips a leading shebang line (only at byte 0), and a bare
//! `wavelet <file.wlt>` invocation runs the script through the normal run path.

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

/// The reader skips a leading `#!` line so a shebang'd script still parses to
/// exactly the forms that follow it.
#[test]
fn reader_skips_leading_shebang() {
    let src = "#!/usr/bin/env wavelet\nPackage \"demo:hi@0.1.0\"\nExport run\n\
               Def run Fn {}\n  add(1 2)\n";
    let (arena, roots) = wavelet::read_file(src).expect("shebang'd source reads");
    // First form is the Package declaration, not a stray comment/error.
    let first = wavelet::print(&arena, roots[0]);
    assert!(first.contains("package-MACRO"), "got: {first}");
}

/// `#!` is a shebang only at byte 0; a `#` elsewhere is still a read error.
#[test]
fn hash_bang_not_at_start_is_an_error() {
    let src = "Package \"demo:hi@0.1.0\"\n#!not a shebang\n";
    assert!(wavelet::read_file(src).is_err(), "mid-file #! must not lex");
}

/// A bare `wavelet <file.wlt>` runs the script (the shape a shebang produces).
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
