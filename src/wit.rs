mod imports;
pub(crate) mod names;

pub(crate) use imports::import_from_record;

use std::collections::HashMap;

use crate::form::{Arena, Node, NodeId};

pub struct FileInfo {
    pub package: String,

    pub package_path: String,

    pub world: String,
    pub imports: Vec<ImportInfo>,

    pub functors: Vec<FunctorInst>,
    pub exports: Vec<FuncSig>,
    pub types: Vec<(String, NodeId)>,

    pub defs: HashMap<String, (NodeId, NodeId)>,

    pub fn_defs: HashMap<String, Vec<(NodeId, NodeId)>>,

    pub value_defs: Vec<(String, NodeId)>,

    pub overload_bodies: HashMap<String, (NodeId, NodeId)>,

    pub resources: Vec<ResourceDecl>,
}

#[derive(Clone)]
pub struct ResourceDecl {
    pub name: String,

    pub iface: Option<String>,

    pub constructor: Option<Vec<(String, String)>>,

    pub methods: Vec<FuncSig>,

    pub statics: Vec<FuncSig>,

    pub has_drop: bool,
}

impl ResourceDecl {
    pub fn to_wit(&self) -> String {
        let mut out = format!("  resource {} {{\n", names::identifier(&self.name));
        if let Some(params) = &self.constructor {
            out.push_str(&format!(
                "    constructor({});\n",
                names::parameters(params)
            ));
        }
        for (members, prefix) in [(&self.methods, ""), (&self.statics, "static ")] {
            for member in members {
                out.push_str(&format!("    {}\n", member.declaration(prefix)));
            }
        }
        out.push_str("  }\n");
        out
    }
}

fn strip_static_member(arena: &Arena, val: NodeId) -> (bool, NodeId) {
    if let Node::Tup(items) = arena.node(val)
        && items.len() == 2
        && matches!(arena.node(items[0]), Node::Sym(s) if s == "static-MACRO")
    {
        return (true, items[1]);
    }
    (false, val)
}

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

pub fn import_roles(arena: &Arena, fields: &[(String, NodeId)]) -> Result<(bool, bool), String> {
    let legacy = fields.iter().find(|(name, _)| name == "macros");
    if let Some((_, id)) = fields.iter().find(|(name, _)| name == "roles") {
        if legacy.is_some() {
            return Err("Import: use either `roles` or `macros`, not both".into());
        }
        let Node::Flg(roles) = arena.node(*id) else {
            return Err("Import roles must be a flags literal such as {runtime macro}".into());
        };
        if roles.is_empty()
            || roles
                .iter()
                .any(|role| role != "runtime" && role != "macro")
        {
            return Err("Import roles must contain `runtime`, `macro`, or both".into());
        }
        return Ok((
            roles.iter().any(|r| r == "macro"),
            roles.iter().any(|r| r == "runtime"),
        ));
    }
    let macros = match legacy {
        Some((_, id)) => match arena.node(*id) {
            Node::Bool(value) => *value,
            _ => return Err("Import `macros` must be a boolean".into()),
        },
        None => false,
    };
    Ok((macros, !macros))
}

pub struct ImportInfo {
    pub path: String,
    pub version: Option<String>,

    pub package: String,
    pub alias: String,

    pub macros: bool,
    pub runtime: bool,

    pub from: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub enum FunctorKind {
    Set,
}

pub struct FunctorInst {
    pub kind: FunctorKind,

    pub alias: String,

    pub elem: String,

    pub iface: String,
}

enum SetOpResult {
    Handle,

    Ty(&'static str),

    Unit,
}

struct SetOp {
    name: &'static str,
    takes_value: bool,
    result: SetOpResult,
}

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

    pub iface: String,
    pub params: Vec<(String, String)>,
    pub result: Option<String>,
}

impl FuncSig {
    pub fn to_wit(&self) -> String {
        self.declaration("")
    }

