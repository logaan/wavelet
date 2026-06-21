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
    pub imports: Vec<ImportInfo>,
    pub exports: Vec<FuncSig>,
    pub types: Vec<(String, NodeId)>,
    /// all module-level `Def name Fn …` definitions: name -> (params, body).
    /// Same-named defs collapse here (last wins); this map is what the emitter
    /// consumes. Use [`FileInfo::fn_defs`] to see every member of an overload
    /// set.
    pub defs: HashMap<String, (NodeId, NodeId)>,
    /// every module-level `Def name Fn …`, gathered per name in file order:
    /// name -> [(params, body), …]. A name with ≥2 entries is an overload set
    /// (Phase C); exported overload sets are name-mangled at the boundary.
    pub fn_defs: HashMap<String, Vec<(NodeId, NodeId)>>,
    /// non-function module-level defs, in file order: (name, expr)
    pub value_defs: Vec<(String, NodeId)>,
}

pub struct ImportInfo {
    /// interface path as written, version stripped, e.g. `demo:shout/api`
    pub path: String,
    /// package part, e.g. `demo:shout`
    pub package: String,
    pub alias: String,
    /// `Import {… macros: true}` — load this import's macro manifest at compile
    /// time (§6.3). A `macros: true` import is resolved to a `.wasm` macro
    /// component and instantiated at build time (Step 5,
    /// [`crate::macrodep`]); Steps 6–7 register its arities and route
    /// expansion. A pure `macros: true` import is *compile-time only* and does
    /// not contribute a runtime import to the synthesized world. Defaults to
    /// `false`, including for the bare-string import form.
    pub macros: bool,
    /// `Import {… macros: true from: "path/to/macros.wasm"}` — an explicit
    /// local path to the macro component to load for this import (Step 5). The
    /// path is resolved relative to the project root when relative. `None`
    /// falls back to the conventional `wit/macros/<ns>-<name>.wasm` location.
    /// Ignored for non-`macros` imports. An ordinary record field, so it needs
    /// no lexer/highlighting change.
    pub from: Option<String>,
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
    let mut imports = Vec::new();
    let mut export_decls: Vec<(String, Option<FuncSig>)> = Vec::new();
    let mut types = Vec::new();
    let mut defs = HashMap::new();
    let mut fn_defs: HashMap<String, Vec<(NodeId, NodeId)>> = HashMap::new();
    let mut value_defs = Vec::new();

