use super::*;
use std::collections::HashSet;

#[derive(Default)]
struct Declarations<'a> {
    package: Option<&'a str>,
    imports: Vec<ImportInfo>,
    functors: Vec<FunctorInst>,
    exports: Vec<(String, Option<FuncSig>)>,
    types: Vec<(String, NodeId)>,
    fn_defs: HashMap<String, Vec<(NodeId, NodeId)>>,
    value_defs: Vec<(String, NodeId)>,
    resources: Vec<ResourceForm<'a>>,
}

struct ResourceForm<'a> {
    name: &'a str,
    members: &'a [(String, NodeId)],
}

impl<'a> Declarations<'a> {
    fn read(arena: &'a Arena, roots: &[NodeId]) -> Result<Self, String> {
        let mut declarations = Self::default();
        for &root in roots {
            let Node::Tup(items) = arena.node(root) else {
                continue;
            };
            let Some((&head, args)) = items.split_first() else {
                continue;
            };
            let Node::Sym(name) = arena.node(head) else {
                continue;
            };
            match (name.as_str(), args) {
                ("package-MACRO", [package, ..]) => {
                    if let Node::Str(package) = arena.node(*package) {
                        declarations.package = Some(package);
                    }
                }
                ("instantiate-MACRO", [instantiation, ..]) => {
                    let Node::Rec(fields) = arena.node(*instantiation) else {
                        return Err("malformed Instantiate (expects a record)".into());
                    };
                    declarations
                        .functors
                        .push(parse_instantiate(arena, fields)?);
                }
                ("import-MACRO", [import, ..]) => {
                    declarations
                        .imports
                        .push(imports::parse_import(arena, *import)?);
                }
                ("export-MACRO", [export, ..]) => {
                    let declaration = match arena.node(*export) {
                        Node::Sym(name) => (name.clone(), None),
                        Node::Rec(fields) => {
                            let sig =
                                parse_explicit_sig(arena, fields).ok_or("malformed Export")?;
                            (sig.name.clone(), Some(sig))
                        }
                        _ => return Err("malformed Export".into()),
                    };
                    if !declarations.exports.contains(&declaration) {
                        declarations.exports.push(declaration);
                    }
                }
                ("deftype-MACRO", [name, body, ..]) => {
                    if let Node::Sym(name) = arena.node(*name) {
                        declarations.types.push((name.clone(), *body));
                    }
                }
                ("defresource-MACRO", [name, members, ..]) => {
                    if let Node::Sym(name) = arena.node(*name)
                        && let Node::Rec(members) = arena.node(*members)
                    {
                        declarations.resources.push(ResourceForm { name, members });
                    }
                }
                ("def-MACRO", [name, value, ..]) => {
                    if let Node::Sym(name) = arena.node(*name) {
                        declarations.define(arena, name, *value);
                    }
                }
                _ => {}
            }
        }
        Ok(declarations)
    }

    fn define(&mut self, arena: &Arena, name: &str, value: NodeId) {
        if let Some(function) = as_fn_form(arena, value) {
            self.fn_defs
                .entry(name.to_owned())
                .or_default()
                .push(function);
        } else {
            self.value_defs.push((name.to_owned(), value));
        }
    }
}

pub fn collect(arena: &Arena, roots: &[NodeId]) -> Result<FileInfo, String> {
    let declarations = Declarations::read(arena, roots)?;
    let package = declarations
        .package
        .ok_or("file has no Package declaration")?
        .to_owned();
    let package_path = strip_version(&package);
    let world = package_path
        .rsplit(':')
        .next()
        .unwrap_or("component")
        .to_owned();
    let defs = declarations
        .fn_defs
        .iter()
        .map(|(name, members)| (name.clone(), *members.last().unwrap()))
        .collect();
    let mut info = FileInfo {
        package,
        package_path,
        world,
        imports: declarations.imports,
        functors: declarations.functors,
        exports: Vec::new(),
        types: declarations.types,
        defs,
        fn_defs: declarations.fn_defs,
        value_defs: declarations.value_defs,
        overload_bodies: HashMap::new(),
        resources: Vec::new(),
    };
    let functor_ops = functor_op_table(&info.functors);
    info.resources = declarations
        .resources
        .into_iter()
        .map(|resource| collect_resource(arena, roots, resource, &info.defs, &functor_ops))
        .collect::<Result<_, _>>()?;
    collect_exports(arena, roots, &mut info, declarations.exports, &functor_ops)?;
    Ok(info)
}

