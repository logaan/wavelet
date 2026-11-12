use std::path::PathBuf;
use std::process::{Command, Output};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wavelet-checker-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn write_src(dir: &std::path::Path, name: &str, src: &str) -> PathBuf {
    let file = dir.join(name);
    std::fs::write(&file, src).expect("write source file");
    file
}

fn run_wavelet(subcommand: &str, file: &PathBuf) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wavelet"))
        .arg(subcommand)
        .arg(file)
        .output()
        .unwrap_or_else(|e| panic!("run `wavelet {subcommand}`: {e}"))
}

#[test]
fn build_rejects_an_ill_typed_program() {
    let dir = scratch("build-bad");

    let file = write_src(
        &dir,
        "bad.wlt",
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
        "good.wlt",
        r#"Package "demo:good@0.1.0"
Export shout
Def shout Fn {phrase: string}
  str-cat(phrase "!")
"#,
    );

    let out = run_wavelet("build", &dir.join("good.wlt"));
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

#[test]
fn wit_rejects_an_ill_typed_program() {
    let dir = scratch("wit-bad");
    let file = write_src(
        &dir,
        "bad.wlt",
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
        "good.wlt",
        r#"Package "demo:good@0.1.0"
Export shout
Def shout Fn {phrase: string}
  str-cat(phrase "!")
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
    assert!(
        stdout.contains("shout: func(phrase: string) -> string"),
        "{stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn run_rejects_an_ill_typed_program() {
    let dir = scratch("run-bad");

    let file = write_src(
        &dir,
        "bad.wlt",
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

    let file = write_src(
        &dir,
        "overload.wlt",
        r#"Package "demo:over@0.1.0"
Def show Fn {x: s32} x
Def show Fn {x: string} str-cat(x)
Export run
Def run Fn {} show(5)
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

#[test]
fn build_rejects_a_cross_component_type_error() {
    let dir = scratch("build-xcomp-bad");
    let src = dir.join("src");
    std::fs::create_dir_all(&src).expect("mkdir src");
    let provider = write_src(
        &src,
        "shout.wlt",
        r#"Package "demo:shout@0.1.0"
Export shout
Def shout Fn {phrase: string} str-cat(phrase)
"#,
    );

    let consumer = write_src(
        &src,
        "main.wlt",
        r#"Package "demo:main@0.1.0"
Import {pkg: "demo:shout/api" as: sh}
Export {name: run result: string}
Def run Fn {} sh/shout(42)
"#,
    );

    let out = Command::new(env!("CARGO_BIN_EXE_wavelet"))
        .arg("build")
        .arg(&consumer)
        .arg(&provider)
        .output()
        .expect("run wavelet build");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "`wavelet build` accepted a cross-component type error\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("phrase") || stderr.contains("wrong type"),
        "error should point at the mistyped argument; got:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn build_accepts_a_well_typed_cross_component_call() {
    let dir = scratch("build-xcomp-ok");
    let src = dir.join("src");
    std::fs::create_dir_all(&src).expect("mkdir src");
    let provider = write_src(
        &src,
        "shout.wlt",
        r#"Package "demo:shout@0.1.0"
Export shout
Def shout Fn {phrase: string} str-cat(phrase)
"#,
    );
    let consumer = write_src(
        &src,
        "main.wlt",
        r#"Package "demo:main@0.1.0"
Import {pkg: "demo:shout/api" as: sh}
Export {name: run result: string}
Def run Fn {} sh/shout("hi")
"#,
    );

    let out = Command::new(env!("CARGO_BIN_EXE_wavelet"))
        .arg("build")
        .arg(&consumer)
        .arg(&provider)
        .output()
        .expect("run wavelet build");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "well-typed cross-component call rejected:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
