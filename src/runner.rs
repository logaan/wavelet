use std::collections::HashMap;
use std::rc::Rc;

use crate::builtins;
use crate::form::{Arena, Node, NodeId};
use crate::interp::Interp;
use crate::value::{Env, Value};

pub struct Module {
    pub path: String,
    pub arena: Rc<Arena>,
    pub roots: Vec<NodeId>,
    pub package: Option<String>,
    pub env: Env,

    pub exports: Vec<(String, String)>,
    pub state: ModState,
}

#[derive(PartialEq)]
pub enum ModState {
    Unevaluated,
    Evaluating,
    Done,
}

pub fn run_files(paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("no input files".into());
    }
    let interp = Interp::new();
    let std_env = Env::root();
    builtins::install(&std_env);

    let root = project_root(paths);
    let mut modules = Vec::new();
    let mut by_package = HashMap::new();
    for path in paths {
        let src = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let (arena, roots) = crate::macrodep::read_file_with_macros(&src, &root)
            .map_err(|e| format!("{path}: {e}"))?;

        let mut foreign = crate::macrodep::FileExpander::for_file(&root, &arena, &roots);
        let (arena, roots) = crate::expand::expand_file(
            arena,
            &roots,
            foreign
                .as_mut()
                .map(|f| f as &mut dyn crate::expand::ForeignExpander),
        )
        .map_err(|e| format!("{path}: {e}"))?;

        let (arena, roots) =
            crate::check::resolve_overloads(arena, &roots).map_err(|e| format!("{path}: {e}"))?;
        let arena = Rc::new(arena);
        let package = find_package(&arena, &roots);
        if let Some(pkg) = &package {
            by_package.insert(pkg.clone(), modules.len());
        }
        modules.push(Module {
            path: path.clone(),
            arena,
            roots,
            package,
            env: std_env.child(),
            exports: Vec::new(),
            state: ModState::Unevaluated,
        });
    }

    eval_module(&interp, &mut modules, &by_package, 0)?;

    let entry = &modules[0];
    match entry.env.lookup("run") {
        Some(run @ Value::Closure(_)) => {
            interp
                .apply(&run, Value::Tup(vec![]))
                .map_err(|e| format!("{}: {e}", entry.path))?;
        }
        Some(_) => {
            return Err(format!(
                "{}: `run` is defined but is not a closure to execute",
                entry.path
            ));
        }
        None => {
            return Err(format!(
                "{}: nothing to run — define a no-argument closure named `run` \
                 (e.g. `Def run Fn {{}} …`) as the entry point",
                entry.path
            ));
        }
    }
    interp.teardown().map_err(|e| e.to_string())
}