    fn declaration(&self, prefix: &str) -> String {
        let result = self
            .result
            .as_ref()
            .map(|ty| format!(" -> {}", names::type_expression(ty)))
            .unwrap_or_default();
        format!(
            "{}: {prefix}func({}){result};",
            names::identifier(&self.name),
            names::parameters(&self.params)
        )
    }
}

pub(crate) fn qualified_name(info: &FileInfo, name: &str) -> Option<(String, String)> {
    if info.defs.contains_key(name) || info.value_defs.iter().any(|(n, _)| n == name) {
        return None;
    }
    info.imports
        .iter()
        .filter(|i| !is_macro_only(i))
        .map(|i| i.alias.as_str())
        .chain(info.functors.iter().map(|f| f.alias.as_str()))
        .chain(info.resources.iter().map(|r| r.name.as_str()))
        .filter_map(|alias| {
            name.strip_prefix(&format!("{alias}-"))
                .map(|suffix| (alias.to_owned(), suffix.to_owned()))
        })
        .max_by_key(|(alias, _)| alias.len())
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

    let mut resource_forms: Vec<(String, Vec<(String, NodeId)>)> = Vec::new();

    for &root in roots {
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

            "instantiate-MACRO" => {
                let Some(&p) = items.get(1) else { continue };
                let Node::Rec(fields) = arena.node(p) else {
                    return Err("malformed Instantiate (expects a record)".into());
                };
                functors.push(parse_instantiate(arena, fields)?);
            }
            "import-MACRO" => {
                let Some(&p) = items.get(1) else { continue };

                imports.push(imports::parse_import(arena, p)?);
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

    let functor_ops = functor_op_table(&functors);

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

    let export_decls = dedup_export_decls(export_decls);

    let mut exports = Vec::new();
    for (name, explicit) in export_decls {
        if resource_names.contains(&name) {
            let iface = explicit
                .map(|s| s.iface)
                .unwrap_or_else(|| "api".to_string());
            if let Some(r) = resources.iter_mut().find(|r| r.name == name) {
                r.iface = Some(iface);
            }
            continue;
        }

        let explicit_overrides =
            matches!(&explicit, Some(s) if !s.params.is_empty() || s.result.is_some());
        if !explicit_overrides && is_overload_export(&name, &fn_defs) {
            let members = fn_defs
                .get(&name)
                .ok_or(format!("Export `{name}` has no definition"))?;
            let iface = explicit
                .map(|s| s.iface)
                .unwrap_or_else(|| "api".to_string());

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

                overload_bodies.insert(sig.name.clone(), (params_id, body));
                exports.push(sig);
            }
            continue;
        }

        let sig = match explicit {
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

fn opt_sig_eq(a: &Option<FuncSig>, b: &Option<FuncSig>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            x.name == y.name && x.iface == y.iface && x.params == y.params && x.result == y.result
        }
        _ => false,
    }
}

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

fn strip_version(s: &str) -> String {
    s.split('@').next().unwrap_or(s).to_string()
}

fn record_pkg(arena: &Arena, fields: &[(String, NodeId)]) -> Option<String> {
    fields
        .iter()
        .find_map(|(k, v)| match (k.as_str(), arena.node(*v)) {
            ("pkg", Node::Str(s)) => Some(s.clone()),
            _ => None,
        })
}

fn is_functor_pkg(pkg: &str) -> bool {
    strip_version(pkg).ends_with("coll/set")
}

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

type FunctorOps = HashMap<(String, String), Option<String>>;

fn functor_op_table(functors: &[FunctorInst]) -> FunctorOps {
    let mut table: FunctorOps = HashMap::new();
    for f in functors {
        match f.kind {
            FunctorKind::Set => {
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

const OVERLOADABLE_OPS: &[&str] = &[
    "eq", "compare", "show", "hash", "lt", "le", "gt", "ge", "add", "sub", "mul", "div", "rem",
    "neg", "min", "max", "abs",
];

fn is_overload_export(name: &str, fn_defs: &HashMap<String, Vec<(NodeId, NodeId)>>) -> bool {
    let count = fn_defs.get(name).map(|v| v.len()).unwrap_or(0);
    count >= 2 || OVERLOADABLE_OPS.contains(&name)
}

fn safe_type_token(ty_text: &str) -> String {
    let mut out = String::with_capacity(ty_text.len());
    for ch in ty_text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            if !out.ends_with('-') {
                out.push('-');
            }
        }
    }
    out.trim_matches('-').to_string()
}

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

fn mangle_name(arena: &Arena, name: &str, params_id: NodeId) -> Result<String, String> {
    let tokens = param_type_tokens(arena, name, params_id)?;
    Ok(format!("{name}-{}", tokens[0]))
}

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

        Inferred::Unknown => match crate::check::infer_wit_result(arena, roots, &params, body) {
            crate::check::InferredWit::Known(t) => Some(t),
            crate::check::InferredWit::Unit => None,

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

fn builtin_param_type(callee: &str, pos: usize) -> Option<String> {
    match callee {
        "upper" | "lower" | "str-cat" | "split" | "join" => Some("string".into()),

        "contains" => Some("string".into()),
        _ => {
            let _ = pos;
            None
        }
    }
}

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

        Some(_) => conflict = true,
    });
    if conflict { None } else { found }
}

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
                    let is_param = matches!(arena.node(arg), Node::Sym(s) if s == pname);
                    if is_param && let Some(t) = call_param_type(arena, callee, pos, defs) {
                        on_use(t);
                    }
                }
            }

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

fn call_param_type(
    arena: &Arena,
    callee: &str,
    pos: usize,
    defs: &HashMap<String, (NodeId, NodeId)>,
) -> Option<String> {
    if let Some(t) = builtin_param_type(callee, pos) {
        return Some(t);
    }

    if let Some((params_id, _body)) = defs.get(callee)
        && let Node::Rec(fields) = arena.node(*params_id)
        && let Some((_k, v)) = fields.get(pos)
        && let Ok(t) = type_text(arena, *v)
    {
        return Some(t);
    }
    None
}

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
        let signature = (params, result);
        out.insert(format!("{alias}-{}", f.name), signature.clone());
        let source = f
            .name
            .strip_prefix("[constructor]")
            .map(str::to_owned)
            .or_else(|| {
                f.name
                    .strip_prefix("[resource-drop]")
                    .map(|name| format!("drop-{name}"))
            })
            .or_else(|| {
                f.name
                    .strip_prefix("[method]")
                    .or_else(|| f.name.strip_prefix("[static]"))
                    .map(|name| name.replace('.', "-"))
            })
            .unwrap_or_else(|| f.name.clone());
        out.insert(format!("{alias}-{source}"), signature.clone());
        if let Some((_, bare)) = f
            .name
            .strip_prefix("[method]")
            .or_else(|| f.name.strip_prefix("[static]"))
            .and_then(|name| name.rsplit_once('.'))
            && funcs
                .iter()
                .filter(|other| other.iface == iface && other.name.ends_with(&format!(".{bare}")))
                .count()
                == 1
        {
            out.insert(format!("{alias}-{bare}"), signature);
        }
    }
    out
}

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
                    out.insert(format!("{}-{}", f.alias, op.name), (params, result));
                }
            }
        }
    }
    out
}

