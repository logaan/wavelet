mod dependencies;
mod macros;
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
mod project;
mod protocol;
pub mod snippet;
mod unit;

pub use protocol::*;

use macros::Catalog;
use std::collections::{HashMap, HashSet};
use unit::Unit;

pub fn compile(request: &Request, host: &mut dyn MacroHost) -> Result<Step, Vec<Diagnostic>> {
    validate_request(request).map_err(|error| vec![error])?;
    let mut catalog = Catalog::new(host);
    let units = match prepare(request, &mut catalog) {
        Ok(units) => units,
        Err(_) if !catalog.pending.is_empty() => return Ok(Step::NeedsMacros(catalog.pending)),
        Err(error) => return Err(vec![error]),
    };
    compile_units(request, &units)
        .map(Step::Complete)
        .map_err(|error| vec![error])
}

fn prepare(request: &Request, catalog: &mut Catalog<'_>) -> Result<Vec<Unit>, Diagnostic> {
    catalog.index_libraries(&request.sources)?;
    let mut sources = Vec::new();
    for source in &request.sources {
        let (mut arena, mut roots) = read(catalog, source)?;
        if request.mode == Mode::Snippet {
            let wrapped = snippet::wrap(&arena, &roots)
                .map_err(|e| Diagnostic::new(Phase::Input, Some(&source.name), e))?;
            (arena, roots) = read(
                catalog,
                &SourceFile {
                    name: source.name.clone(),
                    contents: wrapped,
                },
            )?;
        }
        let macro_library = crate::macrobuild::is_macro_library(&arena, &roots);
        if !macro_library {
            catalog
                .prepare_local(&arena, &roots)
                .map_err(|e| Diagnostic::new(Phase::Expand, Some(&source.name), e))?;
        }
        sources.push((&source.name, arena, roots, macro_library));
    }
    let mut units = Vec::new();
    for (name, arena, roots, macro_library) in sources {
        let (arena, roots) = if macro_library {
            (arena, roots)
        } else {
            catalog
                .expand(arena, &roots)
                .map_err(|e| Diagnostic::new(Phase::Expand, Some(name), e))?
        };
        units.push(Unit::new(name, arena, roots, macro_library)?);
    }
    Ok(units)
}

fn read(
    catalog: &mut Catalog<'_>,
    source: &SourceFile,
) -> Result<(crate::Arena, Vec<crate::NodeId>), Diagnostic> {
    catalog.read(&source.contents).map_err(|error| Diagnostic {
        phase: Phase::Read,
        source: Some(source.name.clone()),
        span: Some(SourceSpan {
            start: error.at,
            end: error.at,
        }),
        message: error.to_string(),
    })
}

fn compile_units(request: &Request, units: &[Unit]) -> Result<Compilation, Diagnostic> {
    let supplied = dependencies::Dependencies::parse(&request.dependencies)
        .map_err(|e| Diagnostic::new(Phase::Wit, None, e))?;
    let mut index = HashMap::new();
    let mut names = HashMap::new();
    for (position, unit) in units.iter().enumerate() {
        if let Some(previous) = index.insert(unit.info.package_path.clone(), position) {
            return Err(Diagnostic::new(
                Phase::Input,
                Some(&unit.name),
                format!(
                    "duplicate package `{}` in `{}` and `{}`",
                    unit.info.package_path, units[previous].name, unit.name
                ),
            ));
        }
        let name = unit.output_name();
        validate_name(&name).map_err(|e| {
            Diagnostic::new(
                Phase::Input,
                Some(&unit.name),
                format!(
                    "package `{}` produces an unsupported output path: {e}",
                    unit.info.package
                ),
            )
        })?;
        if name.contains('/') {
            return Err(Diagnostic::new(
                Phase::Input,
                Some(&unit.name),
                format!("output filename `{name}` collides or escapes the output directory"),
            ));
        }
        if let Some(previous) = names.insert(name.clone(), unit.name.as_str()) {
            return Err(Diagnostic::new(
                Phase::Input,
                Some(&unit.name),
                format!(
                    "output filename `{name}` collides for `{previous}` and `{}`",
                    unit.name
                ),
            ));
        }
    }
    let mut artifacts = Vec::new();
    for unit in units {
        let dependencies = if unit.macro_library {
            HashMap::new()
        } else {
            unit::resolve_dependencies(unit, units, &index, &supplied)
                .map_err(|e| Diagnostic::new(Phase::Wit, Some(&unit.name), e))?
        };
        artifacts.push(unit.compile(&dependencies)?);
    }
    let entry = project::compose(request.entry.as_deref(), units, &index, &mut artifacts)
        .map_err(|e| Diagnostic::new(Phase::Compose, None, e))?;
    let invocation = (request.mode == Mode::Snippet).then(|| Invocation {
        interface_name: snippet::INTERFACE.into(),
        function_name: snippet::FUNCTION.into(),
    });
    Ok(Compilation {
        artifacts,
        entry,
        invocation,
    })
}

fn validate_request(request: &Request) -> Result<(), Diagnostic> {
    if request.sources.is_empty() {
        return Err(Diagnostic::new(Phase::Input, None, "no input files"));
    }
    if request.mode == Mode::Snippet && request.sources.len() != 1 {
        return Err(Diagnostic::new(
            Phase::Input,
            None,
            "snippet mode requires exactly one source file",
        ));
    }
    for names in [
        request
            .sources
            .iter()
            .map(|file| file.name.as_str())
            .collect::<Vec<_>>(),
        request
            .dependencies
            .iter()
            .map(|file| file.name.as_str())
            .collect(),
    ] {
        let mut seen = HashSet::new();
        for name in names {
            validate_name(name).map_err(|e| Diagnostic::new(Phase::Input, Some(name), e))?;
            if !seen.insert(name) {
                return Err(Diagnostic::new(
                    Phase::Input,
                    Some(name),
                    "duplicate virtual filename",
                ));
            }
        }
    }
    if let Some(entry) = &request.entry
        && !request.sources.iter().any(|source| source.name == *entry)
    {
        return Err(Diagnostic::new(
            Phase::Input,
            Some(entry),
            "entry is not a supplied source file",
        ));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.contains('\\')
        || name.contains('\0')
        || name
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        Err(format!("unsupported virtual path `{name}`"))
    } else {
        Ok(())
    }
}
