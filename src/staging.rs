use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum StageError {
    Io(std::io::Error),
    InvalidPath(PathBuf),
    DuplicateName(PathBuf),
    Build(String),
}

impl fmt::Display for StageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "staging: {e}"),
            Self::InvalidPath(p) => write!(f, "unsupported path: {}", p.display()),
            Self::DuplicateName(p) => write!(f, "duplicate staged file name: {}", p.display()),
            Self::Build(e) => write!(f, "build: {e}"),
        }
    }
}

impl std::error::Error for StageError {}

impl From<std::io::Error> for StageError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

pub struct Project {
    dir: tempfile::TempDir,
    paths: Vec<String>,
}

impl Project {
    pub fn new() -> Result<Self, StageError> {
        let dir = tempfile::Builder::new().prefix("wavelet-").tempdir()?;
        std::fs::create_dir(dir.path().join("src"))?;
        Ok(Self {
            dir,
            paths: Vec::new(),
        })
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn output(&self) -> PathBuf {
        self.path().join("out")
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    pub fn write(&mut self, name: &Path, source: &str) -> Result<(), StageError> {
        if name.components().count() != 1 || name.file_name().is_none() {
            return Err(StageError::InvalidPath(name.to_path_buf()));
        }
        let path = self.path().join("src").join(name);
        let text = utf8_path(&path)?.to_owned();
        if self.paths.contains(&text) {
            return Err(StageError::DuplicateName(path));
        }
        std::fs::write(&path, source)?;
        self.paths.push(text);
        Ok(())
    }

    pub fn copy(&mut self, path: &Path) -> Result<(), StageError> {
        let name = path
            .file_name()
            .ok_or_else(|| StageError::InvalidPath(path.to_path_buf()))?;
        let source = std::fs::read_to_string(path)?;
        self.write(Path::new(name), &source)
    }

    pub fn build(&self) -> Result<Vec<String>, StageError> {
        crate::build::build_files(&self.paths, utf8_path(&self.output())?)
            .map_err(StageError::Build)
    }

    pub fn compile(source: &str) -> Result<Vec<u8>, StageError> {
        let mut project = Self::new()?;
        project.write(Path::new("entry.wlt"), source)?;
        let outputs = project.build()?;
        let first = outputs
            .first()
            .ok_or_else(|| StageError::Build("no component emitted".into()))?;
        Ok(std::fs::read(first)?)
    }
}

pub fn utf8_path(path: &Path) -> Result<&str, StageError> {
    path.to_str()
        .ok_or_else(|| StageError::InvalidPath(path.to_path_buf()))
}
