use std::path::Path;
use wavelet::staging::{Project, StageError};

#[test]
fn staged_project_is_private_and_removed_on_drop() {
    let mut project = Project::new().unwrap();
    let root = project.path().to_owned();
    project.write(Path::new("entry.wlt"), "Def x 1").unwrap();
    assert!(root.join("src/entry.wlt").exists());
    assert!(matches!(
        project.write(Path::new("entry.wlt"), "Def x 2"),
        Err(StageError::DuplicateName(_))
    ));
    assert!(matches!(
        project.write(Path::new("../outside.wlt"), ""),
        Err(StageError::InvalidPath(_))
    ));
    drop(project);
    assert!(!root.exists());
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_report_errors() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    assert!(matches!(
        wavelet::staging::utf8_path(Path::new(OsStr::from_bytes(b"bad\xff"))),
        Err(StageError::InvalidPath(_))
    ));
}

#[test]
fn dependencies_and_lock_are_copied_without_modifying_the_original() {
    let source = tempfile::tempdir().unwrap();
    let deps = source.path().join("wit/deps/example");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(deps.join("world.wit"), "package test:example; world app {}").unwrap();
    std::fs::write(source.path().join("wkg.lock"), "locked").unwrap();
    let project = Project::new().unwrap();
    project.copy_dependencies(source.path()).unwrap();
    assert!(project.path().join("wit/deps/example/world.wit").is_file());
    assert_eq!(
        std::fs::read_to_string(project.path().join("wkg.lock")).unwrap(),
        "locked"
    );
    assert!(deps.join("world.wit").is_file());
}
