//! Shared temp-project staging for the compiled path (0.15).
//!
//! Building a Wavelet program through the real emitter needs an on-disk
//! project shape — sources under `src/`, artifacts under `out/`, the
//! synthesised `wit/` beside them — so the compiled `wavelet run`
//! (`runner::run_files_compiled`), the compiled-first REPL, and the
//! differential test harness each stage their inputs into a private temp
//! project first, keeping the build's side effects out of the user's tree.
//! This module is the one implementation of that dance.
//!
//! A [`StagedProject`] owns a [`tempfile::TempDir`]: the directory name is
//! unpredictable (no pre-delete of a guessable path, so no symlink/TOCTOU
//! footgun) and the tree is removed when the value drops — including on
//! panic — rather than by a cleanup call that error paths can skip. Every
//! failure is a typed [`StageError`] naming the stage that failed, and
//! non-UTF-8 paths surface as `Setup` errors instead of panicking.

use std::path::{Path, PathBuf};

use crate::host::{HostComponent, Val};

/// A failure on the staged compiled path, tagged with the stage that failed.
///
/// The `Display` texts are the stage-prefixed strings the staging sites have
/// always reported (`setup: …`, `build: …`, `read artifact: …`,
/// `instantiate: …`, `call: …`); a guest runtime trap surfaces through
/// [`StageError::Call`] with the host's diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageError {
    /// Creating or populating the staged project failed (fs errors, bad file
    /// names, non-UTF-8 paths).
    Setup(String),
    /// The emitter/toolchain rejected the program.
    Build(String),
    /// A built artifact could not be read back.
    Read(String),
    /// The artifact would not instantiate in the capability-free host.
    Instantiate(String),
    /// Calling the exported entry failed — including a runtime trap in the
    /// guest, or a result of an unexpected shape.
    Call(String),
}

impl std::fmt::Display for StageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StageError::Setup(e) => write!(f, "setup: {e}"),
            StageError::Build(e) => write!(f, "build: {e}"),
            StageError::Read(e) => write!(f, "read artifact: {e}"),
            StageError::Instantiate(e) => write!(f, "instantiate: {e}"),
            StageError::Call(e) => write!(f, "call: {e}"),
        }
    }
}

impl std::error::Error for StageError {}

/// A private temp project holding staged sources, with RAII cleanup.
///
/// Keep the value alive for as long as the staged paths (or anything under
/// [`StagedProject::out_dir`]) are in use; dropping it removes the whole tree.
#[derive(Debug)]
pub struct StagedProject {
    /// The temp project root; dropped last, removing the tree.
    dir: tempfile::TempDir,
    /// Absolute staged source paths (UTF-8, `build_files`-ready), entry first.
    sources: Vec<String>,
}

impl StagedProject {
    /// Stage named sources (`(file-name, contents)` pairs) into a fresh temp
    /// project's `src/`. The first pair is the entry file. `kind` tags the
    /// temp directory name (`wavelet-<kind>-…`) for debuggability.
    pub fn from_sources(kind: &str, files: &[(&str, &str)]) -> Result<Self, StageError> {
        let (dir, src_dir) = fresh_project(kind)?;
        let mut sources = Vec::with_capacity(files.len());
        for (name, contents) in files {
            let dest = src_dir.join(name);
            std::fs::write(&dest, contents).map_err(|e| StageError::Setup(e.to_string()))?;
            sources.push(utf8(&dest)?.to_string());
        }
        Ok(StagedProject { dir, sources })
    }

    /// Copy existing files into a fresh temp project's `src/`, keeping their
    /// file names, so imports resolve against the staged set. The first path
    /// is the entry file.
    pub fn from_files(kind: &str, paths: &[String]) -> Result<Self, StageError> {
        let (dir, src_dir) = fresh_project(kind)?;
        let mut sources = Vec::with_capacity(paths.len());
        for p in paths {
            let name = Path::new(p)
                .file_name()
                .ok_or_else(|| StageError::Setup(format!("{p}: bad file name")))?;
            let dest = src_dir.join(name);
            std::fs::copy(p, &dest).map_err(|e| StageError::Setup(format!("{p}: {e}")))?;
            sources.push(utf8(&dest)?.to_string());
        }
        Ok(StagedProject { dir, sources })
    }

    /// The staged source paths (absolute, entry first), as given to the build.
    pub fn sources(&self) -> &[String] {
        &self.sources
    }

