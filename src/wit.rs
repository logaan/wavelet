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
    /// Compile-time functor instantiations: an `Instantiate {pkg: … with:
    /// {elem: T} as: alias}` (4.6.1) is not a runtime import but a request to
    /// stamp out a monomorphic component specialized at element type `T`
    /// (Steps 10–11). Recorded here instead of in `imports`, so synthesis
    /// emits a specialized interface rather than an `import` of the
    /// (non-existent) functor package.
    pub functors: Vec<FunctorInst>,
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
    /// For every member of an *exported* overload set, the mangled WIT name
    /// (`eq-point`, `eq-string`, …) it was synthesized into, mapped to the exact
    /// `(params, body)` NodeIds it came from. The underlying `Def` is named `eq`,
    /// so `info.defs`/`fn_defs` are keyed by the original name and the emitter
    /// would otherwise have no function to back the mangled export. The emitter
    /// keys on this identity to register and emit one internal function per
    /// member. NodeIds are valid only for the local arena `emit` walks (the build
    /// path), which is exactly where this is consumed — it is *not* preserved
    /// across cross-package `Dep` clones.
    pub overload_bodies: HashMap<String, (NodeId, NodeId)>,
    /// User-declared resource types (4.5), in file order. Each carries the
    /// synthesized WIT signatures of its constructor/methods/statics and the
    /// interface it is exported into (`None` = internal, not exported).
    pub resources: Vec<ResourceDecl>,
}

/// A synthesized `DefResource` (4.5): the WIT `resource { … }` block plus where
/// it is exported. Methods carry their signature with the implicit `self`
/// parameter already dropped (WIT methods take `self` implicitly).
#[derive(Clone)]
pub struct ResourceDecl {
    pub name: String,
    /// The interface this resource is exported into, or `None` if internal.
    pub iface: Option<String>,
    /// The constructor's parameters (`New`), or `None` if the resource declares
    /// no `New` member. WIT constructors are anonymous and have no result.
    pub constructor: Option<Vec<(String, String)>>,
    /// Instance methods, `self` already removed from each signature.
    pub methods: Vec<FuncSig>,
    /// Static members (`Static Fn`), including any that return the resource.
    pub statics: Vec<FuncSig>,
    /// Whether the resource declares a `Drop` destructor (rendered implicitly by
    /// WIT as `[dtor]` at the boundary; no source line in the resource block).
    pub has_drop: bool,
}

