use std::path::Path;
use std::process::{Command, Output};
use wavelet::host::{HostComponent, Val};

fn binaryen_available() -> bool {
    if Command::new("wasm-opt").arg("--version").output().is_ok() {
        true
    } else {
        assert!(std::env::var_os("WAVELET_REQUIRE_HOST_TOOLS").is_none());
        false
    }
}

fn source(directory: &Path) -> std::path::PathBuf {
    let path = directory.join("main.wlt");
    std::fs::write(
        &path,
        "Package \"test:build-default\" Export run Def run Fn {} add(20 22)",
    )
    .unwrap();
    path
}

fn build(path: &Path, output: &Path, flags: &[&str], empty_path: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wavelet"));
    command
        .arg("build")
        .arg(path)
        .args(flags)
        .arg("-o")
        .arg(output);
    if empty_path {
        command.env("PATH", "");
    }
    command.output().unwrap()
}

fn artifact(directory: &Path) -> Vec<u8> {
    std::fs::read(directory.join("test-build-default.wasm")).unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn default_and_explicit_size_builds_match_and_debug_retains_metadata() {
    if !binaryen_available() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let source = source(directory.path());
    let default = directory.path().join("default");
    let explicit = directory.path().join("explicit");
    let debug = directory.path().join("debug");
    assert_success(&build(&source, &default, &[], false));
    assert_success(&build(&source, &explicit, &["--optimize-size"], false));
    assert_success(&build(&source, &debug, &["--no-optimize"], false));
    let optimized = artifact(&default);
    let unoptimized = artifact(&debug);
    assert_eq!(optimized, artifact(&explicit));
    assert!(optimized.len() < unoptimized.len());
    assert_eq!(wavelet::metadata::strip(&optimized).unwrap(), optimized);
    assert_ne!(wavelet::metadata::strip(&unoptimized).unwrap(), unoptimized);
    for bytes in [&optimized, &unoptimized] {
        let mut component = HostComponent::from_bytes(bytes).unwrap();
        assert_eq!(
            component
                .call_instance("test:build-default/api", "run", &[])
                .unwrap(),
            vec![Val::S64(42)]
        );
    }
    let api_default = directory.path().join("api-default");
    let api_debug = directory.path().join("api-debug");
    let paths = vec![source.to_str().unwrap().to_owned()];
    wavelet::build::build_files(&paths, api_default.to_str().unwrap()).unwrap();
    wavelet::build::build_files_unoptimized(&paths, api_debug.to_str().unwrap()).unwrap();
    assert_eq!(artifact(&api_default), optimized);
    assert_eq!(artifact(&api_debug), unoptimized);
}

#[test]
fn missing_binaryen_reports_debug_opt_out_and_debug_needs_no_optimizer() {
    let directory = tempfile::tempdir().unwrap();
    let source = source(directory.path());
    let output = directory.path().join("out");
    let failure = build(&source, &output, &[], true);
    assert!(!failure.status.success());
    let message = String::from_utf8_lossy(&failure.stderr);
    assert!(message.contains("wasm-opt"), "{message}");
    assert!(message.contains("--no-optimize"), "{message}");
    assert!(!output.exists());
    assert_success(&build(&source, &output, &["--no-optimize"], true));
    assert!(!artifact(&output).is_empty());
}

#[test]
fn contradictory_optimization_flags_are_rejected_before_building() {
    let directory = tempfile::tempdir().unwrap();
    let source = source(directory.path());
    let output = directory.path().join("out");
    let failure = build(
        &source,
        &output,
        &["--optimize-size", "--no-optimize"],
        true,
    );
    assert!(!failure.status.success());
    assert!(String::from_utf8_lossy(&failure.stderr).contains("cannot be used together"));
    assert!(!output.exists());
}