fn collect_resource(
    arena: &Arena,
    roots: &[NodeId],
    resource: ResourceForm<'_>,
    defs: &HashMap<String, (NodeId, NodeId)>,
    functor_ops: &FunctorOps,
) -> Result<ResourceDecl, String> {
    let mut declaration = ResourceDecl {
        name: resource.name.to_owned(),
        iface: None,
        constructor: None,
        methods: Vec::new(),
        statics: Vec::new(),
        has_drop: false,
    };
    for (key, val) in resource.members {
        let (is_static, fn_id) = strip_static_member(arena, *val);
        let (params_id, body) = as_fn_form(arena, fn_id).ok_or_else(|| {
            format!(
                "DefResource `{}`: member `{key}` must be an `Fn`",
                resource.name
            )
        })?;
        match key.as_str() {
            "New" => declaration.constructor = Some(annotated_params(arena, params_id)?),
            "Drop" => declaration.has_drop = true,
            other => {
                let mut sig = infer_sig(arena, roots, other, params_id, body, defs, functor_ops)?;
                sig.iface.clear();
                if is_static {
                    declaration.statics.push(sig);
                } else {
                    if !sig.params.is_empty() {
                        sig.params.remove(0);
                    }
                    declaration.methods.push(sig);
                }
            }
        }
    }
    Ok(declaration)
}

fn collect_exports(
    arena: &Arena,
    roots: &[NodeId],
    info: &mut FileInfo,
    declarations: Vec<(String, Option<FuncSig>)>,
    functor_ops: &FunctorOps,
) -> Result<(), String> {
    for (name, explicit) in declarations {
        if let Some(resource) = info
            .resources
            .iter_mut()
            .find(|resource| resource.name == name)
        {
            resource.iface = Some(
                explicit
                    .map(|sig| sig.iface)
                    .unwrap_or_else(|| "api".into()),
            );
            continue;
        }
        let explicit_overrides =
            matches!(&explicit, Some(sig) if !sig.params.is_empty() || sig.result.is_some());
        if !explicit_overrides && is_overload_export(&name, &info.fn_defs) {
            let members = info
                .fn_defs
                .get(&name)
                .ok_or_else(|| format!("Export `{name}` has no definition"))?;
            let iface = explicit
                .map(|sig| sig.iface)
                .unwrap_or_else(|| "api".into());
            for (&(params_id, body), mangled) in
                members.iter().zip(overload_labels(arena, &name, members)?)
            {
                let mut sig = infer_sig(
                    arena,
                    roots,
                    &mangled,
                    params_id,
                    body,
                    &info.defs,
                    functor_ops,
                )?;
                sig.iface = iface.clone();
                info.overload_bodies.insert(mangled, (params_id, body));
                info.exports.push(sig);
            }
        } else {
            let sig = export_signature(arena, roots, &name, explicit, &info.defs, functor_ops)?;
            info.exports.push(sig);
        }
    }
    Ok(())
}

fn export_signature(
    arena: &Arena,
    roots: &[NodeId],
    name: &str,
    explicit: Option<FuncSig>,
    defs: &HashMap<String, (NodeId, NodeId)>,
    functor_ops: &FunctorOps,
) -> Result<FuncSig, String> {
    match explicit {
        Some(sig) if sig.params.is_empty() && sig.result.is_none() && defs.contains_key(name) => {
            let (params_id, body) = defs[name];
            let mut inferred = infer_sig(arena, roots, name, params_id, body, defs, functor_ops)?;
            inferred.iface = sig.iface;
            Ok(inferred)
        }
        Some(sig) => Ok(sig),
        None => {
            let &(params_id, body) = defs
                .get(name)
                .ok_or_else(|| format!("Export `{name}` has no definition"))?;
            infer_sig(arena, roots, name, params_id, body, defs, functor_ops)
        }
    }
}

fn overload_labels(
    arena: &Arena,
    name: &str,
    members: &[(NodeId, NodeId)],
) -> Result<Vec<String>, String> {
    let tokens = members
        .iter()
        .map(|&(params_id, _)| param_type_tokens(arena, name, params_id))
        .collect::<Result<Vec<_>, _>>()?;
    let short = tokens
        .iter()
        .map(|types| format!("{name}-{}", types[0]))
        .collect::<Vec<_>>();
    if distinct(&short) {
        return Ok(short);
    }
    let full = tokens
        .iter()
        .map(|types| format!("{name}-{}", types.join("-")))
        .collect::<Vec<_>>();
    if !distinct(&full) {
        return Err(format!(
            "exported overload set `{name}` has two members with identical \
             parameter types; they mangle to the same WIT name and cannot \
             both be exported (remove the duplicate definition)"
        ));
    }
    Ok(full)
}

fn distinct(names: &[String]) -> bool {
    names.iter().collect::<HashSet<_>>().len() == names.len()
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
