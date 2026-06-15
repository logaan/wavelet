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
    /// `///` doc comments by defined name (Defs and DefTypes)
    pub docs: HashMap<String, String>,
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
    /// interface this export lands in — `api` unless grouped (§6.1)
    pub iface: String,
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
    let mut docs = HashMap::new();

    for &root in roots {
        let Node::Call(head, payload) = arena.node(root) else { continue };
        let Node::Sym(head_name) = arena.node(*head) else { continue };
        if let Some(d) = arena.doc(root) {
            if let Some(name) = defined_name(arena, *payload) {
                docs.insert(name, d.to_string());
            }
        }
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
            // a record form that only names/groups still gets an inferred sig
            Some(sig) if sig.params.is_empty() && sig.result.is_none() && defs.contains_key(&name) => {
                let (params_id, body) = defs[&name];
                let mut inferred = infer_sig(arena, &name, params_id, body, &defs)?;
                inferred.iface = sig.iface;
                inferred
            }
            Some(sig) => sig,
            None => {
                let (params_id, body) = defs
                    .get(&name)
                    .ok_or(format!("Export `{name}` has no definition"))?;
                infer_sig(arena, &name, *params_id, *body, &defs)?
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
        docs,
    })
}

/// The name a `Def`/`DefType` payload defines or an `Export` payload names.
fn defined_name(arena: &Arena, payload: NodeId) -> Option<String> {
    match arena.node(payload) {
        Node::Sym(name) => Some(name.clone()),
        Node::Tup(items) => match items.first().map(|&i| arena.node(i)) {
            Some(Node::Sym(name)) => Some(name.clone()),
            _ => None,
        },
        Node::Rec(fields) => fields.iter().find(|(k, _)| k == "name").and_then(|(_, v)| {
            match arena.node(*v) {
                Node::Sym(s) => Some(s.clone()),
                _ => None,
            }
        }),
        _ => None,
    }
}

/// Distinct export interfaces in first-appearance order; `api` is forced
/// first when type declarations exist (they always live in `api`).
pub fn iface_order(exports: &[FuncSig], has_types: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if has_types {
        out.push("api".to_string());
    }
    for sig in exports {
        if !out.contains(&sig.iface) {
            out.push(sig.iface.clone());
        }
    }
    out
}

/// `"demo:shout@0.1.0"` -> `"demo:shout"`
fn strip_version(s: &str) -> String {
    s.split('@').next().unwrap_or(s).to_string()
}

fn parse_explicit_sig(arena: &Arena, fields: &[(String, NodeId)]) -> Option<FuncSig> {
    let mut name = None;
    let mut iface = None;
    let mut params = Vec::new();
    let mut result = None;
    for (k, v) in fields {
        match (k.as_str(), arena.node(*v)) {
            ("name", Node::Sym(s)) => name = Some(s.clone()),
            ("iface", Node::Str(s)) => iface = Some(s.clone()),
            ("params", Node::Rec(pfields)) => {
                for (pk, pv) in pfields {
                    params.push((pk.clone(), type_text(arena, *pv).ok()?));
                }
            }
            ("result", _) => result = Some(type_text(arena, *v).ok()?),
            _ => {}
        }
    }
    Some(FuncSig {
        name: name?,
        iface: iface.unwrap_or_else(|| "api".to_string()),
        params,
        result,
    })
}

fn infer_sig(
    arena: &Arena,
    name: &str,
    params_id: NodeId,
    body: NodeId,
    defs: &HashMap<String, (NodeId, NodeId)>,
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
    let mut visiting = vec![name.to_string()];
    let result = match infer(arena, body, &param_types, defs, &mut visiting) {
        Inferred::Known(t) => Some(t),
        Inferred::Unit => None,
        Inferred::Unknown => {
            return Err(format!(
                "cannot infer result type of `{name}` (use the Export record form)"
            ));
        }
    };
    Ok(FuncSig { name: name.to_string(), iface: "api".to_string(), params, result })
}

/// Synthesize WIT text for a file (§6.1), as shown by `wavelet wit`.
pub fn synthesize(arena: &Arena, roots: &[NodeId]) -> Result<String, String> {
    let info = collect(arena, roots)?;
    synthesize_info(arena, &info, false)
}

