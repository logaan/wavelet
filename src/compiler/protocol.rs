#[derive(Clone, Debug)]
pub struct SourceFile {
    pub name: String,
    pub contents: String,
}

#[derive(Clone, Debug)]
pub struct DependencyFile {
    pub name: String,
    pub contents: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Project,
    Snippet,
    Repl,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub sources: Vec<SourceFile>,
    pub dependencies: Vec<DependencyFile>,
    pub entry: Option<String>,
    pub mode: Mode,
}

impl Request {
    pub fn snippet(source: impl Into<String>) -> Self {
        Self {
            sources: vec![SourceFile {
                name: "snippet.wlt".into(),
                contents: source.into(),
            }],
            dependencies: Vec::new(),
            entry: None,
            mode: Mode::Snippet,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Input,
    Read,
    Expand,
    Check,
    Wit,
    Emit,
    Compose,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSpan {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub phase: Phase,
    pub source: Option<String>,
    pub span: Option<SourceSpan>,
    pub message: String,
}

impl Diagnostic {
    pub(crate) fn new(phase: Phase, source: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            phase,
            source: source.map(str::to_owned),
            span: None,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(source) = &self.source {
            write!(formatter, "{source}: ")?;
        }
        write!(formatter, "{}", self.message)
    }
}

#[derive(Clone, Debug)]
pub struct MacroImport {
    pub package_id: String,
    pub source_path: Option<String>,
}

#[derive(Clone, Debug)]
pub enum MacroSource {
    Component(Vec<u8>),
    Imported(MacroImport),
}

#[derive(Clone, Debug)]
pub struct MacroRequest {
    pub key: String,
    pub source: MacroSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactKind {
    Component,
    MacroComponent,
    Application,
}

#[derive(Clone, Debug)]
pub struct Artifact {
    pub name: String,
    pub package_id: String,
    pub kind: ArtifactKind,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Invocation {
    pub interface_name: String,
    pub function_name: String,
}

#[derive(Clone, Debug)]
pub struct Compilation {
    pub artifacts: Vec<Artifact>,
    pub entry: Option<String>,
    pub invocation: Option<Invocation>,
}

impl Compilation {
    pub fn entry_artifact(&self) -> Option<&Artifact> {
        let name = self.entry.as_deref()?;
        self.artifacts.iter().find(|artifact| artifact.name == name)
    }
}

#[derive(Clone, Debug)]
pub enum Step {
    NeedsMacros(Vec<MacroRequest>),
    Complete(Compilation),
}

pub trait MacroHost {
    fn manifest(&mut self, key: &str) -> Option<Result<Vec<(String, u32)>, String>>;
    fn expand(
        &mut self,
        key: &str,
        name: &str,
        args: &crate::meta::Tree,
    ) -> Result<crate::meta::Tree, String>;
}

pub struct NoMacros;

impl MacroHost for NoMacros {
    fn manifest(&mut self, _: &str) -> Option<Result<Vec<(String, u32)>, String>> {
        None
    }
    fn expand(
        &mut self,
        _: &str,
        _: &str,
        _: &crate::meta::Tree,
    ) -> Result<crate::meta::Tree, String> {
        Err("macro capability was not supplied".into())
    }
}