    for &root in roots {
        // Top-level forms are tuples `Tup[head, …args]`. The arity-1 special
        // heads take their single payload from `items[1]`; Def/DefType take two.
        let Node::Tup(items) = arena.node(root) else { continue };
        let Some(&head) = items.first() else { continue };
        let Node::Sym(head_name) = arena.node(head) else { continue };
        match head_name.as_str() {
            "package-MACRO" => {
                if let Some(&p) = items.get(1) {
                    if let Node::Str(s) = arena.node(p) {
                        package = Some(s.clone());
                    }
                }
            }
            "import-MACRO" => {
                let Some(&p) = items.get(1) else { continue };
                let spec = match arena.node(p) {
                    Node::Str(s) => Some((s.clone(), None, false, None)),
                    Node::Rec(fields) => {
                        let mut pkg = None;
                        let mut alias = None;
                        let mut macros = false;
                        let mut from = None;
                        for (k, v) in fields {
                            match (k.as_str(), arena.node(*v)) {
                                ("pkg", Node::Str(s)) => pkg = Some(s.clone()),
                                ("as", Node::Sym(s)) => alias = Some(s.clone()),
                                ("macros", Node::Bool(b)) => macros = *b,
                                ("from", Node::Str(s)) => from = Some(s.clone()),
                                _ => {}
                            }
                        }
                        pkg.map(|p| (p, alias, macros, from))
                    }
                    _ => None,
                };
                let (pkg_str, alias, macros, from) = spec.ok_or("malformed Import")?;
                let path = strip_version(&pkg_str);
                let pkg_part = path.split('/').next().unwrap_or(&path).to_string();
                let alias = alias.unwrap_or_else(|| {
                    path.rsplit('/').next().unwrap_or(&path).to_string()
                });
                imports.push(ImportInfo { path, package: pkg_part, alias, macros, from });
            }
            "export-MACRO" => {
                let Some(&p) = items.get(1) else { continue };
                match arena.node(p) {
                    Node::Sym(s) => export_decls.push((s.clone(), None)),
                    Node::Rec(fields) => {
                        let sig = parse_explicit_sig(arena, fields).ok_or("malformed Export")?;
                        export_decls.push((sig.name.clone(), Some(sig)));
                    }
                    _ => return Err("malformed Export".into()),
                }
            }
            "deftype-MACRO" => {
                if items.len() >= 3 {
                    if let Node::Sym(name) = arena.node(items[1]) {
                        types.push((name.clone(), items[2]));
                    }
                }
            }
            "def-MACRO" => {
                if items.len() >= 3 {
                    if let Node::Sym(name) = arena.node(items[1]) {
                        // A function def binds an `Fn` form, which now reads as
                        // `Tup[fn-MACRO, params, body]`.
                        let mut is_fn = false;
                        if let Node::Tup(fn_items) = arena.node(items[2]) {
                            if fn_items.len() == 3
                                && matches!(arena.node(fn_items[0]), Node::Sym(s) if s == "fn-MACRO")
                            {
                                defs.insert(name.clone(), (fn_items[1], fn_items[2]));
                                fn_defs
                                    .entry(name.clone())
                                    .or_default()
                                    .push((fn_items[1], fn_items[2]));
                                is_fn = true;
                            }
                        }
                        if !is_fn {
                            value_defs.push((name.clone(), items[2]));
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
        // An exported *overload set* (≥2 same-named Fn defs, or a name that
        // collides with a builtin operation) has no single WIT signature: WIT
        // does not overload. Lower each member to its own concrete, mangled
        // function `name-<first-param-type>` (Phase C, Step 8). This applies
        // only when the Export does not supply an explicit signature override.
        let explicit_overrides = matches!(&explicit, Some(s) if !s.params.is_empty() || s.result.is_some());
        if !explicit_overrides && is_overload_export(&name, &fn_defs) {
            let members = fn_defs
                .get(&name)
                .ok_or(format!("Export `{name}` has no definition"))?;
            let iface = explicit.map(|s| s.iface).unwrap_or_else(|| "api".to_string());
            for &(params_id, body) in members {
                let mangled = mangle_name(arena, &name, params_id)?;
                let mut sig = infer_sig(arena, &mangled, params_id, body, &defs)?;
                sig.iface = iface.clone();
                exports.push(sig);
            }
            continue;
        }

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
        imports,
        exports,
        types,
        defs,
        fn_defs,
        value_defs,
    })
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

/// Whether an exported `name` denotes an overload set that must be name-mangled
/// at the boundary (Step 8). The trigger is either ≥2 module-level Fn defs of
/// `name`, **or** `name` colliding with a builtin operation (`builtins::NAMES`)
/// — a single `Def eq …` is still mangled because `eq` is an overloadable
/// operation name.
fn is_overload_export(name: &str, fn_defs: &HashMap<String, Vec<(NodeId, NodeId)>>) -> bool {
    let count = fn_defs.get(name).map(|v| v.len()).unwrap_or(0);
    count >= 2 || crate::builtins::NAMES.contains(&name)
}

/// The mangled WIT name for one overload-set member: `name-<type>` where
/// `<type>` is the WIT type name of the member's distinguishing (first)
/// parameter. `eq` with a `point` first parameter → `eq-point`.
fn mangle_name(arena: &Arena, name: &str, params_id: NodeId) -> Result<String, String> {
    let Node::Rec(fields) = arena.node(params_id) else {
        return Err(format!(
            "cannot mangle overloaded export `{name}`: its parameters must be \
             typed (a `{{a: t …}}` record) to distinguish overloads"
        ));
    };
    let Some((_k, ty)) = fields.first() else {
        return Err(format!(
            "cannot mangle overloaded export `{name}`: it takes no parameters, \
             so its overloads cannot be distinguished by argument type"
        ));
    };
    let ty_text = type_text(arena, *ty)?;
    Ok(format!("{name}-{ty_text}"))
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
            // Untyped parameters: infer each one's WIT type from how the body
            // uses it (§"Inference and literals"). A parameter used as a direct
            // argument to a builtin (or another def) whose parameter type at
            // that position is known takes that type; uses are unified.
            for n in names {
                let Some(t) = infer_param_from_use(arena, n, body, defs) else {
                    return Err(format!(
                        "cannot synthesize WIT for `{name}`: parameter `{n}` is untyped \
                         and its type cannot be inferred from use \
                         (annotate it or use the Export record form)"
                    ));
                };
                param_types.insert(n.clone(), t.clone());
                params.push((n.clone(), t));
            }
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

/// The WIT type a builtin imposes on its argument at position `pos`, if known.
/// Models at least the string-imposing builtins (§"Inference and literals"):
/// `upper`/`lower` and the `str-cat`/`split`/`join`/`contains` family all take
/// `string` arguments. Returns `None` when the builtin does not pin a concrete
/// argument type we can name in WIT.
fn builtin_param_type(callee: &str, pos: usize) -> Option<String> {
    match callee {
        // Every operand is a string.
        "upper" | "lower" | "str-cat" | "split" | "join" => Some("string".into()),
        // `contains(haystack needle)` — both string in the string overload.
        "contains" => Some("string".into()),
        _ => {
            let _ = pos;
            None
        }
    }
}

/// Infer an untyped parameter `pname`'s WIT type by scanning `body` for uses of
/// it as a *direct* argument to a call whose parameter type at that position is
/// known (a string-imposing builtin, or another module-level def with a typed
/// parameter). Types found across uses are unified; an inconsistent or absent
/// constraint yields `None`.
fn infer_param_from_use(
    arena: &Arena,
    pname: &str,
    body: NodeId,
    defs: &HashMap<String, (NodeId, NodeId)>,
) -> Option<String> {
    let mut found: Option<String> = None;
    let mut conflict = false;
    scan_param_uses(arena, pname, body, defs, &mut |t| match &found {
        _ if conflict => {}
        None => found = Some(t),
        Some(prev) if *prev == t => {}
        // Conflicting constraints across uses: not inferable.
        Some(_) => conflict = true,
    });
    if conflict { None } else { found }
}

/// Walk `id`, invoking `on_use(ty)` each time `pname` appears as a direct
/// argument to a call whose parameter type at that position is known.
fn scan_param_uses(
    arena: &Arena,
    pname: &str,
    id: NodeId,
    defs: &HashMap<String, (NodeId, NodeId)>,
    on_use: &mut dyn FnMut(String),
) {
    match arena.node(id) {
        Node::Tup(items) => {
            if let Some((&head, args)) = items.split_first()
                && let Node::Sym(callee) = arena.node(head)
            {
                for (pos, &arg) in args.iter().enumerate() {
                    // Is this argument the parameter used directly?
                    let is_param = matches!(arena.node(arg), Node::Sym(s) if s == pname);
                    if is_param
                        && let Some(t) = call_param_type(arena, callee, pos, defs)
                    {
                        on_use(t);
                    }
                }
            }
            // Recurse into every child to catch nested uses.
            for &child in items {
                scan_param_uses(arena, pname, child, defs, on_use);
            }
        }
        Node::Lst(items) => {
            for &child in items {
                scan_param_uses(arena, pname, child, defs, on_use);
            }
        }
        Node::Rec(fields) => {
            for (_k, v) in fields {
                scan_param_uses(arena, pname, *v, defs, on_use);
            }
        }
        _ => {}
    }
}

/// The known parameter type of `callee` at position `pos`: a string-imposing
/// builtin, or another module-level def with a typed parameter there.
fn call_param_type(
    arena: &Arena,
    callee: &str,
    pos: usize,
    defs: &HashMap<String, (NodeId, NodeId)>,
) -> Option<String> {
    if let Some(t) = builtin_param_type(callee, pos) {
        return Some(t);
    }
    // Follow a call to another module-level def: take that def's declared
    // parameter type at `pos`, if it is typed (a `Rec` params form).
    if let Some((params_id, _body)) = defs.get(callee)
        && let Node::Rec(fields) = arena.node(*params_id)
        && let Some((_k, v)) = fields.get(pos)
        && let Ok(t) = type_text(arena, *v)
    {
        return Some(t);
    }
    None
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

/// Whether an import is a *pure macro* import: `macros: true`, used only at
/// compile time (§6.3). Such an import is resolved to a macro component and run
/// during expand ([`crate::macrodep`]); it contributes **no runtime import** to
/// the consumer component, so it is excluded from world synthesis and from
/// sibling-edge composition.
///
/// The common case is that `macros: true` means macro-only. An import that is
/// *both* a runtime dependency and a macro library is an unsupported edge case
/// for now: it would need two surfaces (a runtime import in the world plus a
/// compile-time macro instance). That is noted as a follow-up; until then
/// `macros: true` is treated as macro-only here.
pub fn is_macro_only(imp: &ImportInfo) -> bool {
    imp.macros
}

/// Whether a file's synthesized world references any host (`wasi:*`) package,
/// i.e. whether it has anything for `wkg wit fetch` to pull into `wit/deps`.
/// Pure macro imports never reach a registry, so they are not counted.
pub fn has_host_deps(info: &FileInfo) -> bool {
    info.imports
        .iter()
        .any(|i| !is_macro_only(i) && i.package.starts_with("wasi:"))
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

    let ifaces = iface_order(&info.exports, !info.types.is_empty());
    // External interfaces (e.g. wasi:http/incoming-handler) are defined by the
    // host's WIT; we only export them by name, never re-declare them.
    for iface in ifaces.iter().filter(|i| !is_external_iface(i)) {
        out.push_str(&format!("\ninterface {iface} {{\n"));
        if iface == "api" {
            for (name, ty) in &info.types {
                out.push_str(&format!("  {}\n", type_decl(arena, name, *ty)?));
            }
        }
        for sig in info.exports.iter().filter(|s| &s.iface == iface) {
            out.push_str(&format!("  {}\n", sig.to_wit()));
        }
        out.push_str("}\n");
    }

    out.push_str(&format!("\nworld {} {{\n", info.world));
    for imp in &info.imports {
        // A pure macro import (§6.3) is a compile-time-only dependency resolved
        // during expand; it is not a runtime import of this component, so it
        // never appears in the synthesized world.
        if is_macro_only(imp) {
            continue;
        }
        // Host (wasi:*) imports name an external, versioned interface; a
        // build-set dependency is imported by its bare path.
        if imp.package.starts_with("wasi:") {
            out.push_str(&format!("  import {};\n", external_versioned(&imp.path)));
        } else if !host_only {
            out.push_str(&format!("  import {};\n", imp.path));
        }
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
                    // A payloaded case like `days(30)` reads as `Tup[days, 30]`.
                    Node::Tup(case_items) => {
                        let Some((&h, payload)) = case_items.split_first() else {
                            return Err(format!("bad variant case in `{name}`"));
                        };
                        let Node::Sym(case) = arena.node(h) else {
                            return Err(format!("bad variant case in `{name}`"));
                        };
                        let payload_text: Vec<String> = payload
                            .iter()
                            .map(|&i| type_text(arena, i))
                            .collect::<Result<_, _>>()?;
                        parts.push(format!("{case}({})", payload_text.join(", ")));
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
/// `result(t e)` -> `result<t, e>`, `tuple(a b)` -> `tuple<a, b>`. A bare type
/// name is a `Sym`; an applied constructor is a `Tup[ctor, arg…]`.
pub fn type_text(arena: &Arena, id: NodeId) -> Result<String, String> {
    match arena.node(id) {
        Node::Sym(s) => Ok(s.clone()),
        Node::Tup(items) => {
            let Some((&head, args)) = items.split_first() else {
                return Err("bad type form".into());
            };
            let Node::Sym(ctor) = arena.node(head) else {
                return Err("bad type form".into());
            };
            let args: Vec<String> = args
                .iter()
                .map(|&i| type_text(arena, i))
                .collect::<Result<_, _>>()?;
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
        Node::Tup(items) => {
            let Some((&head, args)) = items.split_first() else {
                return Inferred::Unknown;
            };
            let Node::Sym(name) = arena.node(head) else {
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
                // Sequence builtins whose monomorphic result type is the type of
                // their (single) sequence argument: `reverse(xs)` and `tail(xs)`
                // have the same type as `xs` (§"Inference and literals").
                "reverse" | "tail" if args.len() == 1 => {
                    infer(arena, args[0], params, defs, visiting)
                }
                "add" | "sub" | "mul" | "div" | "rem" | "neg" | "min" | "max" | "abs" => {
                    let any_dec = args.iter().any(|&i| {
                        matches!(infer(arena, i, params, defs, visiting),
                                 Inferred::Known(t) if t == "f64")
                    });
                    Inferred::Known(if any_dec { "f64" } else { "s64" }.into())
                }
                "drop" | "cell-set" => Inferred::Unit,
                "if-MACRO" if args.len() == 3 => unify(
                    infer(arena, args[1], params, defs, visiting),
                    infer(arena, args[2], params, defs, visiting),
                ),
                "do-MACRO" if args.len() == 1 => match arena.node(args[0]) {
                    Node::Lst(items) => match items.last() {
                        Some(&last) => infer(arena, last, params, defs, visiting),
                        None => Inferred::Unit,
                    },
                    _ => Inferred::Unknown,
                },
                "let-MACRO" if args.len() == 2 => {
                    let mut scope = params.clone();
                    if let Node::Rec(fields) = arena.node(args[0]) {
                        for (k, v) in fields {
                            if let Inferred::Known(t) = infer(arena, *v, &scope, defs, visiting) {
                                scope.insert(k.clone(), t);
                            }
                        }
                    }
                    infer(arena, args[1], &scope, defs, visiting)
                }
                // unify every clause's result type; pattern-bound names are
                // left untyped (best effort, as elsewhere)
                "match-MACRO" if args.len() == 2 => match arena.node(args[1]) {
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
                "the-MACRO" if args.len() == 2 => match type_text(arena, args[0]) {
                    Ok(t) => Inferred::Known(t),
                    Err(_) => Inferred::Unknown,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::read_file;

    fn collect_src(src: &str) -> FileInfo {
        let (arena, roots) = read_file(src).expect("read");
        collect(&arena, &roots).expect("collect")
    }

    fn import_named<'a>(info: &'a FileInfo, alias: &str) -> &'a ImportInfo {
        info.imports
            .iter()
            .find(|i| i.alias == alias)
            .unwrap_or_else(|| panic!("no import aliased `{alias}`"))
    }

    #[test]
    fn import_macros_flag_true() {
        let info = collect_src(
            "Package \"demo:app@0.1.0\"\n\
             Import {pkg: \"acme:html/dsl\" macros: true}\n",
        );
        let imp = import_named(&info, "dsl");
        assert!(imp.macros, "`macros: true` should set ImportInfo.macros");
    }

    #[test]
    fn import_macros_flag_defaults_false() {
        // Record form without `macros:`.
        let info = collect_src(
            "Package \"demo:app@0.1.0\"\n\
             Import {pkg: \"acme:html/dsl\" as: html}\n",
        );
        assert!(
            !import_named(&info, "html").macros,
            "omitting `macros:` should default to false"
        );

        // `macros: false` written explicitly.
        let info = collect_src(
            "Package \"demo:app@0.1.0\"\n\
             Import {pkg: \"acme:html/dsl\" macros: false}\n",
        );
        assert!(!import_named(&info, "dsl").macros);
    }

    #[test]
    fn import_bare_string_form_defaults_false() {
        let info = collect_src(
            "Package \"demo:app@0.1.0\"\n\
             Import \"acme:html/dsl\"\n",
        );
        assert!(
            !import_named(&info, "dsl").macros,
            "bare-string Import should yield macros: false"
        );
    }

    #[test]
    fn import_from_field_is_parsed() {
        let info = collect_src(
            "Package \"demo:app@0.1.0\"\n\
             Import {pkg: \"acme:html/dsl\" macros: true from: \"build/dsl.wasm\"}\n",
        );
        let imp = import_named(&info, "dsl");
        assert!(imp.macros);
        assert_eq!(imp.from.as_deref(), Some("build/dsl.wasm"));
    }

    #[test]
    fn import_from_defaults_none() {
        let info = collect_src(
            "Package \"demo:app@0.1.0\"\n\
             Import {pkg: \"acme:html/dsl\" macros: true}\n",
        );
        assert_eq!(import_named(&info, "dsl").from, None);
    }

    #[test]
    fn macro_only_import_absent_from_synthesized_world() {
        // A file that imports a macro library but uses none of its runtime
        // exports must synthesize a world with NO `import` for it.
        let src = "Package \"demo:app@0.1.0\"\n\
             Import {pkg: \"acme:html/dsl\" macros: true}\n\
             Export greet\n\
             Def greet Fn {} \"hi\"\n";
        let (arena, roots) = read_file(src).expect("read");
        let world = synthesize(&arena, &roots).expect("synthesize");
        assert!(
            !world.contains("import acme:html/dsl"),
            "macro-only import leaked into the world:\n{world}"
        );
        // A non-macro import, by contrast, is still emitted.
        let src2 = "Package \"demo:app@0.1.0\"\n\
             Import \"acme:html/dsl\"\n\
             Export greet\n\
             Def greet Fn {} \"hi\"\n";
        let (arena, roots) = read_file(src2).expect("read");
        let world = synthesize(&arena, &roots).expect("synthesize");
        assert!(
            world.contains("import acme:html/dsl"),
            "non-macro import should appear in the world:\n{world}"
        );
    }
}
