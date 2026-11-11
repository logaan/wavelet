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