    /// Where built artifacts land, inside the temp project.
    pub fn out_dir(&self) -> PathBuf {
        self.dir.path().join("out")
    }

    /// Build every staged source through the real emitter
    /// (`build::build_files`); returns the built artifact paths.
    pub fn build(&self) -> Result<Vec<String>, StageError> {
        let out_dir = self.out_dir();
        crate::build::build_files(&self.sources, utf8(&out_dir)?).map_err(StageError::Build)
    }

    /// Build, instantiate the first artifact in the capability-free host, and
    /// call `func` in the exported interface `iface`, expecting a single WIT
    /// `string` result — the shape the REPL's `repl-eval` entry and the
    /// differential harness's `differential-main` share.
    pub fn build_and_call_str(&self, iface: &str, func: &str) -> Result<String, StageError> {
        let outputs = self.build()?;
        let artifact = outputs
            .first()
            .ok_or_else(|| StageError::Read("build produced no artifacts".to_string()))?;
        let bytes = std::fs::read(artifact).map_err(|e| StageError::Read(e.to_string()))?;
        let mut component = HostComponent::from_bytes(&bytes).map_err(StageError::Instantiate)?;
        let vals = component
            .call_instance(iface, func, &[])
            .map_err(StageError::Call)?;
        match vals.as_slice() {
            [Val::String(s)] => Ok(s.to_string()),
            other => Err(StageError::Call(format!(
                "unexpected result shape {other:?}"
            ))),
        }
    }
}

/// Create the fresh temp-project layout: a `wavelet-<kind>-…` temp dir (the
/// random suffix comes from `tempfile`) with an empty `src/` inside.
fn fresh_project(kind: &str) -> Result<(tempfile::TempDir, PathBuf), StageError> {
    let dir = tempfile::Builder::new()
        .prefix(&format!("wavelet-{kind}-"))
        .tempdir()
        .map_err(|e| StageError::Setup(e.to_string()))?;
    let src_dir = dir.path().join("src");
    std::fs::create_dir_all(&src_dir).map_err(|e| StageError::Setup(e.to_string()))?;
    Ok((dir, src_dir))
}

/// A staging path as UTF-8, or a `Setup` error naming it — `build_files` and
/// the host take `&str` paths, and a non-UTF-8 temp path must be a reported
/// error, not a panic.
fn utf8(p: &Path) -> Result<&str, StageError> {
    p.to_str()
        .ok_or_else(|| StageError::Setup(format!("{}: path is not valid UTF-8", p.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Display texts are pinned: they are the stage-prefixed wording the
    /// compiled `run`/REPL error surface has always used.
    #[test]
    fn display_keeps_the_stage_prefixed_texts() {
        assert_eq!(StageError::Setup("x".into()).to_string(), "setup: x");
        assert_eq!(StageError::Build("x".into()).to_string(), "build: x");
        assert_eq!(StageError::Read("x".into()).to_string(), "read artifact: x");
        assert_eq!(
            StageError::Instantiate("x".into()).to_string(),
            "instantiate: x"
        );
        assert_eq!(StageError::Call("x".into()).to_string(), "call: x");
    }

    #[test]
    fn staged_sources_land_under_src_and_are_removed_on_drop() {
        let staged =
            StagedProject::from_sources("stage-test", &[("a.wlt", "1"), ("b.wlt", "2")]).unwrap();
        let root = staged.dir.path().to_path_buf();
        assert_eq!(staged.sources().len(), 2);
        for (path, contents) in staged.sources().iter().zip(["1", "2"]) {
            assert!(path.starts_with(root.to_str().unwrap()), "{path}");
            assert!(path.ends_with(".wlt"), "{path}");
            assert_eq!(std::fs::read_to_string(path).unwrap(), contents);
        }
        drop(staged);
        assert!(!root.exists(), "temp project must be removed on drop");
    }

    #[test]
    fn from_files_rejects_a_path_with_no_file_name() {
        let err = StagedProject::from_files("stage-test", &["/".to_string()]).unwrap_err();
        assert_eq!(err.to_string(), "setup: /: bad file name");
    }

    #[test]
    fn from_files_reports_a_missing_input() {
        let err = StagedProject::from_files("stage-test", &["/nonexistent/x.wlt".to_string()])
            .unwrap_err();
        assert!(matches!(err, StageError::Setup(_)));
        assert!(err.to_string().starts_with("setup: /nonexistent/x.wlt: "));
    }
}