pub fn synthesize(arena: &Arena, roots: &[NodeId]) -> Result<String, String> {
    let info = collect(arena, roots)?;
    synthesize_info(arena, &info, None)
}

pub fn synthesize_fetch_world(arena: &Arena, roots: &[NodeId]) -> Result<String, String> {
    synthesize_fetch_world_excluding(arena, roots, &[])
}

pub fn synthesize_fetch_world_excluding(
    arena: &Arena,
    roots: &[NodeId],
    local_packages: &[String],
) -> Result<String, String> {
    let info = collect(arena, roots)?;
    synthesize_info(arena, &info, Some(local_packages))
}

pub fn is_macro_only(imp: &ImportInfo) -> bool {
    imp.macros && !imp.runtime
}

pub fn has_external_deps(info: &FileInfo, local_packages: &[String]) -> bool {
    info.imports
        .iter()
        .any(|i| !is_macro_only(i) && !local_packages.contains(&i.package))
        || iface_order(&info.exports, !info.types.is_empty())
            .iter()
            .any(|i| is_external_iface(i))
        || info
            .resources
            .iter()
            .filter_map(|resource| resource.iface.as_deref())
            .any(is_external_iface)
}

fn synthesize_info(
    arena: &Arena,
    info: &FileInfo,
    local_packages: Option<&[String]>,
) -> Result<String, String> {
    let host_only = local_packages.is_some();
    let mut out = String::new();
    out.push_str(&format!("package {};\n", names::path(&info.package)));

    let mut ifaces = iface_order(&info.exports, !info.types.is_empty());

    for r in &info.resources {
        if let Some(iface) = &r.iface
            && !ifaces.contains(iface)
        {
            ifaces.push(iface.clone());
        }
    }

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

    for iface in ifaces.iter().filter(|i| !is_external_iface(i)) {
        out.push_str(&format!("\ninterface {} {{\n", names::identifier(iface)));
        if iface == "api" {
            if !hoisted.is_empty() {
                out.push_str(&format!(
                    "  use types.{{{}}};\n",
                    names::identifiers(&hoisted)
                ));
            }
            for (name, ty) in info.types.iter().filter(|(n, _)| !hoisted.contains(n)) {
                out.push_str(&format!("  {}\n", type_decl(arena, name, *ty)?));
            }
        }

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

    out.push_str(&format!("\nworld {} {{\n", names::identifier(&info.world)));
    for imp in &info.imports {
        if is_macro_only(imp) {
            continue;
        }

        if !local_packages.is_some_and(|packages| packages.contains(&imp.package)) {
            let path = match &imp.version {
                Some(version) => format!("{}@{version}", imp.path),
                None => imp.path.clone(),
            };
            out.push_str(&format!("  import {};\n", names::path(&path)));
        }
    }
    if !hoisted.is_empty() && !host_only {
        out.push_str("  export types;\n");
    }
    for iface in &ifaces {
        if is_external_iface(iface) {
            out.push_str(&format!("  export {};\n", names::path(iface)));
        } else if !host_only {
            out.push_str(&format!("  export {};\n", names::identifier(iface)));
        }
    }

    for f in &info.functors {
        out.push_str(&format!("  export {};\n", names::identifier(&f.iface)));
    }
    out.push_str("}\n");
    Ok(out)
}

pub(crate) fn functor_interface(
    _arena: &Arena,
    f: &FunctorInst,
    elem_iface: Option<&str>,
) -> Result<String, String> {
    match f.kind {
        FunctorKind::Set => {
            let t = names::type_expression(&f.elem);

            let uses = match elem_iface {
                Some(iface) => format!(
                    "  use {}.{{{}}};\n",
                    names::identifier(iface),
                    names::identifier(&f.elem)
                ),
                None => String::new(),
            };

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
                iface = names::identifier(&f.iface),
            ))
        }
    }
}

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

