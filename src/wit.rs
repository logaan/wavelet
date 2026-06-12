use std::collections::HashMap;

use crate::form::{Arena, Node, NodeId};

/// Everything the toolchain knows about one file's component surface (§6.1).
pub struct FileInfo {
    /// package id with version, e.g. `demo:shout@0.1.0`
    pub package: String,
    /// package id without version, e.g. `demo:shout`
    pub package_path: String,
    /// world name, e.g. `shout`
    pub world: String,
    pub target: Option<String>,
    pub imports: Vec<ImportInfo>,
    pub exports: Vec<FuncSig>,
    pub types: Vec<(String, NodeId)>,
    /// all module-level `Def name Fn …` definitions: name -> (params, body)
    pub defs: HashMap<String, (NodeId, NodeId)>,
    /// non-function module-level defs, in file order: (name, expr)
    pub value_defs: Vec<(String, NodeId)>,
}

pub struct ImportInfo {
    /// interface path as written, version stripped, e.g. `demo:shout/api`
    pub path: String,
    /// package part, e.g. `demo:shout`
    pub package: String,
    pub alias: String,
}

#[derive(Clone)]
pub struct FuncSig {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub result: Option<String>,
}

impl FuncSig {
    pub fn to_wit(&self) -> String {
        let ps: Vec<String> = self.params.iter().map(|(n, t)| format!("{n}: {t}")).collect();
        match &self.result {
            Some(r) => format!("{}: func({}) -> {r};", self.name, ps.join(", ")),
            None => format!("{}: func({});", self.name, ps.join(", ")),
        }
    }
}

