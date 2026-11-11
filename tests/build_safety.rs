use std::path::Path;
use wavelet::staging::Project;

#[test]
fn empty_inputs_return_errors_without_creating_outputs() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("out");
    assert_eq!(
        wavelet::build::build_files(&[], output.to_str().unwrap()).unwrap_err(),
        "no input files"
    );
    assert_eq!(
        wavelet::build::compose_files(&[], output.to_str().unwrap()).unwrap_err(),
        "no input files"
    );
    assert_eq!(
        wavelet::runner::run_files(&[]).unwrap_err(),
        "no input files"
    );
    assert_eq!(
        wavelet::runner::run_files_compiled(&[]).unwrap_err(),
        "no input files"
    );
    assert!(!output.exists());
}

fn assert_rejected_before_writing(first: &str, second: &str, diagnostic: &str) {
    let mut project = Project::new().unwrap();
    project.write(Path::new("first.wlt"), first).unwrap();
    project.write(Path::new("second.wlt"), second).unwrap();
    let error = project.build().unwrap_err().to_string();
    assert!(error.contains(diagnostic), "{error}");
    assert!(error.contains("first.wlt"), "{error}");
    assert!(error.contains("second.wlt"), "{error}");
    assert!(!project.output().exists());
}

#[test]
fn duplicate_package_versions_are_rejected_before_writing() {
    assert_rejected_before_writing(
        "Package \"test:duplicate@1.0.0\" Export run Def run Fn {} 1",
        "Package \"test:duplicate@2.0.0\" Export run Def run Fn {} 2",
        "duplicate package `test:duplicate`",
    );
}

#[test]
fn macro_and_runtime_packages_share_output_identity_checks() {
    assert_rejected_before_writing(
        "Package \"test:duplicate@1.0.0\" DefMacro identity {x} x",
        "Package \"test:duplicate@1.0.0\" Export run Def run Fn {} 2",
        "duplicate package `test:duplicate`",
    );
}

#[test]
fn distinct_packages_with_colliding_output_names_are_rejected() {
    assert_rejected_before_writing(
        "Package \"test-part:name@1.0.0\" Export run Def run Fn {} 1",
        "Package \"test:part-name@1.0.0\" Export run Def run Fn {} 2",
        "output filename `test-part-name.wasm` collides",
    );
}

#[test]
fn compile_errors_do_not_replace_existing_artifacts() {
    let mut project = Project::new().unwrap();
    project
        .write(
            Path::new("first.wlt"),
            "Package \"test:first@1.0.0\" Export run Def run Fn {} 1",
        )
        .unwrap();
    project
        .write(
            Path::new("second.wlt"),
            "Package \"test:second@1.0.0\" Export run Def run Fn {} missing()",
        )
        .unwrap();
    std::fs::create_dir_all(project.output()).unwrap();
    let existing = project.output().join("test-first.wasm");
    std::fs::write(&existing, b"previous build").unwrap();
    assert!(project.build().is_err());
    assert_eq!(std::fs::read(existing).unwrap(), b"previous build");
}

#[test]
fn macro_package_cannot_write_outside_output_directory() {
    let mut project = Project::new().unwrap();
    project
        .write(
            Path::new("macro.wlt"),
            "Package \"../outside\" DefMacro identity {x} x",
        )
        .unwrap();
    let error = project.build().unwrap_err().to_string();
    assert!(error.contains("unsupported output path"), "{error}");
    assert!(!project.path().join("outside.wasm").exists());
    assert!(!project.output().exists());
}