fn is_external_iface(iface: &str) -> bool {
    iface.contains(':')
}

pub fn type_decl(arena: &Arena, name: &str, ty: NodeId) -> Result<String, String> {
    let name = names::identifier(name);
    match arena.node(ty) {
        Node::Rec(fields) => {
            let mut parts = Vec::new();
            for (k, v) in fields {
                parts.push(format!(
                    "{}: {}",
                    names::identifier(k),
                    names::type_expression(&type_text(arena, *v)?)
                ));
            }
            Ok(format!("record {name} {{ {} }}", parts.join(", ")))
        }
        Node::Flg(names) => Ok(format!("flags {name} {{ {} }}", names::identifiers(names))),
        Node::Lst(cases) => {
            if cases.iter().all(|&c| matches!(arena.node(c), Node::Sym(_))) {
                let names: Vec<String> = cases
                    .iter()
                    .map(|&c| match arena.node(c) {
                        Node::Sym(s) => names::identifier(s).into_owned(),
                        _ => unreachable!("all-Sym checked above"),
                    })
                    .collect();
                return Ok(format!("enum {name} {{ {} }}", names.join(", ")));
            }
            let mut parts = Vec::new();
            for &c in cases {
                match arena.node(c) {
                    Node::Sym(s) => parts.push(names::identifier(s).into_owned()),

                    Node::Tup(case_items) => {
                        let Some((&h, payload)) = case_items.split_first() else {
                            return Err(format!("bad variant case in `{name}`"));
                        };
                        let Node::Sym(case) = arena.node(h) else {
                            return Err(format!("bad variant case in `{name}`"));
                        };
                        if payload.len() > 1 {
                            return Err(format!(
                                "variant case `{name}/{case}` takes at most one payload type; wrap several types in tuple(...)"
                            ));
                        }
                        if payload.is_empty() {
                            parts.push(names::identifier(case).into_owned());
                            continue;
                        }
                        let payload_text: Vec<String> = payload
                            .iter()
                            .map(|&i| type_text(arena, i).map(|text| names::type_expression(&text)))
                            .collect::<Result<_, _>>()?;
                        parts.push(format!(
                            "{}({})",
                            names::identifier(case),
                            payload_text.join(", ")
                        ));
                    }
                    _ => return Err(format!("bad variant case in `{name}`")),
                }
            }
            Ok(format!("variant {name} {{ {} }}", parts.join(", ")))
        }
        _ => Ok(format!(
            "type {name} = {};",
            names::type_expression(&type_text(arena, ty)?)
        )),
    }
}

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

            let Node::Sym(name) = arena.node(head) else {
                return Inferred::Unknown;
            };
            if let Some((_, result)) = functor_ops
                .iter()
                .find(|((alias, op), _)| format!("{alias}-{op}") == *name)
            {
                return match result {
                    Some(ty) => Inferred::Known(ty.clone()),
                    None => Inferred::Unit,
                };
            }
            match name.as_str() {
                "eq" | "lt" | "le" | "gt" | "ge" | "not" | "empty" | "contains" => {
                    Inferred::Known("bool".into())
                }
                "str-cat" | "upper" | "lower" | "join" | "to-string" => {
                    Inferred::Known("string".into())
                }
                "len" => Inferred::Known("s64".into()),

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
        let info = collect_src(
            "Package \"demo:app@0.1.0\"\n\
             Import {pkg: \"acme:html/dsl\" as: html}\n",
        );
        assert!(
            !import_named(&info, "html").macros,
            "omitting `macros:` should default to false"
        );

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