pub fn collect(arena: &Arena, roots: &[NodeId]) -> Result<FileInfo, String> {
    let mut package = None;
    let mut target = None;
    let mut imports = Vec::new();
    let mut export_decls: Vec<(String, Option<FuncSig>)> = Vec::new();
    let mut types = Vec::new();
    let mut defs = HashMap::new();
    let mut value_defs = Vec::new();

    for &root in roots {
        let Node::Call(head, payload) = arena.node(root) else { continue };
        let Node::Sym(head_name) = arena.node(*head) else { continue };
        match head_name.as_str() {
            "package-MACRO" => {
                if let Node::Str(s) = arena.node(*payload) {
                    package = Some(s.clone());
                }
            }
            "target-MACRO" => {
                if let Node::Str(s) = arena.node(*payload) {
                    target = Some(s.clone());
                }
            }
            "import-MACRO" => {
                let spec = match arena.node(*payload) {
                    Node::Str(s) => Some((s.clone(), None)),
                    Node::Rec(fields) => {
                        let mut pkg = None;
                        let mut alias = None;
                        for (k, v) in fields {
                            match (k.as_str(), arena.node(*v)) {
                                ("pkg", Node::Str(s)) => pkg = Some(s.clone()),
                                ("as", Node::Sym(s)) => alias = Some(s.clone()),
                                _ => {}
                            }
                        }
                        pkg.map(|p| (p, alias))
                    }
                    _ => None,
                };
                let (pkg_str, alias) = spec.ok_or("malformed Import")?;
                let path = strip_version(&pkg_str);
                let pkg_part = path.split('/').next().unwrap_or(&path).to_string();
                let alias = alias.unwrap_or_else(|| {
                    path.rsplit('/').next().unwrap_or(&path).to_string()
                });
                imports.push(ImportInfo { path, package: pkg_part, alias });
            }
            "export-MACRO" => match arena.node(*payload) {
                Node::Sym(s) => export_decls.push((s.clone(), None)),
                Node::Rec(fields) => {
                    let sig = parse_explicit_sig(arena, fields).ok_or("malformed Export")?;
                    export_decls.push((sig.name.clone(), Some(sig)));
                }
                _ => return Err("malformed Export".into()),
            },
            "def-type-MACRO" => {
                if let Node::Tup(items) = arena.node(*payload) {
                    if items.len() == 2 {
                        if let Node::Sym(name) = arena.node(items[0]) {
                            types.push((name.clone(), items[1]));
                        }
                    }
                }
            }
            "def-MACRO" => {
                if let Node::Tup(items) = arena.node(*payload) {
                    if items.len() == 2 {
                        if let Node::Sym(name) = arena.node(items[0]) {
                            let mut is_fn = false;
                            if let Node::Call(fh, fp) = arena.node(items[1]) {
                                if matches!(arena.node(*fh), Node::Sym(s) if s == "fn-MACRO") {
                                    if let Node::Tup(fn_parts) = arena.node(*fp) {
                                        if fn_parts.len() == 2 {
                                            defs.insert(
                                                name.clone(),
                                                (fn_parts[0], fn_parts[1]),
                                            );
                                            is_fn = true;
                                        }
                                    }
                                }
                            }
                            if !is_fn {
                                value_defs.push((name.clone(), items[1]));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let package = package.ok_or("file has no Package declaration")?;
    let package_path = strip_version(&package);
    let world = package_path
        .rsplit(':')
        .next()
        .unwrap_or("component")
        .to_string();

    let mut exports = Vec::new();
    for (name, explicit) in export_decls {
        let sig = match explicit {
            Some(sig) => sig,
            None => {
                let (params_id, body) = defs
                    .get(&name)
                    .ok_or(format!("Export `{name}` has no definition"))?;
                infer_sig(arena, &name, *params_id, *body)?
            }
        };
        exports.push(sig);
    }

    Ok(FileInfo {
        package,
        package_path,
        world,
        target,
        imports,
        exports,
        types,
        defs,
        value_defs,
    })
}

/// `"demo:shout@0.1.0"` -> `"demo:shout"`
fn strip_version(s: &str) -> String {
    s.split('@').next().unwrap_or(s).to_string()
}

fn parse_explicit_sig(arena: &Arena, fields: &[(String, NodeId)]) -> Option<FuncSig> {
    let mut name = None;
    let mut params = Vec::new();
    let mut result = None;
    for (k, v) in fields {
        match (k.as_str(), arena.node(*v)) {
            ("name", Node::Sym(s)) => name = Some(s.clone()),
            ("params", Node::Rec(pfields)) => {
                for (pk, pv) in pfields {
                    params.push((pk.clone(), type_text(arena, *pv).ok()?));
                }
            }
            ("result", _) => result = Some(type_text(arena, *v).ok()?),
            _ => {}
        }
    }
    Some(FuncSig { name: name?, params, result })
}

fn infer_sig(
    arena: &Arena,
    name: &str,
    params_id: NodeId,
    body: NodeId,
) -> Result<FuncSig, String> {
    let mut params = Vec::new();
    let mut param_types = HashMap::new();
    match arena.node(params_id) {
        Node::Flg(names) if names.is_empty() => {}
        Node::Flg(names) => {
            return Err(format!(
                "cannot synthesize WIT for `{name}`: parameters {names:?} are untyped \
                 (annotate them or use the Export record form)"
            ));
        }
        Node::Rec(fields) => {
            for (k, v) in fields {
                let t = type_text(arena, *v)?;
                param_types.insert(k.clone(), t.clone());
                params.push((k.clone(), t));
            }
        }
        _ => return Err(format!("`{name}`: malformed Fn parameters")),
    }
    let result = match infer(arena, body, &param_types) {
        Inferred::Known(t) => Some(t),
        Inferred::Unit => None,
        Inferred::Unknown => {
            return Err(format!(
                "cannot infer result type of `{name}` (use the Export record form)"
            ));
        }
    };
    Ok(FuncSig { name: name.to_string(), params, result })
}

/// Synthesize WIT text for a file (§6.1), as shown by `wavelet wit`.
pub fn synthesize(arena: &Arena, roots: &[NodeId]) -> Result<String, String> {
    let info = collect(arena, roots)?;
    let mut out = String::new();
    out.push_str(&format!("package {};\n", info.package));

    if !info.exports.is_empty() || !info.types.is_empty() {
        out.push_str("\ninterface api {\n");
        for (name, ty) in &info.types {
            out.push_str(&format!("  {};\n", type_decl(arena, name, *ty)?));
        }
        for sig in &info.exports {
            out.push_str(&format!("  {}\n", sig.to_wit()));
        }
        out.push_str("}\n");
    }

    out.push_str(&format!("\nworld {} {{\n", info.world));
    if let Some(t) = &info.target {
        out.push_str(&format!("  include {t};\n"));
    }
    for imp in &info.imports {
        out.push_str(&format!("  import {};\n", imp.path));
    }
    if !info.exports.is_empty() {
        out.push_str("  export api;\n");
    }
    out.push_str("}\n");
    Ok(out)
}

pub fn type_decl(arena: &Arena, name: &str, ty: NodeId) -> Result<String, String> {
    match arena.node(ty) {
        Node::Rec(fields) => {
            let mut parts = Vec::new();
            for (k, v) in fields {
                parts.push(format!("{k}: {}", type_text(arena, *v)?));
            }
            Ok(format!("record {name} {{ {} }}", parts.join(", ")))
        }
        Node::Flg(names) => Ok(format!("flags {name} {{ {} }}", names.join(", "))),
        Node::Lst(cases) => {
            let mut parts = Vec::new();
            for &c in cases {
                match arena.node(c) {
                    Node::Sym(s) => parts.push(s.clone()),
                    Node::Call(h, p) => {
                        let Node::Sym(case) = arena.node(*h) else {
                            return Err(format!("bad variant case in `{name}`"));
                        };
                        parts.push(format!("{case}({})", type_text(arena, *p)?));
                    }
                    _ => return Err(format!("bad variant case in `{name}`")),
                }
            }
            Ok(format!("variant {name} {{ {} }}", parts.join(", ")))
        }
        _ => Ok(format!("type {name} = {}", type_text(arena, ty)?)),
    }
}

/// A type form as WIT text: `string`, `list(u8)` -> `list<u8>`,
/// `result(t e)` -> `result<t, e>`, `tuple[a b]` -> `tuple<a, b>`.
pub fn type_text(arena: &Arena, id: NodeId) -> Result<String, String> {
    match arena.node(id) {
        Node::Sym(s) => Ok(s.clone()),
        Node::Call(head, payload) => {
            let Node::Sym(ctor) = arena.node(*head) else {
                return Err("bad type form".into());
            };
            let args = match arena.node(*payload) {
                Node::Tup(items) | Node::Lst(items) => items
                    .iter()
                    .map(|&i| type_text(arena, i))
                    .collect::<Result<Vec<_>, _>>()?,
                _ => vec![type_text(arena, *payload)?],
            };
            Ok(format!("{ctor}<{}>", args.join(", ")))
        }
        _ => Err("bad type form".into()),
    }
}

enum Inferred {
    Known(String),
    Unit,
    Unknown,
}

fn unify(a: Inferred, b: Inferred) -> Inferred {
    match (a, b) {
        (Inferred::Known(x), Inferred::Known(y)) if x == y => Inferred::Known(x),
        (Inferred::Unit, Inferred::Unit) => Inferred::Unit,
        _ => Inferred::Unknown,
    }
}

/// Best-effort result-type inference over a function body (§6.1: signatures
/// come "from typed Fn parameters, from inference against use, or from an
/// explicit record form"). Anything it cannot see is Unknown.
fn infer(arena: &Arena, id: NodeId, params: &HashMap<String, String>) -> Inferred {
    match arena.node(id) {
        Node::Bool(_) => Inferred::Known("bool".into()),
        Node::Int(_) => Inferred::Known("s64".into()),
        Node::Dec(_) => Inferred::Known("f64".into()),
        Node::Char(_) => Inferred::Known("char".into()),
        Node::Str(_) => Inferred::Known("string".into()),
        Node::Sym(name) => match params.get(name) {
            Some(t) => Inferred::Known(t.clone()),
            None => Inferred::Unknown,
        },
        Node::Call(head, payload) => {
            let Node::Sym(name) = arena.node(*head) else {
                return Inferred::Unknown;
            };
            match name.as_str() {
                "eq" | "lt" | "le" | "gt" | "ge" | "not" | "empty" | "contains" => {
                    Inferred::Known("bool".into())
                }
                "str-cat" | "upper" | "lower" | "join" | "to-string" => {
                    Inferred::Known("string".into())
                }
                "len" => Inferred::Known("s64".into()),
                "add" | "sub" | "mul" | "div" | "rem" | "neg" | "min" | "max" | "abs" => {
                    match arena.node(*payload) {
                        Node::Lst(items) | Node::Tup(items) => {
                            let any_dec = items.iter().any(|&i| {
                                matches!(infer(arena, i, params), Inferred::Known(t) if t == "f64")
                            });
                            Inferred::Known(if any_dec { "f64" } else { "s64" }.into())
                        }
                        _ => Inferred::Known("s64".into()),
                    }
                }
                "print" | "println" => Inferred::Unit,
                "if-MACRO" => match arena.node(*payload) {
                    Node::Tup(items) if items.len() == 3 => unify(
                        infer(arena, items[1], params),
                        infer(arena, items[2], params),
                    ),
                    _ => Inferred::Unknown,
                },
                "do-MACRO" => match arena.node(*payload) {
                    Node::Lst(items) => match items.last() {
                        Some(&last) => infer(arena, last, params),
                        None => Inferred::Unit,
                    },
                    _ => Inferred::Unknown,
                },
                "let-MACRO" => match arena.node(*payload) {
                    Node::Tup(items) if items.len() == 2 => {
                        let mut scope = params.clone();
                        if let Node::Rec(fields) = arena.node(items[0]) {
                            for (k, v) in fields {
                                if let Inferred::Known(t) = infer(arena, *v, &scope) {
                                    scope.insert(k.clone(), t);
                                }
                            }
                        }
                        infer(arena, items[1], &scope)
                    }
                    _ => Inferred::Unknown,
                },
                "the-MACRO" => match arena.node(*payload) {
                    Node::Tup(items) if items.len() == 2 => match type_text(arena, items[0]) {
                        Ok(t) => Inferred::Known(t),
                        Err(_) => Inferred::Unknown,
                    },
                    _ => Inferred::Unknown,
                },
                _ => Inferred::Unknown,
            }
        }
        _ => Inferred::Unknown,
    }
}