impl ResourceDecl {
    /// Render the WIT `resource <name> { … }` block (4.5). The constructor is
    /// anonymous; methods take `self` implicitly; statics use `static func`.
    pub fn to_wit(&self) -> String {
        let mut out = format!("  resource {} {{
", self.name);
        if let Some(params) = &self.constructor {
            let ps: Vec<String> = params.iter().map(|(n, t)| format!("{n}: {t}")).collect();
            out.push_str(&format!("    constructor({});
", ps.join(", ")));
        }
        for m in &self.methods {
            let ps: Vec<String> = m.params.iter().map(|(n, t)| format!("{n}: {t}")).collect();
            match &m.result {
                Some(r) => out.push_str(&format!("    {}: func({}) -> {r};
", m.name, ps.join(", "))),
                None => out.push_str(&format!("    {}: func({});
", m.name, ps.join(", "))),
            }
        }
        for st in &self.statics {
            let ps: Vec<String> = st.params.iter().map(|(n, t)| format!("{n}: {t}")).collect();
            match &st.result {
                Some(r) => {
                    out.push_str(&format!("    {}: static func({}) -> {r};
", st.name, ps.join(", ")))
                }
                None => out.push_str(&format!("    {}: static func({});
", st.name, ps.join(", "))),
            }
        }
        out.push_str("  }
");
        out
    }
}

/// Peel a `Static` marker off a resource member value in the WIT collector:
/// `Tup[static-MACRO, <fn>]` → `(true, <fn>)`; anything else → `(false, value)`.
fn strip_static_member(arena: &Arena, val: NodeId) -> (bool, NodeId) {
    if let Node::Tup(items) = arena.node(val)
        && items.len() == 2
        && matches!(arena.node(items[0]), Node::Sym(s) if s == "static-MACRO")
    {
        return (true, items[1]);
    }
    (false, val)
}

/// If `id` is `Fn {params} body`, return `(params_form, body_form)`.
fn as_fn_form(arena: &Arena, id: NodeId) -> Option<(NodeId, NodeId)> {
    let Node::Tup(items) = arena.node(id) else {
        return None;
    };
    if items.len() != 3 {
        return None;
    }
    if !matches!(arena.node(items[0]), Node::Sym(s) if s == "fn-MACRO") {
        return None;
    }
    Some((items[1], items[2]))
}

/// Parse an annotated parameter record into `(name, wit-type-text)` pairs (used
/// for a constructor's parameters, whose result WIT never needs inferring).
fn annotated_params(arena: &Arena, params_id: NodeId) -> Result<Vec<(String, String)>, String> {
    match arena.node(params_id) {
        Node::Flg(names) if names.is_empty() => Ok(Vec::new()),
        Node::Rec(fields) => fields
            .iter()
            .map(|(k, v)| Ok((k.clone(), type_text(arena, *v)?)))
            .collect(),
        _ => Err("a resource constructor's parameters must be annotated `{name: type …}`".into()),
    }
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

/// Which built-in functor template an instantiation requests, identified from the
/// import's `pkg`. The compiler knows these templates intrinsically — there is no
/// real `wavelet:coll/*` component in the tree.
#[derive(Clone, PartialEq, Eq)]
pub enum FunctorKind {
    /// `wavelet:coll/set` — a set parameterized over its element type.
    Set,
}

/// One `Instantiate {pkg: … with: {elem: T} as: alias}` functor instantiation
/// (Steps 10–11, spelling per 4.6.1).
pub struct FunctorInst {
    pub kind: FunctorKind,
    /// the `as:` alias used to qualify the functor's ops (`pts/new`, `pts/add`, …)
    pub alias: String,
    /// the WIT type text of the element type, e.g. `point`, `string`, `s32`
    pub elem: String,
    /// the specialized interface name, e.g. `point-set`
    pub iface: String,
}

/// The result type of one `Set` functor operation, abstracted away from the
/// element type so a single table describes the template.
enum SetOpResult {
    /// the set handle, the resource type `<iface>.set`
    Handle,
    /// a fixed Known WIT type, e.g. `bool` or `u32`
    Ty(&'static str),
    /// unit — a discarding op like `add`
    Unit,
}

/// One operation of the `Set` functor template. Single source of truth for both
/// the inference op-table (`functor_op_table`) and the emitted WIT resource
/// (`functor_interface`), so the two cannot drift.
struct SetOp {
    name: &'static str,
    takes_value: bool,
    result: SetOpResult,
}

/// The `Set` functor's operations, in interface order (constructor first).
const SET_OPS: &[SetOp] = &[
    SetOp {
        name: "new",
        takes_value: false,
        result: SetOpResult::Handle,
    },
    SetOp {
        name: "add",
        takes_value: true,
        result: SetOpResult::Unit,
    },
    SetOp {
        name: "contains",
        takes_value: true,
        result: SetOpResult::Ty("bool"),
    },
    SetOp {
        name: "size",
        takes_value: false,
        result: SetOpResult::Ty("u32"),
    },
];

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
        let ps: Vec<String> = self
            .params
            .iter()
            .map(|(n, t)| format!("{n}: {t}"))
            .collect();
        match &self.result {
            Some(r) => format!("{}: func({}) -> {r};", self.name, ps.join(", ")),
            None => format!("{}: func({});", self.name, ps.join(", ")),
        }
    }
}

pub fn collect(arena: &Arena, roots: &[NodeId]) -> Result<FileInfo, String> {
    let mut package = None;
    let mut imports = Vec::new();
    let mut functors: Vec<FunctorInst> = Vec::new();
    let mut export_decls: Vec<(String, Option<FuncSig>)> = Vec::new();
    let mut types = Vec::new();
    let mut defs = HashMap::new();
    let mut fn_defs: HashMap<String, Vec<(NodeId, NodeId)>> = HashMap::new();
    let mut value_defs = Vec::new();
    let mut overload_bodies: HashMap<String, (NodeId, NodeId)> = HashMap::new();
    // Raw DefResource members, gathered in the root loop and turned into
    // `ResourceDecl`s once `defs`/`functor_ops` exist for member inference (4.5).
    let mut resource_forms: Vec<(String, Vec<(String, NodeId)>)> = Vec::new();

    for &root in roots {
        // Top-level forms are tuples `Tup[head, …args]`. The arity-1 special
        // heads take their single payload from `items[1]`; Def/DefType take two.
        let Node::Tup(items) = arena.node(root) else {
            continue;
        };
        let Some(&head) = items.first() else { continue };
        let Node::Sym(head_name) = arena.node(head) else {
            continue;
        };
        match head_name.as_str() {
            "package-MACRO" => {
                if let Some(&p) = items.get(1)
                    && let Node::Str(s) = arena.node(p)
                {
                    package = Some(s.clone());
                }
            }
            // `Instantiate {pkg: … with: {elem: t} as: alias}` applies a
            // compile-time functor (4.6, decision 4.6.1 option 1): stamp out a
            // monomorphic interface specialized at the `with:` arguments. It is
            // recorded in `functors` and never reaches the ordinary `imports`
            // list, so synthesis emits the specialized interface rather than an
            // `import <functor pkg>;`.
            "instantiate-MACRO" => {
                let Some(&p) = items.get(1) else { continue };
                let Node::Rec(fields) = arena.node(p) else {
                    return Err("malformed Instantiate (expects a record)".into());
                };
                functors.push(parse_instantiate(arena, fields)?);
            }
            "import-MACRO" => {
                let Some(&p) = items.get(1) else { continue };
                // `Import` brings an unparameterized dependency into scope;
                // functor application moved to its own `Instantiate` form
                // (4.6.1). Importing a functor package is an actionable error.
                if let Node::Rec(fields) = arena.node(p)
                    && let Some(pkg) = record_pkg(arena, fields)
                    && is_functor_pkg(&pkg)
                {
                    return Err(format!(
                        "`{}` is a functor package: apply it with \
                         `Instantiate {{pkg: … with: {{elem: t}} as: alias}}`, \
                         not `Import`",
                        strip_version(&pkg)
                    ));
                }
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
                let alias =
                    alias.unwrap_or_else(|| path.rsplit('/').next().unwrap_or(&path).to_string());
                imports.push(ImportInfo {
                    path,
                    package: pkg_part,
                    alias,
                    macros,
                    from,
                });
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
                if items.len() >= 3
                    && let Node::Sym(name) = arena.node(items[1])
                {
                    types.push((name.clone(), items[2]));
                }
            }
            "defresource-MACRO" => {
                if items.len() >= 3
                    && let Node::Sym(name) = arena.node(items[1])
                    && let Node::Rec(members) = arena.node(items[2])
                {
                    resource_forms.push((name.clone(), members.clone()));
                }
            }
            "def-MACRO" => {
                if items.len() >= 3
                    && let Node::Sym(name) = arena.node(items[1])
                {
                    // A function def binds an `Fn` form, which now reads as
                    // `Tup[fn-MACRO, params, body]`.
                    let mut is_fn = false;
                    if let Node::Tup(fn_items) = arena.node(items[2])
                        && fn_items.len() == 3
                        && matches!(arena.node(fn_items[0]), Node::Sym(s) if s == "fn-MACRO")
                    {
                        defs.insert(name.clone(), (fn_items[1], fn_items[2]));
                        fn_defs
                            .entry(name.clone())
                            .or_default()
                            .push((fn_items[1], fn_items[2]));
                        is_fn = true;
                    }
                    if !is_fn {
                        value_defs.push((name.clone(), items[2]));
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

    // The result types of every functor op, keyed by `(alias, op)`, so a
    // qualified `alias/op(...)` call in an export body infers concretely (Steps
    // 10–11). `Some(t)` is a Known WIT type; `None` is unit (`alias/add`).
    let functor_ops = functor_op_table(&functors);

    // Turn each DefResource into a `ResourceDecl` with WIT signatures (4.5).
    // The constructor contributes its (annotated) parameters only — its body
    // returns the rep, which is anonymous in WIT. Methods and statics infer
    // their result like any export; a method's implicit `self` is dropped.
    let resource_names: std::collections::HashSet<String> =
        resource_forms.iter().map(|(n, _)| n.clone()).collect();
    let mut resources: Vec<ResourceDecl> = Vec::new();
    for (rname, members) in &resource_forms {
        let mut constructor = None;
        let mut methods = Vec::new();
        let mut statics = Vec::new();
        let mut has_drop = false;
        for (key, val) in members {
            let (is_static, fn_id) = strip_static_member(arena, *val);
            let (params_id, body) = as_fn_form(arena, fn_id).ok_or(format!(
                "DefResource `{rname}`: member `{key}` must be an `Fn`"
            ))?;
            match key.as_str() {
                "New" => constructor = Some(annotated_params(arena, params_id)?),
                "Drop" => has_drop = true,
                other => {
                    let mut sig =
                        infer_sig(arena, roots, other, params_id, body, &defs, &functor_ops)?;
                    sig.iface = String::new();
                    if is_static {
                        statics.push(sig);
                    } else {
                        // WIT methods take `self` implicitly: drop the first param.
                        if !sig.params.is_empty() {
                            sig.params.remove(0);
                        }
                        methods.push(sig);
                    }
                }
            }
        }
        resources.push(ResourceDecl {
            name: rname.clone(),
            iface: None,
            constructor,
            methods,
            statics,
            has_drop,
        });
    }

    // De-duplicate identical export declarations before lowering (§6 review
    // fix). `Derive` auto-emits a bare `Export {op}-{tname}` for each derived op
    // (`expand.rs`), so an author who also writes that same bare `Export eq-point`
    // explicitly would declare it twice — and `synthesize` would then emit two
    // identical `eq-point: func(...)` lines (invalid WIT). Collapse runs that are
    // *genuinely identical* (same exported name and same explicit signature, if
    // any); keep distinct declarations that merely share a name but differ in
    // `iface:`/signature, since those are not the case this fix targets.
    let export_decls = dedup_export_decls(export_decls);

    let mut exports = Vec::new();
    for (name, explicit) in export_decls {
        // Exporting a resource exports the whole `resource { … }` block, not a
        // function (4.5). Record its placement interface and skip the
        // function-export machinery below.
        if resource_names.contains(&name) {
            let iface = explicit
                .map(|s| s.iface)
                .unwrap_or_else(|| "api".to_string());
            if let Some(r) = resources.iter_mut().find(|r| r.name == name) {
                r.iface = Some(iface);
            }
            continue;
        }
        // An exported *overload set* (≥2 same-named Fn defs, or a name that
        // collides with a builtin operation) has no single WIT signature: WIT
        // does not overload. Lower each member to its own concrete, mangled
        // function `name-<first-param-type>` (Phase C, Step 8). This applies
        // only when the Export does not supply an explicit signature override.
        let explicit_overrides =
            matches!(&explicit, Some(s) if !s.params.is_empty() || s.result.is_some());
        if !explicit_overrides && is_overload_export(&name, &fn_defs) {
            let members = fn_defs
                .get(&name)
                .ok_or(format!("Export `{name}` has no definition"))?;
            let iface = explicit
                .map(|s| s.iface)
                .unwrap_or_else(|| "api".to_string());

            // First-parameter labels disambiguate the set only if they are all
            // distinct (preserving the established `eq-point` / `eq-string`
            // scheme). If two members collide on the first parameter, fall back
            // to mangling over *all* parameter types for the whole set so the
            // labels stay mutually consistent. If even the full-signature labels
            // collide, two members have byte-identical parameter type lists — a
            // genuine duplicate that WIT cannot represent — so report it.
            let first_labels = members
                .iter()
                .map(|&(params_id, _)| mangle_name(arena, &name, params_id))
                .collect::<Result<Vec<_>, _>>()?;
            let first_distinct = {
                let unique: std::collections::HashSet<&String> = first_labels.iter().collect();
                unique.len() == first_labels.len()
            };
            let labels = if first_distinct {
                first_labels
            } else {
                let full_labels = members
                    .iter()
                    .map(|&(params_id, _)| mangle_name_full(arena, &name, params_id))
                    .collect::<Result<Vec<_>, _>>()?;
                let unique: std::collections::HashSet<&String> = full_labels.iter().collect();
                if unique.len() != full_labels.len() {
                    return Err(format!(
                        "exported overload set `{name}` has two members with identical \
                         parameter types; they mangle to the same WIT name and cannot \
                         both be exported (remove the duplicate definition)"
                    ));
                }
                full_labels
            };

            for (&(params_id, body), mangled) in members.iter().zip(labels) {
                let mut sig =
                    infer_sig(arena, roots, &mangled, params_id, body, &defs, &functor_ops)?;
                sig.iface = iface.clone();
                // Record the identity backing this mangled export so the emitter
                // can register a concrete internal function for it (the `Def` is
                // named `name`, not `mangled`, so `defs`/`fn_defs` cannot back it).
                overload_bodies.insert(sig.name.clone(), (params_id, body));
                exports.push(sig);
            }
            continue;
        }

        let sig = match explicit {
            // a record form that only names/groups still gets an inferred sig
            Some(sig)
                if sig.params.is_empty() && sig.result.is_none() && defs.contains_key(&name) =>
            {
                let (params_id, body) = defs[&name];
                let mut inferred =
                    infer_sig(arena, roots, &name, params_id, body, &defs, &functor_ops)?;
                inferred.iface = sig.iface;
                inferred
            }
            Some(sig) => sig,
            None => {
                let (params_id, body) = defs
                    .get(&name)
                    .ok_or(format!("Export `{name}` has no definition"))?;
                infer_sig(arena, roots, &name, *params_id, *body, &defs, &functor_ops)?
            }
        };
        exports.push(sig);
    }

    Ok(FileInfo {
        package,
        package_path,
        world,
        imports,
        functors,
        exports,
        types,
        defs,
        fn_defs,
        value_defs,
        overload_bodies,
        resources,
    })
}

/// Drop later export declarations that are byte-for-byte identical to one
/// already seen, preserving first-appearance order (§6 review fix). Two
/// declarations are "identical" iff they name the same export *and* carry the
/// same explicit signature payload (or both carry none). Declarations that share
/// a name but differ in their explicit signature (`iface:`, params, result) are
/// kept — those are not duplicates this fix collapses.
fn dedup_export_decls(decls: Vec<(String, Option<FuncSig>)>) -> Vec<(String, Option<FuncSig>)> {
    let mut out: Vec<(String, Option<FuncSig>)> = Vec::with_capacity(decls.len());
    for decl in decls {
        let dup = out
            .iter()
            .any(|seen| seen.0 == decl.0 && opt_sig_eq(&seen.1, &decl.1));
        if !dup {
            out.push(decl);
        }
    }
    out
}

/// Structural equality of two optional explicit export signatures. `FuncSig`
/// does not derive `PartialEq` (its `NodeId`-free fields are compared by value),
/// so compare the carried strings directly.
fn opt_sig_eq(a: &Option<FuncSig>, b: &Option<FuncSig>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            x.name == y.name && x.iface == y.iface && x.params == y.params && x.result == y.result
        }
        _ => false,
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

/// The `pkg:` field of an `Import`/`Instantiate` record, if present.
fn record_pkg(arena: &Arena, fields: &[(String, NodeId)]) -> Option<String> {
    fields
        .iter()
        .find_map(|(k, v)| match (k.as_str(), arena.node(*v)) {
            ("pkg", Node::Str(s)) => Some(s.clone()),
            _ => None,
        })
}

/// Whether a package id names a compile-time functor the compiler knows
/// intrinsically (currently only `wavelet:coll/set`).
fn is_functor_pkg(pkg: &str) -> bool {
    strip_version(pkg).ends_with("coll/set")
}

/// Parse an `Instantiate {pkg: … with: {…} as: alias}` record (4.6, decision
/// 4.6.1): `pkg:` must name a known functor package, and the functor's
/// parameters are passed *by name* in the `with:` record — for `Set`, the
/// single parameter `elem:`. The `as:` alias defaults to the trailing path
/// segment, as for ordinary imports.
fn parse_instantiate(arena: &Arena, fields: &[(String, NodeId)]) -> Result<FunctorInst, String> {
    let pkg = record_pkg(arena, fields).ok_or("Instantiate is missing `pkg:`")?;
    let path = strip_version(&pkg);
    if !is_functor_pkg(&pkg) {
        return Err(format!(
            "`{path}` is not a functor package (known functors: wavelet:coll/set)"
        ));
    }
    let kind = FunctorKind::Set;
    let mut alias = None;
    let mut with: Vec<(String, NodeId)> = Vec::new();
    for (k, v) in fields {
        match (k.as_str(), arena.node(*v)) {
            ("as", Node::Sym(s)) => alias = Some(s.clone()),
            ("with", Node::Rec(args)) => with = args.clone(),
            _ => {}
        }
    }
    // Arguments bind the functor's parameters by name; `Set` takes `elem`.
    let mut elem = None;
    for (k, v) in &with {
        match k.as_str() {
            "elem" => elem = Some(type_text(arena, *v)?),
            other => {
                return Err(format!(
                    "functor `{path}` has no parameter `{other}` (expected `elem:`)"
                ));
            }
        }
    }
    let elem =
        elem.ok_or_else(|| format!("Instantiate of `{path}` is missing `with: {{elem: …}}`"))?;
    let alias = alias.unwrap_or_else(|| path.rsplit('/').next().unwrap_or(&path).to_string());
    let iface = format!("{elem}-set");
    Ok(FunctorInst {
        kind,
        alias,
        elem,
        iface,
    })
}

/// The result type of one functor op: `Some(t)` is a Known WIT type, `None` is
/// unit (a discarding op like `set/add`).
type FunctorOps = HashMap<(String, String), Option<String>>;

/// Build the `(alias, op) -> result` table for every functor instantiation so
/// qualified calls in export bodies infer concretely (Steps 10–11).
fn functor_op_table(functors: &[FunctorInst]) -> FunctorOps {
    let mut table: FunctorOps = HashMap::new();
    for f in functors {
        match f.kind {
            FunctorKind::Set => {
                // The set handle is the resource type `<iface>.set` (WIT's
                // dotted interface-member syntax, as in fig-wit). Its exact text
                // is unasserted — it only needs to be Known so an export returning
                // `alias/new()` synthesizes. Every op is derived from `SET_OPS`,
                // the same descriptor that drives the emitted WIT.
                for op in SET_OPS {
                    let res = match op.result {
                        SetOpResult::Handle => Some(format!("{}.set", f.iface)),
                        SetOpResult::Ty(t) => Some(t.to_string()),
                        SetOpResult::Unit => None,
                    };
                    table.insert((f.alias.clone(), op.name.to_string()), res);
                }
            }
        }
    }
    table
}

/// The builtin operation names that are genuinely *overloadable* — the
/// operator-like and derivable operations whose meaning is per-type, so that a
/// single typed `Def` of one of them is treated as one member of an overload
/// set and name-mangled at the boundary (Step 8). This is deliberately much
/// narrower than `builtins::NAMES`: the latter also lists the collection/string
/// library (`get`, `head`, `map`, `concat`, `to-string`, …), which are ordinary
/// functions — defining one of *those* once must keep its given name, not be
/// mangled into illegal-looking labels.
///
/// It MUST include the derivable ops emitted by `Derive` (`eq`, `compare`,
/// `show`, `hash` — see `expand.rs`) so a lone derived/typed `Def eq …` still
/// mangles to `eq-point`, and it covers the comparison/arithmetic operators that
/// share that operator-overloading story. `compare`/`show`/`hash` are not in
/// `builtins::NAMES`, but they are derivable and so belong here.
const OVERLOADABLE_OPS: &[&str] = &[
    // derivable ops (Eq / Ord / Show / Hash)
    "eq", "compare", "show", "hash", // comparison operators
    "lt", "le", "gt", "ge", // arithmetic / ordering operators
    "add", "sub", "mul", "div", "rem", "neg", "min", "max", "abs",
];

/// Whether an exported `name` denotes an overload set that must be name-mangled
/// at the boundary (Step 8). The trigger is either ≥2 module-level Fn defs of
/// `name` (a genuine overload set, whatever the name), **or** `name` being one
/// of the curated overloadable operations (`OVERLOADABLE_OPS`) — a single
/// `Def eq …` is still mangled because `eq` is an overloadable operation name.
/// Ordinary library functions (`get`, `head`, `map`, `to-string`, …) defined
/// once are *not* mangled and keep their given names.
fn is_overload_export(name: &str, fn_defs: &HashMap<String, Vec<(NodeId, NodeId)>>) -> bool {
    let count = fn_defs.get(name).map(|v| v.len()).unwrap_or(0);
    count >= 2 || OVERLOADABLE_OPS.contains(&name)
}

/// Turn a `type_text` rendering of a parameter type into a WIT-identifier-safe
/// token for use as a mangled-name suffix. `type_text` emits the WIT type
/// *syntax* (`list<s32>`, `option<string>`, `tuple<s32, s32>`), whose `<`, `>`,
/// `,` and spaces are illegal in a WIT identifier. This replaces every run of
/// such separators with a single `-`, then trims leading/trailing `-`, so
/// `list<s32>` → `list-s32`, `tuple<s32, s32>` → `tuple-s32-s32`, and a bare
/// `point` stays `point` (keeping `eq-point` unchanged).
fn safe_type_token(ty_text: &str) -> String {
    let mut out = String::with_capacity(ty_text.len());
    for ch in ty_text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            // `<`, `>`, `,`, whitespace, etc. all collapse to a single `-`.
            if !out.ends_with('-') {
                out.push('-');
            }
        }
    }
    out.trim_matches('-').to_string()
}

/// The identifier-safe tokens of every parameter type of an overload-set member,
/// in order: `{a: point b: string}` → `["point", "string"]`. The member's
/// parameters must be typed (a `{a: t …}` record) so the overloads can be
/// distinguished by argument type.
fn param_type_tokens(arena: &Arena, name: &str, params_id: NodeId) -> Result<Vec<String>, String> {
    let Node::Rec(fields) = arena.node(params_id) else {
        return Err(format!(
            "cannot mangle overloaded export `{name}`: its parameters must be \
             typed (a `{{a: t …}}` record) to distinguish overloads"
        ));
    };
    if fields.is_empty() {
        return Err(format!(
            "cannot mangle overloaded export `{name}`: it takes no parameters, \
             so its overloads cannot be distinguished by argument type"
        ));
    }
    fields
        .iter()
        .map(|(_k, ty)| Ok(safe_type_token(&type_text(arena, *ty)?)))
        .collect()
}

/// The mangled WIT name for one overload-set member when its first parameter
/// type alone disambiguates the set: `name-<token>` where `<token>` is an
/// identifier-safe rendering of the WIT type of the member's first parameter.
/// `eq` with a `point` first parameter → `eq-point`; with a `list(s32)` first
/// parameter → `eq-list-s32`.
fn mangle_name(arena: &Arena, name: &str, params_id: NodeId) -> Result<String, String> {
    let tokens = param_type_tokens(arena, name, params_id)?;
    Ok(format!("{name}-{}", tokens[0]))
}

/// The mangled WIT name disambiguated over *all* parameter types, used when the
/// first-parameter label alone collides across the overload set:
/// `{a: point b: string}` → `name-point-string`, `{a: point b: s32}` →
/// `name-point-s32`. Each parameter type goes through the same identifier-safe
/// token path, joined by `-`.
fn mangle_name_full(arena: &Arena, name: &str, params_id: NodeId) -> Result<String, String> {
    let tokens = param_type_tokens(arena, name, params_id)?;
    Ok(format!("{name}-{}", tokens.join("-")))
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
    roots: &[NodeId],
    name: &str,
    params_id: NodeId,
    body: NodeId,
    defs: &HashMap<String, (NodeId, NodeId)>,
    functor_ops: &FunctorOps,
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
    let result = match infer(arena, body, &param_types, defs, functor_ops, &mut visiting) {
        Inferred::Known(t) => Some(t),
        Inferred::Unit => None,
        // The local best-effort walk could not see the result type: fall back
        // to the full Phase A/C checker, which models lists, options, results,
        // tuples, and nominal DefTypes (3.8).
        Inferred::Unknown => match crate::check::infer_wit_result(arena, roots, &params, body) {
            crate::check::InferredWit::Known(t) => Some(t),
            crate::check::InferredWit::Unit => None,
            // Boundary rule (5.8, first cut): the result is a function value —
            // deliberately unspellable at a component boundary (WIT has no
            // function type), so say so rather than "cannot infer".
            crate::check::InferredWit::Func => {
                return Err(format!(
                    "`{name}`: function types cannot cross component boundaries yet; \
                     import the combinator as a macro so it expands into this \
                     component instead"
                ));
            }
            crate::check::InferredWit::Unknown => {
                return Err(format!(
                    "cannot infer result type of `{name}` (use the Export record form)"
                ));
            }
        },
    };
    Ok(FuncSig {
        name: name.to_string(),
        iface: "api".to_string(),
        params,
        result,
    })
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
                    if is_param && let Some(t) = call_param_type(arena, callee, pos, defs) {
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

/// The names a file's `Export` declarations export (bare or record form),
/// without running full collection. Used by the build path to keep exported
/// overload sets intact through `check::resolve_overloads_excluding` (3.14).
pub fn export_names(arena: &Arena, roots: &[NodeId]) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for &root in roots {
        let Node::Tup(items) = arena.node(root) else {
            continue;
        };
        let Some(&head) = items.first() else { continue };
        if !matches!(arena.node(head), Node::Sym(s) if s == "export-MACRO") {
            continue;
        }
        let Some(&p) = items.get(1) else { continue };
        match arena.node(p) {
            Node::Sym(s) => {
                out.insert(s.clone());
            }
            Node::Rec(fields) => {
                for (k, v) in fields {
                    if k == "name"
                        && let Node::Sym(s) = arena.node(*v)
                    {
                        out.insert(s.clone());
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Build checker import signatures (3.1) for one resolved import: every
/// function the dependency exports in `iface`, keyed `alias/name`, its WIT
/// param/result texts parsed into checker types.
pub fn import_sigs_for(alias: &str, funcs: &[FuncSig], iface: &str) -> crate::check::ImportSigs {
    let mut out = crate::check::ImportSigs::new();
    for f in funcs.iter().filter(|f| f.iface == iface) {
        let params: Vec<(String, crate::check::Type)> = f
            .params
            .iter()
            .map(|(n, t)| (n.clone(), crate::check::type_from_wit(t)))
            .collect();
        let result = match &f.result {
            Some(t) => crate::check::type_from_wit(t),
            None => crate::check::Type::Unit,
        };
        out.insert(format!("{alias}/{}", f.name), (params, result));
    }
    out
}

/// Build checker import signatures (3.1) for each functor instantiation's ops
/// (`alias/new`, `alias/add`, …), derived from the same `SET_OPS` descriptor
/// that drives inference and the emitted WIT.
pub fn functor_import_sigs(functors: &[FunctorInst]) -> crate::check::ImportSigs {
    use crate::check::Type;
    let mut out = crate::check::ImportSigs::new();
    for f in functors {
        match f.kind {
            FunctorKind::Set => {
                let handle = Type::Named(format!("{}.set", f.iface));
                let elem = crate::check::type_from_wit(&f.elem);
                for op in SET_OPS {
                    let mut params: Vec<(String, Type)> = Vec::new();
                    if !matches!(op.result, SetOpResult::Handle) {
                        params.push(("self".to_string(), handle.clone()));
                    }
                    if op.takes_value {
                        params.push(("value".to_string(), elem.clone()));
                    }
                    let result = match op.result {
                        SetOpResult::Handle => handle.clone(),
                        SetOpResult::Ty(t) => crate::check::type_from_wit(t),
                        SetOpResult::Unit => Type::Unit,
                    };
                    out.insert(format!("{}/{}", f.alias, op.name), (params, result));
                }
            }
        }
    }
    out
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
/// every referenced package from a registry. Build-set (sibling-`.wlt`) imports
/// have no registry — they are satisfied locally — so handing them to `wkg`
/// fails with "no registry configured". The host imports are exactly the ones
/// `wkg` *should* fetch into `wit/deps`, and their WIT text here is byte-for-byte
/// what [`synthesize`] emits, so the world `wkg` parses matches what the emitter
/// componentizes against. (Sibling packages are kind-(2) dependencies wired by
/// the build set / `wac`, not kind-(1) WIT fetched by `wkg`.)
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

    let mut ifaces = iface_order(&info.exports, !info.types.is_empty());
    // A resource-only export still needs its interface present (4.5): fold each
    // exported resource's placement interface into the interface order.
    for r in &info.resources {
        if let Some(iface) = &r.iface
            && !ifaces.contains(iface)
        {
            ifaces.push(iface.clone());
        }
    }

    // Hoisted local types (4.7), mirroring `emit::synthesize_world_wit`: when
    // an export references a functor handle whose element is a local type, the
    // element's declaration moves into a shared `types` interface so `api` and
    // the functor interface need not `use` each other (a WIT cycle).
    let hoisted = hoisted_types(arena, info)?;
    if !hoisted.is_empty() {
        out.push_str("\ninterface types {\n");
        for name in &hoisted {
            let (_, ty) = info
                .types
                .iter()
                .find(|(n, _)| n == name)
                .expect("hoisted names come from info.types");
            out.push_str(&format!("  {}\n", type_decl(arena, name, *ty)?));
        }
        out.push_str("}\n");
    }

    // External interfaces (e.g. wasi:http/incoming-handler) are defined by the
    // host's WIT; we only export them by name, never re-declare them.
    for iface in ifaces.iter().filter(|i| !is_external_iface(i)) {
        out.push_str(&format!("\ninterface {iface} {{\n"));
        if iface == "api" {
            if !hoisted.is_empty() {
                out.push_str(&format!("  use types.{{{}}};\n", hoisted.join(", ")));
            }
            for (name, ty) in info.types.iter().filter(|(n, _)| !hoisted.contains(n)) {
                out.push_str(&format!("  {}\n", type_decl(arena, name, *ty)?));
            }
        }
        // Exported resource blocks land in their placement interface (4.5).
        for r in info
            .resources
            .iter()
            .filter(|r| r.iface.as_deref() == Some(iface.as_str()))
        {
            out.push_str(&r.to_wit());
        }
        for sig in info.exports.iter().filter(|s| &s.iface == iface) {
            out.push_str(&format!("  {}\n", sig.to_wit()));
        }
        out.push_str("}\n");
    }

    // Functor instantiations stamp out a specialized, monomorphic interface each
    // (Steps 10–11). `Instantiate {pkg: "wavelet:coll/set" with: {elem: T} as: …}` produces a
    // `T-set` interface holding the element-specialized `set` resource (fig-wit).
    for f in &info.functors {
        let elem_iface = if hoisted.contains(&f.elem) {
            Some("types")
        } else if info.types.iter().any(|(name, _)| name == &f.elem) {
            Some("api")
        } else {
            None
        };
        out.push_str(&functor_interface(arena, f, elem_iface)?);
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
    if !hoisted.is_empty() && !host_only {
        out.push_str("  export types;\n");
    }
    for iface in &ifaces {
        if is_external_iface(iface) {
            out.push_str(&format!("  export {};\n", external_versioned(iface)));
        } else if !host_only {
            out.push_str(&format!("  export {iface};\n"));
        }
    }
    // Each functor instantiation exports its specialized interface.
    for f in &info.functors {
        out.push_str(&format!("  export {};\n", f.iface));
    }
    out.push_str("}\n");
    Ok(out)
}

/// The specialized WIT interface for one functor instantiation (fig-wit). For
/// the `Set` functor at element type `T`, a `T-set` interface holding a `set`
/// resource whose every method is monomorphized to `T`.
///
/// `pub(crate)` so the emit/build path (`emit::synthesize_world_wit`) renders the
/// SAME interface text from the SAME `SET_OPS` source as `wavelet wit` does —
/// the resource the wasm backend implements and the WIT the encoder validates
/// against cannot drift.
/// `elem_iface` is the interface that *declares* the element type, when the
/// element is a locally-defined type: the specialized functor interface then
/// `use`s it (`use api.{point};`, or `use types.{point};` once the element has
/// been hoisted to break a cycle — 4.7). `None` for a primitive element
/// (`s32`, `string`, …), which needs no `use`.
pub(crate) fn functor_interface(
    _arena: &Arena,
    f: &FunctorInst,
    elem_iface: Option<&str>,
) -> Result<String, String> {
    match f.kind {
        FunctorKind::Set => {
            let t = &f.elem;
            // The element references a locally-defined record: bring it into
            // scope with a `use` from its declaring interface.
            let uses = match elem_iface {
                Some(iface) => format!("  use {iface}.{{{t}}};\n"),
                None => String::new(),
            };
            // Build the resource members from `SET_OPS`, the same descriptor that
            // drives the inference op-table, so the two cannot drift.
            let mut members = String::new();
            for op in SET_OPS {
                match op.result {
                    SetOpResult::Handle => members.push_str("    constructor();\n"),
                    _ => {
                        let params = if op.takes_value {
                            format!("value: {t}")
                        } else {
                            String::new()
                        };
                        let arrow = match op.result {
                            SetOpResult::Ty(r) => format!(" -> {r}"),
                            _ => String::new(),
                        };
                        members.push_str(&format!(
                            "    {name}: func({params}){arrow};\n",
                            name = op.name,
                        ));
                    }
                }
            }
            Ok(format!(
                "\ninterface {iface} {{\n{uses}  resource set {{\n{members}  }}\n}}\n",
                iface = f.iface,
            ))
        }
    }
}

/// The local types to hoist into a shared `types` interface (4.7): the element
/// types of every functor whose handle some export signature references (as
/// the dotted `<iface>.set` text), plus — transitively — any local types those
/// declarations reference. Hoisting breaks the `api ↔ <elem>-set` WIT
/// interface cycle: both sides `use types.{…}` instead of each other. Shared
/// by [`synthesize_info`] and `emit::synthesize_world_wit` so the two
/// renderers cannot disagree. Empty when no export returns/takes a handle
/// over a local element.
pub(crate) fn hoisted_types(arena: &Arena, info: &FileInfo) -> Result<Vec<String>, String> {
    let handle_referenced = |f: &FunctorInst| {
        let dotted = format!("{}.set", f.iface);
        info.exports.iter().any(|s| {
            s.result.as_deref() == Some(dotted.as_str())
                || s.params.iter().any(|(_, t)| t == &dotted)
        })
    };
    let mut hoisted: Vec<String> = Vec::new();
    for f in info.functors.iter().filter(|f| handle_referenced(f)) {
        if info.types.iter().any(|(n, _)| n == &f.elem) && !hoisted.contains(&f.elem) {
            hoisted.push(f.elem.clone());
        }
    }
    // Transitively pull in local types the hoisted declarations reference.
    let mut i = 0;
    while i < hoisted.len() {
        let name = hoisted[i].clone();
        i += 1;
        let Some((_, ty)) = info.types.iter().find(|(n, _)| n == &name) else {
            continue;
        };
        let text = type_decl(arena, &name, *ty)?;
        for (other, _) in &info.types {
            if !hoisted.contains(other)
                && text
                    .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                    .any(|tok| tok == other)
            {
                hoisted.push(other.clone());
            }
        }
    }
    if !hoisted.is_empty()
        && (info.exports.iter().any(|s| s.iface == "types")
            || info.functors.iter().any(|f| f.iface == "types"))
    {
        return Err(
            "cannot hoist functor element types: the interface name `types` is already taken"
                .into(),
        );
    }
    Ok(hoisted)
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
            // A list whose cases are all payload-less is a WIT `enum`; any
            // payloaded case makes it a `variant` (4.1). Same single-i32
            // discriminant ABI either way, but the synthesized WIT now says
            // what a WIT author would say.
            if cases.iter().all(|&c| matches!(arena.node(c), Node::Sym(_))) {
                let names: Vec<String> = cases
                    .iter()
                    .map(|&c| match arena.node(c) {
                        Node::Sym(s) => s.clone(),
                        _ => unreachable!("all-Sym checked above"),
                    })
                    .collect();
                return Ok(format!("enum {name} {{ {} }}", names.join(", ")));
            }
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
            // `own(<resource>)` is the explicit synonym for a bare owned handle
            // (4.5); WIT spells an owned handle with the bare resource name, so
            // strip the `own` wrapper. `borrow(<resource>)` keeps its WIT keyword
            // form (`borrow<counter>`) through the generic rendering below.
            if ctor == "own"
                && let [inner] = args
            {
                return type_text(arena, *inner);
            }
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
    functor_ops: &FunctorOps,
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
            // A qualified call `alias/op(...)` is a functor op (Steps 10–11): its
            // result type comes from the instantiation's op table, keyed by alias.
            if let Node::Qsym(alias, op) = arena.node(head) {
                return match functor_ops.get(&(alias.clone(), op.clone())) {
                    Some(Some(t)) => Inferred::Known(t.clone()),
                    Some(None) => Inferred::Unit,
                    None => Inferred::Unknown,
                };
            }
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
                    infer(arena, args[0], params, defs, functor_ops, visiting)
                }
                "add" | "sub" | "mul" | "div" | "rem" | "neg" | "min" | "max" | "abs" => {
                    let any_dec = args.iter().any(|&i| {
                        matches!(infer(arena, i, params, defs, functor_ops, visiting),
                                 Inferred::Known(t) if t == "f64")
                    });
                    Inferred::Known(if any_dec { "f64" } else { "s64" }.into())
                }
                "drop" | "cell-set" => Inferred::Unit,
                "let-MACRO" if args.len() == 2 => {
                    let mut scope = params.clone();
                    if let Node::Rec(fields) = arena.node(args[0]) {
                        for (k, v) in fields {
                            if let Inferred::Known(t) =
                                infer(arena, *v, &scope, defs, functor_ops, visiting)
                            {
                                scope.insert(k.clone(), t);
                            }
                        }
                    }
                    infer(arena, args[1], &scope, defs, functor_ops, visiting)
                }
                // unify every clause's result type; pattern-bound names are
                // left untyped (best effort, as elsewhere)
                "match-MACRO" if args.len() == 2 => match arena.node(args[1]) {
                    Node::Lst(clauses) => {
                        let mut acc: Option<Inferred> = None;
                        for &c in clauses {
                            if let Node::Tup(pair) = arena.node(c)
                                && pair.len() == 2
                            {
                                let r = infer(arena, pair[1], params, defs, functor_ops, visiting);
                                acc = Some(match acc {
                                    None => r,
                                    Some(prev) => unify(prev, r),
                                });
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
                        let r = infer(arena, *body, &callee_params, defs, functor_ops, visiting);
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

    #[test]
    fn synthesize_defresource_counter_block() {
        // The 4.5 conformance counter: an exported resource plus the own/borrow
        // free functions. The synthesized WIT must match the suite interface.
        let src = "Package \"roundtrip:suite@0.1.0\"\n            DefResource counter {\n              New: Fn {start: u32} cell-new(start)\n              next: Fn {self: counter} The u32 cell-get(self)\n              value: Fn {self: counter} The u32 cell-get(self)\n              sum: Static Fn {values: list(u32)} The counter counter(fold(add 0 values))\n            }\n            Export counter\n            Def make-counter Fn {start: u32} The counter counter(start)\n            Export make-counter\n            Def bump-counter Fn {c: borrow(counter)} The u32 counter/next(c)\n            Export bump-counter\n            Def take-counter Fn {c: counter} The u32 counter/value(c)\n            Export take-counter\n            Def counter-round-trip Fn {c: counter} The counter counter(0)\n            Export counter-round-trip";
        let (arena, roots) = read_file(src).expect("read");
        let got = synthesize(&arena, &roots).expect("synthesize");
        for needle in [
            "resource counter {",
            "constructor(start: u32);",
            "next: func() -> u32;",
            "value: func() -> u32;",
            "sum: static func(values: list<u32>) -> counter;",
            "make-counter: func(start: u32) -> counter;",
            "bump-counter: func(c: borrow<counter>) -> u32;",
            "take-counter: func(c: counter) -> u32;",
            "counter-round-trip: func(c: counter) -> counter;",
        ] {
            assert!(got.contains(needle), "missing `{needle}` in:\n{got}");
        }
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
