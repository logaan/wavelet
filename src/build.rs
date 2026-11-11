use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::emit::{self, Dep};
use crate::form::{Arena, NodeId};
use crate::tools;
use crate::wit::{self, FileInfo, ImportInfo};

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

    fn dependency(&self) -> Result<Dep, String> {
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
                .map(|(name, _)| (name.clone(), "api".to_owned(), name.clone()))
                .collect(),
        })
    }

    fn compile(&self, dependencies: &HashMap<String, Dep>) -> Result<Vec<u8>, String> {
        let dependencies = emit::scope_dependency_types(&self.info, dependencies);
        let mut signatures = crate::check::ImportSigs::new();
        for import in &self.info.imports {
            if wit::is_macro_only(import) {
                continue;
            }
            if let Some(dependency) = dependencies.get(&import.package) {
                let interface = import.path.split_once('/').map(|(_, i)| i).unwrap_or("api");
                signatures.extend(wit::import_sigs_for(
                    &import.alias,
                    &dependency.funcs,
                    interface,
                ));
            }
        }
        signatures.extend(wit::functor_import_sigs(&self.info.functors));
        crate::check::check_program_with_dependencies(
            &self.arena,
            &self.roots,
            &signatures,
            &dependencies,
            &self.info.imports,
        )
        .map_err(|e| format!("{}: {e}", self.path))?;
        emit::emit_component(&self.arena, &self.roots, &self.info, &dependencies)
            .map_err(|e| format!("{}: {e}", self.path))
    }

    fn output_name(&self) -> String {
        format!("{}.wasm", self.info.package_path.replace(':', "-"))
    }
}

pub fn build_files(paths: &[String], out_dir: &str) -> Result<Vec<String>, String> {
    if paths.is_empty() {
        return Err("no input files".into());
    }
    let root = project_root(paths).unwrap_or_else(|| PathBuf::from("."));
    let mut units = Vec::new();
    let mut macro_units = Vec::new();
    let mut packages = HashMap::new();
    let mut artifacts = HashMap::new();
    for path in paths {
        let (unit, is_macro) = PreparedUnit::load(path, &root)?;
        let name = unit.output_name();
        if Path::new(&name).components().count() != 1 {
            return Err(format!(
                "{path}: package `{}` produces an unsupported output path",
                unit.info.package
            ));
        }

        if let Some(previous) = packages.insert(unit.info.package_path.clone(), path) {
            return Err(format!(
                "duplicate package `{}` in `{previous}` and `{path}`",
                unit.info.package_path
            ));
        }
        if let Some(previous) = artifacts.insert(unit.output_name(), path) {
            return Err(format!(
                "output filename `{}` collides for `{previous}` and `{path}`",
                unit.output_name()
            ));
        }
        if is_macro {
            macro_units.push(unit);
        } else {
            units.push(unit);
        }
    }
    let index: HashMap<String, usize> = units
        .iter()
        .enumerate()
        .map(|(i, unit)| (unit.info.package_path.clone(), i))
        .collect();
    let deps_dir = root.join("wit/deps");
    let mut artifacts = Vec::new();
    for unit in &macro_units {
        let bytes = crate::macrobuild::build_macro_component(&unit.arena, &unit.roots)
            .map_err(|e| format!("{}: {e}", unit.path))?;
        artifacts.push((unit.output_name(), bytes));
    }
    for unit in &units {
        let dependencies = resolve_dependencies(unit, &units, &index, &deps_dir)?;
        artifacts.push((unit.output_name(), unit.compile(&dependencies)?));
    }
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{out_dir}: {e}"))?;
    let mut outputs = Vec::new();
    for (name, bytes) in artifacts {
        let out = format!("{out_dir}/{name}");
        std::fs::write(&out, bytes).map_err(|e| format!("{out}: {e}"))?;
        outputs.push(out);
    }
    if let Err(e) = populate_wit(&root, &units) {
        eprintln!("warning: could not populate wit/ via wkg: {e}");
    }
    match compose_units(&units, &index, out_dir) {
        Ok(Some(app)) => outputs.push(app),
        Ok(None) => {}
        Err(e) => eprintln!("warning: could not compose components via wac: {e}"),
    }
    Ok(outputs)
}

