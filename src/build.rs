//! `wavelet build`: one component per file (§9), and
//! `wavelet compose`: auto-plug components into one app (§6.5).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::emit::{self, Dep};
use crate::form::{Arena, NodeId};
use crate::tools;
use crate::wit::{self, FileInfo};

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
            // External host imports (wasi:*) are satisfied by the host at
            // runtime, not by a sibling file; their WIT is vendored, so they
            // need no Dep entry.
            if emit::is_external_package(&imp.package) {
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
        let bytes = emit::emit_component(&u.arena, &u.roots, &u.info, &deps)
            .map_err(|e| format!("{}: {e}", u.path))?;
        let out = format!("{out_dir}/{}.wasm", u.info.package_path.replace(':', "-"));
        std::fs::write(&out, &bytes).map_err(|e| format!("{out}: {e}"))?;
        outputs.push(out);
    }

    // Synthesize the project's own WIT into `wit/` and let `wkg` fetch+lock its
    // host (`wasi:*`) dependencies into `wit/deps`. This runs *behind the
    // scenes*: codegen above is unchanged and still uses the magic path. A
    // toolless or offline environment (e.g. CI) just skips it with a warning —
    // the build artifacts are already written.
    if let Some(root) = project_root(paths)
        && let Err(e) = populate_wit(&root, &units)
    {
        eprintln!("warning: could not populate wit/ via wkg: {e}");
    }

    Ok(outputs)
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
