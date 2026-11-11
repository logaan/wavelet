use std::collections::HashMap;

use super::{Artifact, ArtifactKind, Diagnostic, Phase, dependencies::Dependencies};
use crate::emit::{self, Dep};
use crate::form::{Arena, NodeId};
use crate::wit::{self, FileInfo, ImportInfo};

pub(super) struct Unit {
    pub name: String,
    pub arena: Arena,
    pub roots: Vec<NodeId>,
    pub info: FileInfo,
    pub macro_library: bool,
}

impl Unit {
    pub(super) fn new(
        name: &str,
        arena: Arena,
        roots: Vec<NodeId>,
        macro_library: bool,
    ) -> Result<Self, Diagnostic> {
        let (arena, roots) = if macro_library {
            (arena, roots)
        } else {
            let exported = wit::export_names(&arena, &roots);
            crate::check::resolve_overloads_excluding(arena, &roots, &exported)
                .map_err(|e| Diagnostic::new(Phase::Check, Some(name), e))?
        };
        let info =
            wit::collect(&arena, &roots).map_err(|e| Diagnostic::new(Phase::Wit, Some(name), e))?;
        Ok(Self {
            name: name.into(),
            arena,
            roots,
            info,
            macro_library,
        })
    }

    pub(super) fn dependency(&self) -> Result<Dep, String> {
        let (type_defs, aliases) = emit::dep_non_record_types(&self.arena, &self.info);
        Ok(Dep {
            package: self.info.package.clone(),
            funcs: self.info.exports.clone(),
            package_wit: emit::dep_package_wit(&self.arena, &self.info)?,
            types: emit::dep_record_types(&self.arena, &self.info),
            type_defs,
            aliases,
            type_ifaces: self
                .info
                .types
                .iter()
                .map(|(name, _)| (name.clone(), "api".into(), name.clone()))
                .collect(),
        })
    }

    pub(super) fn compile(
        &self,
        dependencies: &HashMap<String, Dep>,
    ) -> Result<Artifact, Diagnostic> {
        let bytes = if self.macro_library {
            crate::macrobuild::build_macro_component(&self.arena, &self.roots)
        } else {
            let dependencies = emit::scope_dependency_types(&self.info, dependencies);
            let signatures = emit::import_signatures(&self.info, &dependencies);
            crate::check::check_program_with_dependencies(
                &self.arena,
                &self.roots,
                &signatures,
                &dependencies,
                &self.info.imports,
            )
            .map_err(|e| Diagnostic::new(Phase::Check, Some(&self.name), e))?;
            emit::emit_component(&self.arena, &self.roots, &self.info, &dependencies)
        }
        .map_err(|e| Diagnostic::new(Phase::Emit, Some(&self.name), e))?;
        Ok(Artifact {
            name: self.output_name(),
            package_id: self.info.package.clone(),
            kind: if self.macro_library {
                ArtifactKind::MacroComponent
            } else {
                ArtifactKind::Component
            },
            bytes,
        })
    }

    pub(super) fn output_name(&self) -> String {
        format!("{}.wasm", self.info.package_path.replace(':', "-"))
    }
}

pub(super) fn resolve_dependencies(
    unit: &Unit,
    units: &[Unit],
    index: &HashMap<String, usize>,
    supplied: &Dependencies,
) -> Result<HashMap<String, Dep>, String> {
    let mut dependencies = HashMap::new();
    for import in &unit.info.imports {
        if wit::is_macro_only(import) {
            continue;
        }
        let dependency = if let Some(&position) = index.get(&import.package) {
            let dependency = &units[position];
            if import.version.is_some() && requested_package(import) != dependency.info.package {
                return Err(format!(
                    "import `{}` requires `{}`, but resolved `{}`",
                    import.path,
                    requested_package(import),
                    dependency.info.package
                ));
            }
            dependency.dependency()?
        } else {
            supplied
                .resolve(&requested_package(import))?
                .ok_or_else(|| {
                    format!(
                        "import `{}` requires `{}`, which is not satisfied by supplied sources or WIT dependencies",
                        import.path, requested_package(import)
                    )
                })?
        };
        dependencies.insert(import.package.clone(), dependency);
    }
    for interface in unit
        .info
        .exports
        .iter()
        .map(|sig| sig.iface.as_str())
        .chain(
            unit.info
                .resources
                .iter()
                .filter_map(|resource| resource.iface.as_deref()),
        )
    {
        let Some((package, interface)) = interface.split_once('/') else {
            continue;
        };
        if !package.contains(':') || dependencies.contains_key(package) {
            continue;
        }
        let requested = match interface.split_once('@') {
            Some((_, version)) => format!("{package}@{version}"),
            None => package.into(),
        };
        if let Some(dependency) = supplied.resolve(&requested)? {
            dependencies.insert(package.to_owned(), dependency);
        }
    }
    Ok(dependencies)
}

fn requested_package(import: &ImportInfo) -> String {
    match &import.version {
        Some(version) => format!("{}@{version}", import.package),
        None => import.package.clone(),
    }
}
