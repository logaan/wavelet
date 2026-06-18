//! `wavelet build`: one component per file (§9), and
//! `wavelet compose`: auto-plug components into one app (§6.5).

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
    let mut units = Vec::new();
    for path in paths {
        let src = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let (arena, roots) =
            crate::read_file(&src).map_err(|e| format!("{path}: {e}"))?;
        let (arena, roots) =
            crate::expand::expand_file(arena, &roots).map_err(|e| format!("{path}: {e}"))?;
        let info = wit::collect(&arena, &roots).map_err(|e| format!("{path}: {e}"))?;
        units.push(Unit { path: path.clone(), arena, roots, info });
    }

    let index: HashMap<String, usize> = units
        .iter()
        .enumerate()
        .map(|(i, u)| (u.info.package_path.clone(), i))
        .collect();

    std::fs::create_dir_all(out_dir).map_err(|e| format!("{out_dir}: {e}"))?;

    // Project-level WIT vendored by `wkg` lives in `wit/deps`, a sibling of the
    // `src/` directory the sources come from. Used as a fallback source of
    // dependency interfaces after sibling-`.wvl` resolution.
    let wit_deps_dir = wit_deps_dir(paths);

    let mut outputs = Vec::new();
    for u in &units {
        let mut deps = HashMap::new();
        for imp in &u.info.imports {
            // A pure macro import (§6.3) is resolved to a macro component and
            // run during expand ([`crate::macrodep`]); it is *not* a runtime
            // dependency of this component, so it contributes no `Dep` and is
            // skipped here. (The common case is macro-only; an import that is
            // both a runtime dep and a macro library is an unsupported edge
            // case — see `wit::is_macro_only`.)
            if wit::is_macro_only(imp) {
                continue;
            }
            // (a) A sibling Wavelet file in the build set satisfies the import.
            if let Some(&di) = index.get(&imp.package) {
                let d = &units[di];
                deps.insert(
                    imp.package.clone(),
                    Dep {
                        package: d.info.package.clone(),
                        funcs: d.info.exports.clone(),
                        package_wit: emit::dep_package_wit(&d.arena, &d.info)?,
                        types: emit::dep_record_types(&d.arena, &d.info),
                        type_defs: Vec::new(),
                    },
                );
                continue;
            }
            // (b) Fall back to an external WIT package vendored under
            // `wit/deps`, parsed with `wit-parser` into the same `Dep` shape.
            // This is how host (`wasi:*`) imports now resolve: with a real `Dep`
            // carrying the interface's parsed signatures, the generic bridge
            // lowers calls into them (http is routed this way as of Step 8).
            if let Some(dir) = &wit_deps_dir {
                if let Some(dep) = crate::witdep::resolve_dep(dir, &imp.package)? {
                    deps.insert(imp.package.clone(), dep);
                    continue;
                }
            }
            return Err(format!(
                "{}: import `{}` is not satisfied by any file in the build set or `wit/deps`",
                u.path, imp.path
            ));
        }
        // External interfaces this component *exports* (e.g.
        // `wasi:http/incoming-handler`, `wasi:cli/run`) also need their WIT
        // package available to the encoder so the generic export wrapper
        // validates against the real interface signature. Resolve each such
        // package from `wit/deps` into the same `Dep` map; the magic http/cli
        // export path supplies its own vendored WIT and needs no entry here.
        for sig in &u.info.exports {
            let Some((pkg, _)) = sig.iface.split_once('/') else { continue };
            if !pkg.contains(':') || deps.contains_key(pkg) {
                continue; // local iface (`api`) or already resolved.
            }
            if let Some(dir) = &wit_deps_dir
                && let Some(dep) = crate::witdep::resolve_dep(dir, pkg)?
            {
                deps.insert(pkg.to_string(), dep);
            }
        }
        let bytes = emit::emit_component(&u.arena, &u.roots, &u.info, &deps)
            .map_err(|e| format!("{}: {e}", u.path))?;
        let out = format!("{out_dir}/{}.wasm", u.info.package_path.replace(':', "-"));
        std::fs::write(&out, &bytes).map_err(|e| format!("{out}: {e}"))?;
        outputs.push(out);
    }

    // Synthesize the project's own WIT into `wit/` and let `wkg` fetch+lock its
    // host (`wasi:*`) dependencies into `wit/deps`. A toolless or offline
    // environment (e.g. CI) just skips it with a warning — the build artifacts
    // are already written.
    if let Some(root) = project_root(paths)
        && let Err(e) = populate_wit(&root, &units)
    {
        eprintln!("warning: could not populate wit/ via wkg: {e}");
    }

    // Compose the project's components into one final artifact, `<out>/app.wasm`,
    // by generating a `.wac` that wires each sibling import to the component that
    // exports it and running `wac compose`. Host (`wasi:*`) imports are left
    // unsatisfied for the runtime to provide. Only attempted when there is
    // something to wire (a component importing a sibling); a lone component is
    // already its own artifact. Best-effort like the `wkg` step: a missing `wac`
    // leaves the per-component wasm in place with a warning, so a toolless build
    // (and the hermetic single-component tests) still succeed.
    match compose_units(&units, &index, out_dir) {
        Ok(Some(app)) => outputs.push(app),
        Ok(None) => {}
        Err(e) => eprintln!("warning: could not compose components via wac: {e}"),
    }

    Ok(outputs)
}