/// Synthesize a fetch-ready world for a file: like [`synthesize`], but emitting
/// only the *host* (`wasi:*`) imports.
///
/// `wkg wit fetch` reads the world(s) in a `wit/` directory and tries to fetch
/// every referenced package from a registry. Build-set (sibling-`.wvl`) imports
/// have no registry — they are satisfied locally — so handing them to `wkg`
/// fails with "no registry configured". The host imports are exactly the ones
/// `wkg` *should* fetch into `wit/deps`, and their WIT text here is byte-for-byte
/// what [`synthesize`] emits, so the world `wkg` parses matches what the emitter
/// componentizes against. (Sibling packages are kind-(2) dependencies wired by
/// the build set / `wac`, not kind-(1) WIT fetched by `wkg`; see
/// `dev-notes/decouple-wasi.md`.)
pub fn synthesize_fetch_world(arena: &Arena, roots: &[NodeId]) -> Result<String, String> {
    let info = collect(arena, roots)?;
    synthesize_info(arena, &info, true)
}

/// Whether a file's synthesized world references any host (`wasi:*`) package,
/// i.e. whether it has anything for `wkg wit fetch` to pull into `wit/deps`.
pub fn has_host_deps(info: &FileInfo) -> bool {
    info.target.is_some()
        || info.imports.iter().any(|i| i.package.starts_with("wasi:"))
        || iface_order(&info.exports, !info.types.is_empty())
            .iter()
            .any(|i| is_external_iface(i))
}

/// The body shared by [`synthesize`] and [`synthesize_fetch_world`]. When
/// `host_only`, sibling (non-`wasi:`) imports are omitted so the result is a
/// world `wkg wit fetch` can resolve entirely from registries.
fn synthesize_info(arena: &Arena, info: &FileInfo, host_only: bool) -> Result<String, String> {
    let mut out = String::new();
    out.push_str(&format!("package {};\n", info.package));

    let doc_lines = |out: &mut String, name: &str| {
        if let Some(d) = info.docs.get(name) {
            for line in d.lines() {
                out.push_str(&format!("  /// {line}\n"));
            }
        }
    };
    let is_http = info.target.as_deref() == Some("wasi:http/proxy");
    let ifaces = iface_order(&info.exports, !info.types.is_empty());
    // External interfaces (e.g. wasi:http/incoming-handler) are defined by the
    // host's WIT; we only export them by name, never re-declare them.
    for iface in ifaces.iter().filter(|i| !is_external_iface(i)) {
        out.push_str(&format!("\ninterface {iface} {{\n"));
        if iface == "api" {
            for (name, ty) in &info.types {
                doc_lines(&mut out, name);
                out.push_str(&format!("  {}\n", type_decl(arena, name, *ty)?));
            }
        }
        for sig in info.exports.iter().filter(|s| &s.iface == iface) {
            doc_lines(&mut out, &sig.name);
            out.push_str(&format!("  {}\n", sig.to_wit()));
        }
        out.push_str("}\n");
    }

    out.push_str(&format!("\nworld {} {{\n", info.world));
    // wasi:http/proxy is realized by exporting the handler interface, not by
    // including the proxy world (which would pull in the whole proxy closure).
    if let Some(t) = &info.target {
        if host_only {
            // `wkg wit fetch` can't merge a world that `include`s a world whose
            // package it hasn't fetched yet (chicken-and-egg). Referencing one
            // concrete interface of the target package instead makes `wkg` pull
            // the whole package (and its transitive deps) into `wit/deps`. The
            // `wasi:cli/command` world's natural concrete reference is its
            // `wasi:cli/run` export. (This target translation is Step-2 glue
            // that retires with `Target` itself — see decouple-wasi-todo.md.)
            if t == "wasi:cli/command" {
                out.push_str("  export wasi:cli/run@0.2.0;\n");
            }
        } else if !is_http {
            out.push_str(&format!("  include {t};\n"));
        }
    }
    for imp in &info.imports {
        // Host (wasi:*) imports name an external, versioned interface; a
        // build-set dependency is imported by its bare path.
        if imp.package.starts_with("wasi:") {
            out.push_str(&format!("  import {};\n", external_versioned(&imp.path)));
        } else if !host_only {
            out.push_str(&format!("  import {};\n", imp.path));
        }
    }
    if is_http {
        out.push_str("  import wasi:io/streams@0.2.0;\n");
    }
    for iface in &ifaces {
        if is_external_iface(iface) {
            out.push_str(&format!("  export {};\n", external_versioned(iface)));
        } else if !host_only {
            out.push_str(&format!("  export {iface};\n"));
        }
    }
    out.push_str("}\n");
    Ok(out)
}

