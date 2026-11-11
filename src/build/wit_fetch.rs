use std::path::Path;

pub(super) fn fetch(
    root: &Path,
    worlds: &[(String, String)],
    mut fetch_world: impl FnMut(&Path) -> Result<(), String>,
) -> Result<(), String> {
    if worlds.is_empty() {
        return Ok(());
    }
    let workspace = crate::staging::Project::new().map_err(|e| e.to_string())?;
    workspace
        .copy_dependencies(root)
        .map_err(|e| e.to_string())?;
    let directory = workspace.path().join("wit");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    for (name, world) in worlds {
        clear_generated_roots(&directory)?;
        let path = directory.join(name);
        std::fs::write(&path, world).map_err(|e| format!("{}: {e}", path.display()))?;
        fetch_world(&directory)?;
    }
    let dependencies = directory.join("deps");
    if dependencies.exists() {
        crate::staging::copy_tree(&dependencies, &root.join("wit/deps"))
            .map_err(|e| e.to_string())?;
    }
    let lock = workspace.path().join("wkg.lock");
    if lock.is_file() {
        std::fs::copy(&lock, root.join("wkg.lock"))
            .map_err(|e| format!("{}: {e}", lock.display()))?;
    }
    Ok(())
}

fn clear_generated_roots(directory: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("wit") {
            std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetched_dependencies_preserve_authored_worlds_and_macros() {
        let root = tempfile::tempdir().unwrap();
        let wit = root.path().join("wit");
        std::fs::create_dir_all(wit.join("macros")).unwrap();
        std::fs::write(wit.join("authored.wit"), "authored world").unwrap();
        std::fs::write(wit.join("macros/library.wasm"), b"macro").unwrap();
        std::fs::write(root.path().join("wkg.lock"), "old lock").unwrap();
        let worlds = vec![
            ("first.wit".into(), "first".into()),
            ("second.wit".into(), "second".into()),
        ];
        let mut calls = 0;
        fetch(root.path(), &worlds, |directory| {
            calls += 1;
            assert!(!directory.join("authored.wit").exists());
            assert_eq!(
                std::fs::read(wit.join("authored.wit")).unwrap(),
                b"authored world"
            );
            if calls == 2 {
                assert!(!directory.join("first.wit").exists());
            }
            std::fs::create_dir_all(directory.join("deps")).unwrap();
            std::fs::write(
                directory.join(format!("deps/package-{calls}.wit")),
                "dependency",
            )
            .unwrap();
            std::fs::write(directory.parent().unwrap().join("wkg.lock"), "new lock").unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(
            std::fs::read(wit.join("authored.wit")).unwrap(),
            b"authored world"
        );
        assert_eq!(
            std::fs::read(wit.join("macros/library.wasm")).unwrap(),
            b"macro"
        );
        assert!(wit.join("deps/package-1.wit").exists());
        assert!(wit.join("deps/package-2.wit").exists());
        assert!(!wit.join("first.wit").exists());
        assert!(!wit.join("second.wit").exists());
        assert_eq!(
            std::fs::read(root.path().join("wkg.lock")).unwrap(),
            b"new lock"
        );
    }

    #[test]
    fn failed_fetch_does_not_publish_partial_dependencies_or_lock() {
        let root = tempfile::tempdir().unwrap();
        let wit = root.path().join("wit");
        std::fs::create_dir_all(wit.join("deps")).unwrap();
        std::fs::write(wit.join("authored.wit"), "authored world").unwrap();
        std::fs::write(wit.join("deps/existing.wit"), "existing").unwrap();
        std::fs::write(root.path().join("wkg.lock"), "old lock").unwrap();
        let result = fetch(
            root.path(),
            &[("generated.wit".into(), "generated".into())],
            |directory| {
                std::fs::write(directory.join("deps/partial.wit"), "partial").unwrap();
                std::fs::write(directory.parent().unwrap().join("wkg.lock"), "partial lock")
                    .unwrap();
                Err("registry unavailable".into())
            },
        );
        assert_eq!(result.unwrap_err(), "registry unavailable");
        assert_eq!(
            std::fs::read(wit.join("authored.wit")).unwrap(),
            b"authored world"
        );
        assert_eq!(
            std::fs::read(wit.join("deps/existing.wit")).unwrap(),
            b"existing"
        );
        assert!(!wit.join("deps/partial.wit").exists());
        assert!(!wit.join("generated.wit").exists());
        assert_eq!(
            std::fs::read(root.path().join("wkg.lock")).unwrap(),
            b"old lock"
        );
    }
}