/// Wire the build's components into one composed `<out_dir>/app.wasm`.
///
/// Returns `Ok(Some(path))` when a composed artifact was written, `Ok(None)`
/// when there was nothing to compose (no component imports a sibling — e.g. a
/// single-component build). Generates a `.wac` source describing the wiring and
/// runs `wac compose` via the Step 0 wrapper.
fn compose_units(
    units: &[Unit],
    index: &HashMap<String, usize>,
    out_dir: &str,
) -> Result<Option<String>, String> {
    // Which units does each unit pull in from the build set? An import is a
    // *sibling* edge when its package names another unit (host `wasi:*` imports
    // resolve from `wit/deps` and stay unsatisfied in the composed artifact).
    let mut edges: Vec<(usize, &ImportInfo, usize)> = Vec::new(); // (importer, import, plug)
    for (ui, u) in units.iter().enumerate() {
        for imp in &u.info.imports {
            // A pure macro import is a compile-time-only dependency, never a
            // sibling runtime edge to wire — skip it (see `wit::is_macro_only`).
            if wit::is_macro_only(imp) {
                continue;
            }
            if let Some(&plug) = index.get(&imp.package) {
                if plug != ui {
                    edges.push((ui, imp, plug));
                }
            }
        }
    }
    if edges.is_empty() {
        return Ok(None); // nothing to wire — the lone component is the artifact.
    }

    // The root export is the component nothing else imports. If several qualify
    // (disconnected pieces), prefer the one that imports a sibling — the entry
    // point that pulls the others in.
    let imported: std::collections::HashSet<usize> =
        edges.iter().map(|&(_, _, plug)| plug).collect();
    let importers: std::collections::HashSet<usize> =
        edges.iter().map(|&(ui, _, _)| ui).collect();
    let root = (0..units.len())
        .find(|i| !imported.contains(i) && importers.contains(i))
        .or_else(|| (0..units.len()).find(|i| !imported.contains(i)))
        .ok_or("no root component (cyclic composition?)")?;

    // A stable wac variable per unit, derived from its package path. WAC
    // identifiers are kebab labels (letters/digits/`-`, no `_`), so map the
    // package separators `:`/`/` to `-`.
    let var = |i: usize| units[i].info.package_path.replace([':', '/'], "-");

    // Emit `let` bindings in dependency order (a plug before any socket that
    // wires it), so each reference is already in scope.
    let order = topo_order(units.len(), &edges)?;
    let mut wac = String::from("// Generated by `wavelet build`. Wires the project's components into one\n");
    wac.push_str("// artifact; host (wasi:*) imports stay unsatisfied for the runtime.\n");
    wac.push_str("package wavelet:app;\n\n");
    for &i in &order {
        let wirings: Vec<String> = edges
            .iter()
            .filter(|&&(ui, _, _)| ui == i)
            .map(|&(_, imp, plug)| {
                // The composed import is versioned by the plug's package version,
                // e.g. `demo:shout/api@0.1.0`. Both the socket import and the
                // plug export name it identically.
                let iface = versioned_iface(&imp.path, &units[plug].info.package);
                format!("  \"{iface}\": {plug}[\"{iface}\"],", plug = var(plug), iface = iface)
            })
            .collect();
        if wirings.is_empty() {
            wac.push_str(&format!("let {} = new {} {{ }};\n", var(i), units[i].info.package_path));
        } else {
            wac.push_str(&format!("let {} = new {} {{\n", var(i), units[i].info.package_path));
            for w in &wirings {
                wac.push_str(w);
                wac.push('\n');
            }
            wac.push_str("  ...\n};\n");
        }
    }
    wac.push_str(&format!("\nexport {}...;\n", var(root)));

    // Write the `.wac` beside the components for inspection, then compose.
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

/// Order unit indices so every plug precedes the sockets that wire it (a
/// topological sort over the sibling edges). Errors on a cycle.
fn topo_order(n: usize, edges: &[(usize, &ImportInfo, usize)]) -> Result<Vec<usize>, String> {
    // plug -> importers (an edge `plug` must come before `importer`).
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

/// Append the package's version to an interface path: `demo:shout/api` +
/// `demo:shout@0.1.0` → `demo:shout/api@0.1.0`. An unversioned package leaves the
/// path unchanged.
fn versioned_iface(iface_path: &str, package_versioned: &str) -> String {
    match package_versioned.split_once('@') {
        Some((_, ver)) => format!("{iface_path}@{ver}"),
        None => iface_path.to_string(),
    }
}

/// Scaffold a project's `wit/` from its source files and fetch+lock its host
/// dependencies, without emitting any components.
///
/// Used by `wavelet new` so a fresh project ships with its dependency WIT
/// already vendored under `wit/deps` and pinned in `wkg.lock`. `root` is the
/// project root (the parent of `src/`); `src_paths` are its `.wvl` source files.
/// Like the build-time path, a `wkg` failure (absent tool, offline) is the
/// caller's to treat as a warning.
pub fn populate_project_wit(root: &Path, src_paths: &[PathBuf]) -> Result<(), String> {
    let mut units = Vec::new();
    for path in src_paths {
        let path_str = path.display().to_string();
        let src = std::fs::read_to_string(path).map_err(|e| format!("{path_str}: {e}"))?;
        let (arena, roots) = crate::read_file(&src).map_err(|e| format!("{path_str}: {e}"))?;
        let (arena, roots) =
            crate::expand::expand_file(arena, &roots).map_err(|e| format!("{path_str}: {e}"))?;
        let info = wit::collect(&arena, &roots).map_err(|e| format!("{path_str}: {e}"))?;
        units.push(Unit { path: path_str, arena, roots, info });
    }
    populate_wit(root, &units)
}

/// Synthesize each component's world into `<root>/wit/` and run `wkg wit fetch`
/// so the project ends up with a populated `wit/deps` and an up-to-date
/// `wkg.lock`.
///
/// Only components that actually reference host (`wasi:*`) packages need
/// fetching ([`wit::has_host_deps`]); pure components (a domain model with no
/// imports) contribute nothing to fetch. For each such component we write its
/// host-only fetch world ([`wit::synthesize_fetch_world`]) as the single root
/// package in `wit/` and run `wkg wit fetch`; deps and the lock accumulate in
/// the shared `wit/` across components.
///
/// Returns the first `wkg` error encountered. Callers treat that as a warning,
/// not a hard failure: the magic path has already produced the components.
fn populate_wit(root: &Path, units: &[Unit]) -> Result<(), String> {
    let host_units: Vec<&Unit> = units.iter().filter(|u| wit::has_host_deps(&u.info)).collect();
    if host_units.is_empty() {
        return Ok(());
    }

    let wit_dir = root.join("wit");
    std::fs::create_dir_all(&wit_dir).map_err(|e| format!("{}: {e}", wit_dir.display()))?;

    // Preflight: surface a clear, actionable error if `wkg` is absent before we
    // start writing world files.
    tools::version(tools::Tool::Wkg)?;

    for u in &host_units {
        let world = wit::synthesize_fetch_world(&u.arena, &u.roots)
            .map_err(|e| format!("{}: {e}", u.path))?;
        // One root package per `wit/` at a time: clear any stale root world file
        // from a previous component before writing this one. (Fetched packages
        // live under `wit/deps`, never at the `wit/` root, so they are kept.)
        clear_root_wit(&wit_dir)?;
        let file = wit_dir.join(format!("{}.wit", u.info.world));
        std::fs::write(&file, world).map_err(|e| format!("{}: {e}", file.display()))?;
        tools::wkg_wit_fetch(&wit_dir)?;
    }
    Ok(())
}

/// Remove top-level `*.wit` files from `wit/` (the synthesized root world),
/// leaving `wit/deps` and `wkg.lock` in place.
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

/// The project root: the parent of the `src/` directory the sources live in
/// (so `wit/` and `wkg.lock` sit beside `src/`). Mirrors [`wit_deps_dir`].
fn project_root(paths: &[String]) -> Option<PathBuf> {
    let first = paths.first()?;
    let src_dir = Path::new(first).parent()?;
    let root = src_dir.parent().unwrap_or(src_dir);
    // A bare `src/foo.wvl` yields an empty parent; normalize to `.` so callers
    // never hand an empty path to `current_dir` (which the OS rejects).
    Some(if root.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        root.to_path_buf()
    })
}

/// Locate the project's `wit/deps` directory from its source files.
///
/// A project lays its sources under `src/`, with `wit/` as a sibling of `src/`
/// (so `wit/deps` is `<src-parent>/wit/deps`). We derive it from the first
/// source path's parent directory. Returns `None` if no parent can be found.
fn wit_deps_dir(paths: &[String]) -> Option<std::path::PathBuf> {
    Some(project_root(paths)?.join("wit").join("deps"))
}

/// Compose components: the first file is the entry ("socket"); the rest are
/// plugs whose exports satisfy its imports. Auto-plug semantics (§6.5).
pub fn compose_files(paths: &[String], out: &str) -> Result<(), String> {
    use wac_graph::{types::Package, CompositionGraph, EncodeOptions};

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
            // wac package names are `ns:name`; reverse the build step's `-`
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
