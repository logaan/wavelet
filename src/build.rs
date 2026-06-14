//! `wavelet build`: one component per file (§9), and
//! `wavelet compose`: auto-plug components into one app (§6.5).

use std::collections::HashMap;
use std::path::Path;

use crate::emit::{self, Dep};
use crate::form::{Arena, NodeId};
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
    Ok(outputs)
}

/// Locate the project's `wit/deps` directory from its source files.
///
/// A project lays its sources under `src/`, with `wit/` as a sibling of `src/`
/// (so `wit/deps` is `<src-parent>/wit/deps`). We derive it from the first
/// source path's parent directory. Returns `None` if no parent can be found.
fn wit_deps_dir(paths: &[String]) -> Option<std::path::PathBuf> {
    let first = paths.first()?;
    let src_dir = Path::new(first).parent()?;
    // `src/foo.wvl` -> project root is `src/`'s parent; `wit/` sits beside it.
    let root = src_dir.parent().unwrap_or(src_dir);
    Some(root.join("wit").join("deps"))
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