/// An interface that names an external WIT interface directly — it contains a
/// `:` (e.g. `wasi:http/incoming-handler`) — rather than a local one like `api`.
fn is_external_iface(iface: &str) -> bool {
    iface.contains(':')
}

/// Version an external interface path to the WASI version Wavelet vendors.
fn external_versioned(path: &str) -> String {
    format!("{path}@0.2.0")
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
        _ => Ok(format!("type {name} = {};", type_text(arena, ty)?)),
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
/// explicit record form"). Calls to other module-level defs are followed
/// (with a recursion guard). Anything it cannot see is Unknown.
fn infer(
    arena: &Arena,
    id: NodeId,
    params: &HashMap<String, String>,
    defs: &HashMap<String, (NodeId, NodeId)>,
    visiting: &mut Vec<String>,
) -> Inferred {
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
                                matches!(infer(arena, i, params, defs, visiting),
                                         Inferred::Known(t) if t == "f64")
                            });
                            Inferred::Known(if any_dec { "f64" } else { "s64" }.into())
                        }
                        _ => Inferred::Known("s64".into()),
                    }
                }
                "drop" | "cell-set" => Inferred::Unit,
                "if-MACRO" => match arena.node(*payload) {
                    Node::Tup(items) if items.len() == 3 => unify(
                        infer(arena, items[1], params, defs, visiting),
                        infer(arena, items[2], params, defs, visiting),
                    ),
                    _ => Inferred::Unknown,
                },
                "do-MACRO" => match arena.node(*payload) {
                    Node::Lst(items) => match items.last() {
                        Some(&last) => infer(arena, last, params, defs, visiting),
                        None => Inferred::Unit,
                    },
                    _ => Inferred::Unknown,
                },
                "let-MACRO" => match arena.node(*payload) {
                    Node::Tup(items) if items.len() == 2 => {
                        let mut scope = params.clone();
                        if let Node::Rec(fields) = arena.node(items[0]) {
                            for (k, v) in fields {
                                if let Inferred::Known(t) = infer(arena, *v, &scope, defs, visiting)
                                {
                                    scope.insert(k.clone(), t);
                                }
                            }
                        }
                        infer(arena, items[1], &scope, defs, visiting)
                    }
                    _ => Inferred::Unknown,
                },
                "match-MACRO" => match arena.node(*payload) {
                    // unify every clause's result type; pattern-bound names are
                    // left untyped (best effort, as elsewhere)
                    Node::Tup(items) if items.len() == 2 => match arena.node(items[1]) {
                        Node::Lst(clauses) => {
                            let mut acc: Option<Inferred> = None;
                            for &c in clauses {
                                if let Node::Tup(pair) = arena.node(c) {
                                    if pair.len() == 2 {
                                        let r = infer(arena, pair[1], params, defs, visiting);
                                        acc = Some(match acc {
                                            None => r,
                                            Some(prev) => unify(prev, r),
                                        });
                                    }
                                }
                            }
                            acc.unwrap_or(Inferred::Unknown)
                        }
                        _ => Inferred::Unknown,
                    },
                    _ => Inferred::Unknown,
                },
                "the-MACRO" => match arena.node(*payload) {
                    Node::Tup(items) if items.len() == 2 => match type_text(arena, items[0]) {
                        Ok(t) => Inferred::Known(t),
                        Err(_) => Inferred::Unknown,
                    },
                    _ => Inferred::Unknown,
                },
                _ => match defs.get(name) {
                    // follow a call to another module-level def
                    Some((params_id, body)) if !visiting.contains(name) => {
                        visiting.push(name.clone());
                        let mut callee_params = HashMap::new();
                        if let Node::Rec(fields) = arena.node(*params_id) {
                            for (k, v) in fields {
                                if let Ok(t) = type_text(arena, *v) {
                                    callee_params.insert(k.clone(), t);
                                }
                            }
                        }
                        let r = infer(arena, *body, &callee_params, defs, visiting);
                        visiting.pop();
                        r
                    }
                    _ => Inferred::Unknown,
                },
            }
        }
        _ => Inferred::Unknown,
    }
}
