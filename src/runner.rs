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
    /// (iface, name) — `api` unless the Export record form groups otherwise
    pub exports: Vec<(String, String)>,
    pub state: ModState,
}

#[derive(PartialEq)]
pub enum ModState {
    Unevaluated,
    Evaluating,
    Done,
}

/// Load and evaluate a set of `.wvl` files; the first is the entry component.
/// Imports are resolved by package id against the other files — an
/// interpreter stand-in for `wavelet compose` (§6.5).
pub fn run_files(paths: &[String]) -> Result<(), String> {
    let interp = Interp::new();
    let std_env = Env::root();
    builtins::install(&std_env);

    let mut modules = Vec::new();
    let mut by_package = HashMap::new();
    for path in paths {
        let src = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let (arena, roots) = crate::read_file(&src).map_err(|e| format!("{path}: {e}"))?;
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
    if let Some(run) = entry.env.lookup("run") {
        if matches!(run, Value::Closure(_)) {
            interp
                .apply(&run, Value::Lst(vec![]))
                .map_err(|e| format!("{}: {e}", entry.path))?;
        }
    }
    Ok(())
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
        // Top-level forms are tuples `Tup[head, …args]`. The arity-1 special
        // heads (Package/Target/Import/Export) take `items[1]` as their payload.
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
            ("package-MACRO" | "target-MACRO" | "def-type-MACRO", _) => {}
            ("export-MACRO", Some(payload)) => {
                let entry = export_entry(&arena, payload)
                    .ok_or(format!("{path}: malformed Export"))?;
                modules[idx].exports.push(entry);
            }
            ("import-MACRO", Some(payload)) => {
                let spec = parse_import(&arena, payload)
                    .ok_or(format!("{path}: malformed Import"))?;
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
                    modules[idx].env.define(format!("{}/{name}", spec.alias), v.clone());
                    if spec.open {
                        modules[idx].env.define(name, v);
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

fn find_package(arena: &Arena, roots: &[NodeId]) -> Option<String> {
    for &root in roots {
        if let Node::Tup(items) = arena.node(root) {
            if items.len() >= 2
                && matches!(arena.node(items[0]), Node::Sym(s) if s == "package-MACRO")
            {
                if let Node::Str(s) = arena.node(items[1]) {
                    return Some(strip_version(s));
                }
            }
        }
    }
    None
}

/// `"demo:shout@0.1.0"` -> `"demo:shout"`
fn strip_version(s: &str) -> String {
    s.split('@').next().unwrap_or(s).to_string()
}

fn export_entry(arena: &Arena, payload: NodeId) -> Option<(String, String)> {
    match arena.node(payload) {
        Node::Sym(s) => Some(("api".to_string(), s.clone())),
        Node::Rec(fields) => {
            let name = fields.iter().find(|(k, _)| k == "name").and_then(|(_, v)| {
                match arena.node(*v) {
                    Node::Sym(s) => Some(s.clone()),
                    _ => None,
                }
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
    /// full interface path as written, e.g. `demo:shout/api`
    path: String,
    /// package id, e.g. `demo:shout`
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
    Some(ImportSpec { path, package, alias, open })
}
