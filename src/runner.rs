use crate::form::{Arena, Node, NodeId};

pub fn run_files(paths: &[String]) -> Result<(), String> {
    run_files_compiled(paths)
}

fn project_root(paths: &[String]) -> std::path::PathBuf {
    let Some(first) = paths.first() else {
        return std::path::PathBuf::from(".");
    };
    let src_dir = std::path::Path::new(first).parent();
    let root = src_dir.and_then(|d| d.parent()).or(src_dir);
    match root {
        Some(r) if !r.as_os_str().is_empty() => r.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    }
}

pub fn run_files_compiled(paths: &[String]) -> Result<(), String> {
    use std::path::Path;

    let Some(entry) = paths.first() else {
        return Err("no input files".to_string());
    };

    let mut project = crate::staging::Project::new().map_err(|e| e.to_string())?;
    for path in paths {
        project.copy(Path::new(path)).map_err(|e| e.to_string())?;
    }
    project
        .copy_dependencies(&project_root(paths))
        .map_err(|e| e.to_string())?;
    project.build().map_err(|e| e.to_string())?;
    let staged = project.paths();
    let out_dir = project.output();

    let entry_src = std::fs::read_to_string(&staged[0]).map_err(|e| format!("read: {e}"))?;
    let (arena, roots) = crate::read_file(&entry_src).map_err(|e| format!("read: {e}"))?;
    let pkg = find_package_full(&arena, &roots)
        .ok_or_else(|| format!("{entry}: no `Package` declaration"))?;
    let (pkg_path, version) = match pkg.split_once('@') {
        Some((p, v)) => (p.to_string(), Some(v.to_string())),
        None => (pkg.clone(), None),
    };
    let iface = match &version {
        Some(v) => format!("{pkg_path}/api@{v}"),
        None => format!("{pkg_path}/api"),
    };

    let app = out_dir.join("app.wasm");
    let entry_component = out_dir.join(format!("{}.wasm", pkg_path.replace(':', "-")));
    let artifact = if app.exists() {
        app
    } else if entry_component.exists() {
        entry_component
    } else {
        return Err(format!(
            "no runnable artifact for `{pkg_path}` (a multi-component program \
                 needs composition, which requires `wac`)"
        ));
    };

    let mut component =
        crate::host::HostComponent::from_file(&artifact).map_err(|e| format!("load: {e}"))?;

    component
        .call_instance(&iface, "run", &[])
        .map_err(|e| format!("{entry}: {e}"))?;
    Ok(())
}

pub fn run(paths: &[String]) -> Result<(), String> {
    run_files_compiled(paths)
}

fn find_package_full(arena: &Arena, roots: &[NodeId]) -> Option<String> {
    for &root in roots {
        if let Node::Tup(items) = arena.node(root)
            && items.len() >= 2
            && matches!(arena.node(items[0]), Node::Sym(s) if s == "package-MACRO")
            && let Node::Str(s) = arena.node(items[1])
        {
            return Some(s.clone());
        }
    }
    None
}
