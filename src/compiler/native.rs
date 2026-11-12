use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::*;
use crate::macros::MacroComponent;

mod optimization;

pub struct NativeMacroHost {
    root: PathBuf,
    libraries: HashMap<String, MacroComponent>,
}

impl NativeMacroHost {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            libraries: HashMap::new(),
        }
    }

    pub fn provide(&mut self, request: MacroRequest) -> Result<(), String> {
        if self.libraries.contains_key(&request.key) {
            return Err("compiler requested an already supplied macro capability".into());
        }
        let library = match request.source {
            MacroSource::Component(bytes) => MacroComponent::from_bytes(&bytes)?,
            MacroSource::Imported(import) => {
                let (package, version) = import
                    .package_id
                    .split_once('@')
                    .map_or((import.package_id.as_str(), None), |(package, version)| {
                        (package, Some(version.to_owned()))
                    });
                let info = crate::wit::ImportInfo {
                    path: package.into(),
                    package: package.into(),
                    version,
                    alias: "macros".into(),
                    macros: true,
                    runtime: false,
                    from: import.source_path,
                };
                let resolver = crate::macrodep::MacroResolver::new(&self.root);
                MacroComponent::from_file(&resolver.locate(&info)?)?
            }
        };
        self.libraries.insert(request.key, library);
        Ok(())
    }
}

impl MacroHost for NativeMacroHost {
    fn manifest(&mut self, key: &str) -> Option<Result<Vec<(String, u32)>, String>> {
        self.libraries.get_mut(key).map(MacroComponent::manifest)
    }

    fn expand(
        &mut self,
        key: &str,
        name: &str,
        args: &crate::meta::Tree,
    ) -> Result<crate::meta::Tree, String> {
        self.libraries
            .get_mut(key)
            .ok_or("macro capability was not supplied")?
            .expand(name, args)
    }
}

pub fn compile(request: &Request, root: &Path) -> Result<Compilation, Vec<Diagnostic>> {
    compile_with_optimizer(request, root, &mut Ok)
}

pub fn compile_for_size(request: &Request, root: &Path) -> Result<Compilation, Vec<Diagnostic>> {
    compile_with_optimizer(request, root, &mut optimization::optimize)
}

fn compile_with_optimizer(
    request: &Request,
    root: &Path,
    optimizer: &mut dyn crate::emit::CoreOptimizer,
) -> Result<Compilation, Vec<Diagnostic>> {
    let mut host = NativeMacroHost::new(root);
    loop {
        match super::compile_with_optimizer(request, &mut host, optimizer)? {
            Step::Complete(compilation) => return Ok(compilation),
            Step::NeedsMacros(requests) => {
                for request in requests {
                    host.provide(request)
                        .map_err(|error| vec![Diagnostic::new(Phase::Expand, None, error)])?;
                }
            }
        }
    }
}

pub fn request_from_paths(paths: &[String], root: &Path) -> Result<Request, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.display()))?;
    let mut sources = Vec::new();
    for path in paths {
        let absolute = Path::new(path)
            .canonicalize()
            .map_err(|e| format!("{path}: {e}"))?;
        let relative = absolute.strip_prefix(&root).unwrap_or(&absolute);
        let name = if relative.is_absolute() {
            relative
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("source filename is not UTF-8")?
                .to_owned()
        } else {
            relative
                .to_str()
                .ok_or("source filename is not UTF-8")?
                .replace('\\', "/")
        };
        let contents = std::fs::read_to_string(&absolute).map_err(|e| format!("{path}: {e}"))?;
        sources.push(SourceFile { name, contents });
    }
    let directory = root.join("wit/deps");
    let mut dependencies = Vec::new();
    if directory.is_dir() {
        read_dependencies(&directory, &directory, &mut dependencies)?;
    }
    Ok(Request {
        sources,
        dependencies,
        entry: None,
        mode: Mode::Project,
    })
}

fn read_dependencies(
    root: &Path,
    directory: &Path,
    output: &mut Vec<DependencyFile>,
) -> Result<(), String> {
    let entries =
        std::fs::read_dir(directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            return Err(format!(
                "unsupported dependency symlink: {}",
                path.display()
            ));
        }
        if kind.is_dir() {
            read_dependencies(root, &path, output)?;
        } else if matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("wit" | "wasm" | "wat")
        ) {
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .ok_or("dependency filename is not UTF-8")?
                .replace('\\', "/");
            let contents = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            output.push(DependencyFile { name, contents });
        }
    }
    Ok(())
}

pub fn display_diagnostics(diagnostics: Vec<Diagnostic>) -> String {
    diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}