fn eval_module(
    interp: &Interp,
    modules: &mut Vec<Module>,
    by_package: &HashMap<String, usize>,
    idx: usize,
) -> Result<(), String> {
    if modules[idx].state == ModState::Done {
        return Ok(());
    }
    if modules[idx].state == ModState::Evaluating {
        return Err(format!("{}: import cycle", modules[idx].path));
    }
    modules[idx].state = ModState::Evaluating;

    let arena = modules[idx].arena.clone();
    let roots = modules[idx].roots.clone();
    let path = modules[idx].path.clone();

    for root in roots {
        let Node::Tup(items) = arena.node(root) else {
            interp
                .eval(&arena, root, &modules[idx].env)
                .map_err(|e| format!("{path}: {e}"))?;
            continue;
        };
        let head_name = match items.first() {
            Some(&head) => match arena.node(head) {
                Node::Sym(s) => s.as_str(),
                _ => "",
            },
            None => "",
        };
        let payload = items.get(1).copied();
        match (head_name, payload) {
            ("package-MACRO" | "target-MACRO", _) => {}
            ("export-MACRO", Some(payload)) => {
                let entry =
                    export_entry(&arena, payload).ok_or(format!("{path}: malformed Export"))?;
                modules[idx].exports.push(entry);
            }

            ("instantiate-MACRO", Some(payload)) => {
                let functor = builtins::parse_functor_import(&arena, payload)
                    .ok_or(format!("{path}: malformed Instantiate (unknown functor)"))?;
                builtins::bind_functor(&modules[idx].env, &functor);
            }
            ("import-MACRO", Some(payload)) => {
                if builtins::parse_functor_import(&arena, payload).is_some() {
                    return Err(format!(
                        "{path}: functor packages are applied with \
                         `Instantiate {{pkg: … with: {{elem: t}} as: alias}}`, \
                         not `Import`"
                    ));
                }
                let spec =
                    parse_import(&arena, payload).ok_or(format!("{path}: malformed Import"))?;
                let dep = *by_package.get(&spec.package).ok_or(format!(
                    "{path}: unresolved import `{}` (no file provides package `{}`)",
                    spec.path, spec.package
                ))?;
                eval_module(interp, modules, by_package, dep)?;
                let want_iface = match spec.path.split_once('/') {
                    Some((_, i)) => i.to_string(),
                    None => "api".to_string(),
                };
                let names: Vec<String> = modules[dep]
                    .exports
                    .iter()
                    .filter(|(i, _)| *i == want_iface)
                    .map(|(_, n)| n.clone())
                    .collect();
                if names.is_empty() && !modules[dep].exports.is_empty() {
                    return Err(format!(
                        "{path}: `{}` exports nothing in interface `{want_iface}`",
                        modules[dep].path
                    ));
                }
                let dep_env = modules[dep].env.clone();
                for name in names {
                    let v = dep_env.lookup(&name).ok_or(format!(
                        "{}: exported `{name}` is not defined",
                        modules[dep].path
                    ))?;
                    modules[idx]
                        .env
                        .define(format!("{}-{name}", spec.alias), v.clone());
                    if spec.open {
                        modules[idx].env.define(name, v);
                    }
                }

                for case in deftype_case_names(&modules[dep].arena, &modules[dep].roots) {
                    if let Some(v) = dep_env.lookup(&case) {
                        modules[idx]
                            .env
                            .define(format!("{}-{case}", spec.alias), v.clone());
                        if spec.open {
                            modules[idx].env.define(case, v);
                        }
                    }
                }
            }
            _ => {
                interp
                    .eval(&arena, root, &modules[idx].env)
                    .map_err(|e| format!("{path}: {e}"))?;
            }
        }
    }
    modules[idx].state = ModState::Done;
    Ok(())
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

fn deftype_case_names(arena: &Arena, roots: &[NodeId]) -> Vec<String> {
    let mut out = Vec::new();
    for &root in roots {
        let Node::Tup(items) = arena.node(root) else {
            continue;
        };
        if items.len() != 3 || !matches!(arena.node(items[0]), Node::Sym(s) if s == "deftype-MACRO")
        {
            continue;
        }
        let Node::Lst(cases) = arena.node(items[2]) else {
            continue;
        };
        for &c in cases {
            match arena.node(c) {
                Node::Sym(case) => out.push(case.clone()),
                Node::Tup(case_items) => {
                    if let Some(&h) = case_items.first()
                        && let Node::Sym(case) = arena.node(h)
                    {
                        out.push(case.clone());
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn find_package(arena: &Arena, roots: &[NodeId]) -> Option<String> {
    for &root in roots {
        if let Node::Tup(items) = arena.node(root)
            && items.len() >= 2
            && matches!(arena.node(items[0]), Node::Sym(s) if s == "package-MACRO")
            && let Node::Str(s) = arena.node(items[1])
        {
            return Some(strip_version(s));
        }
    }
    None
}

fn strip_version(s: &str) -> String {
    s.split('@').next().unwrap_or(s).to_string()
}

fn export_entry(arena: &Arena, payload: NodeId) -> Option<(String, String)> {
    match arena.node(payload) {
        Node::Sym(s) => Some(("api".to_string(), s.clone())),
        Node::Rec(fields) => {
            let name = fields
                .iter()
                .find(|(k, _)| k == "name")
                .and_then(|(_, v)| match arena.node(*v) {
                    Node::Sym(s) => Some(s.clone()),
                    _ => None,
                })?;
            let iface = fields
                .iter()
                .find(|(k, _)| k == "iface")
                .and_then(|(_, v)| match arena.node(*v) {
                    Node::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| "api".to_string());
            Some((iface, name))
        }
        _ => None,
    }
}

struct ImportSpec {
    path: String,

    package: String,
    alias: String,
    open: bool,
}

fn parse_import(arena: &Arena, payload: NodeId) -> Option<ImportSpec> {
    let (pkg_str, alias, open) = match arena.node(payload) {
        Node::Str(s) => (s.clone(), None, false),
        Node::Rec(fields) => {
            let mut pkg = None;
            let mut alias = None;
            let mut open = false;
            for (k, v) in fields {
                match (k.as_str(), arena.node(*v)) {
                    ("pkg", Node::Str(s)) => pkg = Some(s.clone()),
                    ("as", Node::Sym(s)) => alias = Some(s.clone()),
                    ("open", Node::Bool(b)) => open = *b,
                    ("macros", _) => {}
                    _ => return None,
                }
            }
            (pkg?, alias, open)
        }
        _ => return None,
    };
    let path = strip_version(&pkg_str);
    let package = path.split('/').next().unwrap_or(&path).to_string();
    let alias = alias.unwrap_or_else(|| {
        path.rsplit('/')
            .next()
            .unwrap_or(&path)
            .rsplit(':')
            .next()
            .unwrap_or(&path)
            .to_string()
    });
    Some(ImportSpec {
        path,
        package,
        alias,
        open,
    })
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