fn resolve_dependencies(
    unit: &PreparedUnit,
    units: &[PreparedUnit],
    index: &HashMap<String, usize>,
    directory: &Path,
) -> Result<HashMap<String, Dep>, String> {
    let mut dependencies = HashMap::new();
    for import in &unit.info.imports {
        if wit::is_macro_only(import) {
            continue;
        }
        let dependency = if let Some(&position) = index.get(&import.package) {
            let dependency = &units[position];
            validate_import_version(import, &dependency.info.package)?;
            dependency.dependency()?
        } else {
            crate::witdep::resolve_dep(directory, &requested_package(import))?.ok_or_else(|| {
                format!(
                    "{}: import `{}` is not satisfied by any file in the build set or `wit/deps`",
                    unit.path, import.path
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
            None => package.to_owned(),
        };
        if let Some(dependency) = crate::witdep::resolve_dep(directory, &requested)? {
            dependencies.insert(package.to_owned(), dependency);
        }
    }
    Ok(dependencies)
}

fn requested_package(imp: &ImportInfo) -> String {
    match &imp.version {
        Some(version) => format!("{}@{version}", imp.package),
        None => imp.package.clone(),
    }
}

fn validate_import_version(imp: &ImportInfo, resolved: &str) -> Result<(), String> {
    if imp.version.is_some() && requested_package(imp) != resolved {
        return Err(format!(
            "import `{}` requires `{}`, but resolved `{resolved}`",
            imp.path,
            requested_package(imp)
        ));
    }
    Ok(())
}

fn compose_units(
    units: &[PreparedUnit],
    index: &HashMap<String, usize>,
    out_dir: &str,
) -> Result<Option<String>, String> {
    let mut edges: Vec<(usize, &ImportInfo, usize)> = Vec::new();
    for (ui, u) in units.iter().enumerate() {
        for imp in &u.info.imports {
            if wit::is_macro_only(imp) {
                continue;
            }
            if let Some(&plug) = index.get(&imp.package)
                && plug != ui
            {
                edges.push((ui, imp, plug));
            }
        }
    }
    if edges.is_empty() {
        return Ok(None);
    }

    let imported: std::collections::HashSet<usize> =
        edges.iter().map(|&(_, _, plug)| plug).collect();
    let importers: std::collections::HashSet<usize> = edges.iter().map(|&(ui, _, _)| ui).collect();
    let root = (0..units.len())
        .find(|i| !imported.contains(i) && importers.contains(i))
        .or_else(|| (0..units.len()).find(|i| !imported.contains(i)))
        .ok_or("no root component (cyclic composition?)")?;

    let var = |i: usize| units[i].info.package_path.replace([':', '/'], "-");

    let order = topo_order(units.len(), &edges)?;
    let mut wac =
        String::from("// Generated by `wavelet build`. Wires the project's components into one\n");
    wac.push_str("package wavelet:app;\n\n");
    for &i in &order {
        let wirings: Vec<String> = edges
            .iter()
            .filter(|&&(ui, _, _)| ui == i)
            .map(|&(_, imp, plug)| {
                let iface = versioned_iface(&imp.path, &units[plug].info.package);
                format!(
                    "  \"{iface}\": {plug}[\"{iface}\"],",
                    plug = var(plug),
                    iface = iface
                )
            })
            .collect();
        if wirings.is_empty() {
            wac.push_str(&format!(
                "let {} = new {} {{ }};\n",
                var(i),
                units[i].info.package_path
            ));
        } else {
            wac.push_str(&format!(
                "let {} = new {} {{\n",
                var(i),
                units[i].info.package_path
            ));
            for w in &wirings {
                wac.push_str(w);
                wac.push('\n');
            }
            wac.push_str("  ...\n};\n");
        }
    }
    wac.push_str(&format!("\nexport {}...;\n", var(root)));

    let wac_path = format!("{out_dir}/app.wac");
    std::fs::write(&wac_path, &wac).map_err(|e| format!("{wac_path}: {e}"))?;

    let deps: Vec<(String, std::path::PathBuf)> = units
        .iter()
        .map(|u| {
            (
                u.info.package_path.clone(),
                std::path::PathBuf::from(format!(
                    "{out_dir}/{}.wasm",
                    u.info.package_path.replace(':', "-")
                )),
            )
        })
        .collect();
    let dep_refs: Vec<(String, &Path)> =
        deps.iter().map(|(n, p)| (n.clone(), p.as_path())).collect();

    let app = format!("{out_dir}/app.wasm");
    tools::wac_compose(Path::new(&wac_path), None, &dep_refs, Path::new(&app))?;
    Ok(Some(app))
}

fn topo_order(n: usize, edges: &[(usize, &ImportInfo, usize)]) -> Result<Vec<usize>, String> {
    let mut indeg = vec![0usize; n];
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(importer, _, plug) in edges {
        succ[plug].push(importer);
        indeg[importer] += 1;
    }
    let mut queue: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut order = Vec::new();
    while let Some(i) = queue.pop() {
        order.push(i);
        for &j in &succ[i] {
            indeg[j] -= 1;
            if indeg[j] == 0 {
                queue.push(j);
            }
        }
    }
    if order.len() != n {
        return Err("cyclic component composition".into());
    }
    Ok(order)
}

fn versioned_iface(iface_path: &str, package_versioned: &str) -> String {
    match package_versioned.split_once('@') {
        Some((_, ver)) => format!("{iface_path}@{ver}"),
        None => iface_path.to_string(),
    }
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
        tools::wkg_wit_fetch(directory).map(|_| ())
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
