use std::path::{Path, PathBuf};

use crate::form::{Arena, NodeId};
use crate::tools;
use crate::wit::{self, FileInfo};

mod wit_fetch;

struct PreparedUnit {
    path: String,
    arena: Arena,
    roots: Vec<NodeId>,
    info: FileInfo,
}

impl PreparedUnit {
    fn load(path: &str, root: &Path) -> Result<(Self, bool), String> {
        let source = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let (arena, roots) = crate::macrodep::read_file_with_macros(&source, root)
            .map_err(|e| format!("{path}: {e}"))?;
        let is_macro = crate::macrobuild::is_macro_library(&arena, &roots);
        let (arena, roots) = if is_macro {
            (arena, roots)
        } else {
            let mut foreign = crate::macrodep::FileExpander::for_file(root, &arena, &roots);
            let (arena, roots) = crate::expand::expand_file(
                arena,
                &roots,
                foreign
                    .as_mut()
                    .map(|f| f as &mut dyn crate::expand::ForeignExpander),
            )
            .map_err(|e| format!("{path}: {e}"))?;
            let exported = wit::export_names(&arena, &roots);
            crate::check::resolve_overloads_excluding(arena, &roots, &exported)
                .map_err(|e| format!("{path}: {e}"))?
        };
        let info = wit::collect(&arena, &roots).map_err(|e| format!("{path}: {e}"))?;
        Ok((
            Self {
                path: path.to_owned(),
                arena,
                roots,
                info,
            },
            is_macro,
        ))
    }
}

pub fn build_files(paths: &[String], out_dir: &str) -> Result<Vec<String>, String> {
    build_files_with_size_optimization(paths, out_dir, false)
}

pub fn build_files_for_size(paths: &[String], out_dir: &str) -> Result<Vec<String>, String> {
    build_files_with_size_optimization(paths, out_dir, true)
}

fn build_files_with_size_optimization(
    paths: &[String],
    out_dir: &str,
    optimize_size: bool,
) -> Result<Vec<String>, String> {
    if paths.is_empty() {
        return Err("no input files".into());
    }
    let root = project_root(paths).unwrap_or_else(|| PathBuf::from("."));
    let request = crate::compiler::native::request_from_paths(paths, &root)?;
    let compile = if optimize_size {
        crate::compiler::native::compile_for_size
    } else {
        crate::compiler::native::compile
    };
    let compilation =
        compile(&request, &root).map_err(crate::compiler::native::display_diagnostics)?;
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{out_dir}: {e}"))?;
    let mut outputs = Vec::new();
    for artifact in compilation.artifacts {
        let output = Path::new(out_dir).join(artifact.name);
        std::fs::write(&output, artifact.bytes)
            .map_err(|e| format!("{}: {e}", output.display()))?;
        outputs.push(
            crate::staging::utf8_path(&output)
                .map_err(|e| e.to_string())?
                .to_owned(),
        );
    }
    let sources = paths.iter().map(PathBuf::from).collect::<Vec<_>>();
    if let Err(error) = populate_project_wit(&root, &sources) {
        eprintln!("warning: could not populate wit/ via wkg: {error}");
    }
    Ok(outputs)
}

pub fn populate_project_wit(root: &Path, src_paths: &[PathBuf]) -> Result<(), String> {
    let units = src_paths
        .iter()
        .map(|path| {
            PreparedUnit::load(
                crate::staging::utf8_path(path).map_err(|e| e.to_string())?,
                root,
            )
            .map(|(unit, _)| unit)
        })
        .collect::<Result<Vec<_>, _>>()?;
    populate_wit(root, &units)
}

fn populate_wit(root: &Path, units: &[PreparedUnit]) -> Result<(), String> {
    let local_packages: Vec<_> = units.iter().map(|u| u.info.package_path.clone()).collect();
    let worlds = units
        .iter()
        .filter(|unit| wit::has_external_deps(&unit.info, &local_packages))
        .map(|unit| {
            wit::synthesize_fetch_world_excluding(&unit.arena, &unit.roots, &local_packages)
                .map(|world| (format!("{}.wit", unit.info.world), world))
                .map_err(|e| format!("{}: {e}", unit.path))
        })
        .collect::<Result<Vec<_>, _>>()?;
    wit_fetch::fetch(root, &worlds, |directory| {
        tools::wkg_wit_fetch_from(directory, root).map(|_| ())
    })
}

fn project_root(paths: &[String]) -> Option<PathBuf> {
    let first = paths.first()?;
    let src_dir = Path::new(first).parent()?;
    let root = src_dir.parent().unwrap_or(src_dir);

    Some(if root.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        root.to_path_buf()
    })
}

pub fn compose_files(paths: &[String], out: &str) -> Result<(), String> {
    use wac_graph::{CompositionGraph, EncodeOptions, types::Package};

    if paths.is_empty() {
        return Err("no input files".into());
    }
    let mut graph = CompositionGraph::new();
    let mut ids = Vec::new();
    for path in paths {
        let stem = Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or(format!("{path}: bad file name"))?;
        let name = if stem.contains(':') {
            stem.to_string()
        } else {
            match stem.split_once('-') {
                Some((ns, n)) => format!("{ns}:{n}"),
                None => format!("wavelet:{stem}"),
            }
        };
        let pkg = Package::from_file(&name, None, path, graph.types_mut())
            .map_err(|e| format!("{path}: {e:#}"))?;
        let id = graph
            .register_package(pkg)
            .map_err(|e| format!("{path}: {e:#}"))?;
        ids.push(id);
    }

    let socket = ids[0];
    let plugs = ids[1..].to_vec();
    wac_graph::plug(&mut graph, plugs, socket).map_err(|e| format!("compose: {e:#}"))?;

    let bytes = graph
        .encode(EncodeOptions::default())
        .map_err(|e| format!("compose: {e:#}"))?;
    std::fs::write(out, bytes).map_err(|e| format!("{out}: {e}"))?;
    Ok(())
}
