use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::emit::{self, Dep};
use crate::form::{Arena, NodeId};
use crate::tools;
use crate::wit::{self, FileInfo, ImportInfo};

struct Unit {
    path: String,
    arena: Arena,
    roots: Vec<NodeId>,
    info: FileInfo,
}

pub fn build_files(paths: &[String], out_dir: &str) -> Result<Vec<String>, String> {
    let root = project_root(paths).unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{out_dir}: {e}"))?;
    let mut units = Vec::new();
    let mut macro_outputs = Vec::new();
    for path in paths {
        let src = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let (arena, roots) = crate::macrodep::read_file_with_macros(&src, &root)
            .map_err(|e| format!("{path}: {e}"))?;

        if crate::macrobuild::is_macro_library(&arena, &roots) {
            let out = build_macro_library(path, &src, out_dir)?;
            macro_outputs.push(out);
            continue;
        }

        let mut foreign = crate::macrodep::FileExpander::for_file(&root, &arena, &roots);
        let (arena, roots) = crate::expand::expand_file(
            arena,
            &roots,
            foreign
                .as_mut()
                .map(|f| f as &mut dyn crate::expand::ForeignExpander),
        )
        .map_err(|e| format!("{path}: {e}"))?;

        let exported = wit::export_names(&arena, &roots);
        let (arena, roots) = crate::check::resolve_overloads_excluding(arena, &roots, &exported)
            .map_err(|e| format!("{path}: {e}"))?;
        let info = wit::collect(&arena, &roots).map_err(|e| format!("{path}: {e}"))?;
        units.push(Unit {
            path: path.clone(),
            arena,
            roots,
            info,
        });
    }

    let index: HashMap<String, usize> = units
        .iter()
        .enumerate()
        .map(|(i, u)| (u.info.package_path.clone(), i))
        .collect();

    let wit_deps_dir = wit_deps_dir(paths);

    let mut outputs = macro_outputs;
    for u in &units {
        let mut deps = HashMap::new();
        for imp in &u.info.imports {
            if wit::is_macro_only(imp) {
                continue;
            }

            if let Some(&di) = index.get(&imp.package) {
                let d = &units[di];
                validate_import_version(imp, &d.info.package)?;
                let (type_defs, aliases) = emit::dep_non_record_types(&d.arena, &d.info);

                let type_ifaces = d
                    .info
                    .types
                    .iter()
                    .map(|(n, _)| (n.clone(), "api".to_string(), n.clone()))
                    .collect();
                deps.insert(
                    imp.package.clone(),
                    Dep {
                        package: d.info.package.clone(),
                        funcs: d.info.exports.clone(),
                        package_wit: emit::dep_package_wit(&d.arena, &d.info)?,
                        types: emit::dep_record_types(&d.arena, &d.info),
                        type_defs,
                        aliases,
                        type_ifaces,
                    },
                );
                continue;
            }

            if let Some(dir) = &wit_deps_dir
                && let Some(dep) = crate::witdep::resolve_dep(dir, &requested_package(imp))?
            {
                deps.insert(imp.package.clone(), dep);
                continue;
            }
            return Err(format!(
                "{}: import `{}` is not satisfied by any file in the build set or `wit/deps`",
                u.path, imp.path
            ));
        }

        for interface in u.info.exports.iter().map(|sig| sig.iface.as_str()).chain(
            u.info
                .resources
                .iter()
                .filter_map(|resource| resource.iface.as_deref()),
        ) {
            let Some((pkg, iface)) = interface.split_once('/') else {
                continue;
            };
            if !pkg.contains(':') || deps.contains_key(pkg) {
                continue;
            }
            if let Some(dir) = &wit_deps_dir
                && let Some(dep) = crate::witdep::resolve_dep(
                    dir,
                    &match iface.split_once('@') {
                        Some((_, version)) => format!("{pkg}@{version}"),
                        None => pkg.to_owned(),
                    },
                )?
            {
                deps.insert(pkg.to_string(), dep);
            }
        }

        let deps = emit::scope_dependency_types(&u.info, &deps);
        let mut import_sigs = crate::check::ImportSigs::new();
        for imp in &u.info.imports {
            if wit::is_macro_only(imp) {
                continue;
            }
            if let Some(dep) = deps.get(&imp.package) {
                let iface = imp.path.split_once('/').map(|(_, i)| i).unwrap_or("api");
                import_sigs.extend(wit::import_sigs_for(&imp.alias, &dep.funcs, iface));
            }
        }
        import_sigs.extend(wit::functor_import_sigs(&u.info.functors));
        crate::check::check_program_with_dependencies(
            &u.arena,
            &u.roots,
            &import_sigs,
            &deps,
            &u.info.imports,
        )
        .map_err(|e| format!("{}: {e}", u.path))?;
        let bytes = emit::emit_component(&u.arena, &u.roots, &u.info, &deps)
            .map_err(|e| format!("{}: {e}", u.path))?;
        let out = format!("{out_dir}/{}.wasm", u.info.package_path.replace(':', "-"));
        std::fs::write(&out, &bytes).map_err(|e| format!("{out}: {e}"))?;
        outputs.push(out);
    }

    if let Some(root) = project_root(paths)
        && let Err(e) = populate_wit(&root, &units)
    {
        eprintln!("warning: could not populate wit/ via wkg: {e}");
    }

    match compose_units(&units, &index, out_dir) {
        Ok(Some(app)) => outputs.push(app),
        Ok(None) => {}
        Err(e) => eprintln!("warning: could not compose components via wac: {e}"),
    }

    Ok(outputs)
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

fn build_macro_library(path: &str, src: &str, out_dir: &str) -> Result<String, String> {
    let (arena, roots) = crate::read_file(src).map_err(|e| format!("{path}: {e}"))?;
    let info = wit::collect(&arena, &roots).map_err(|e| format!("{path}: {e}"))?;

    let bytes = crate::macrobuild::build_macro_component(&arena, &roots)
        .map_err(|e| format!("{path}: {e}"))?;

    let out = format!("{out_dir}/{}.wasm", info.package_path.replace(':', "-"));
    std::fs::write(&out, &bytes).map_err(|e| format!("{out}: {e}"))?;
    Ok(out)
}

fn compose_units(
    units: &[Unit],
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
    let mut units = Vec::new();
    for path in src_paths {
        let path_str = path.display().to_string();
        let src = std::fs::read_to_string(path).map_err(|e| format!("{path_str}: {e}"))?;
        let (arena, roots) = crate::macrodep::read_file_with_macros(&src, root)
            .map_err(|e| format!("{path_str}: {e}"))?;
        let mut foreign = crate::macrodep::FileExpander::for_file(root, &arena, &roots);
        let (arena, roots) = crate::expand::expand_file(
            arena,
            &roots,
            foreign
                .as_mut()
                .map(|f| f as &mut dyn crate::expand::ForeignExpander),
        )
        .map_err(|e| format!("{path_str}: {e}"))?;
        let info = wit::collect(&arena, &roots).map_err(|e| format!("{path_str}: {e}"))?;
        units.push(Unit {
            path: path_str,
            arena,
            roots,
            info,
        });
    }
    populate_wit(root, &units)
}

fn populate_wit(root: &Path, units: &[Unit]) -> Result<(), String> {
    let local_packages: Vec<_> = units.iter().map(|u| u.info.package_path.clone()).collect();
    let host_units: Vec<&Unit> = units
        .iter()
        .filter(|u| wit::has_external_deps(&u.info, &local_packages))
        .collect();
    if host_units.is_empty() {
        return Ok(());
    }

    let wit_dir = root.join("wit");
    std::fs::create_dir_all(&wit_dir).map_err(|e| format!("{}: {e}", wit_dir.display()))?;

    tools::version(tools::Tool::Wkg)?;

    for u in &host_units {
        let world = wit::synthesize_fetch_world_excluding(&u.arena, &u.roots, &local_packages)
            .map_err(|e| format!("{}: {e}", u.path))?;

        clear_root_wit(&wit_dir)?;
        let file = wit_dir.join(format!("{}.wit", u.info.world));
        std::fs::write(&file, world).map_err(|e| format!("{}: {e}", file.display()))?;
        tools::wkg_wit_fetch(&wit_dir)?;
    }
    Ok(())
}

fn clear_root_wit(wit_dir: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(wit_dir).map_err(|e| format!("{}: {e}", wit_dir.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", wit_dir.display()))?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("wit") {
            std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
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

fn wit_deps_dir(paths: &[String]) -> Option<std::path::PathBuf> {
    Some(project_root(paths)?.join("wit").join("deps"))
}

pub fn compose_files(paths: &[String], out: &str) -> Result<(), String> {
    use wac_graph::{CompositionGraph, EncodeOptions, types::Package};

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
