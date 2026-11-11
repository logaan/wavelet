use super::*;

pub(crate) fn record_types(
    arena: &Arena,
    types: &[(String, NodeId)],
) -> Vec<(String, Vec<(String, String)>)> {
    let mut out = Vec::new();
    for (name, node) in types {
        if let Node::Rec(fields) = arena.node(*node) {
            let mut fs = Vec::with_capacity(fields.len());
            let mut ok = true;
            for (fname, fnode) in fields {
                match crate::wit::type_text(arena, *fnode) {
                    Ok(t) => fs.push((fname.clone(), t)),
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                out.push((name.clone(), fs));
            }
        }
    }
    out
}

pub fn dep_record_types(arena: &Arena, info: &FileInfo) -> Vec<(String, Vec<(String, String)>)> {
    record_types(arena, &info.types)
}

pub fn dep_non_record_types(
    arena: &Arena,
    info: &FileInfo,
) -> (Vec<(String, TypeDef)>, Vec<(String, String)>) {
    local_non_record_types(arena, &info.types)
}

pub(crate) fn local_non_record_types(
    arena: &Arena,
    types: &[(String, NodeId)],
) -> (Vec<(String, TypeDef)>, Vec<(String, String)>) {
    let mut defs = Vec::new();
    let mut aliases = Vec::new();
    for (name, node) in types {
        match arena.node(*node) {
            Node::Rec(_) => {}
            Node::Flg(names) => defs.push((name.clone(), TypeDef::Flags(names.clone()))),
            Node::Lst(cases) => {
                let mut resolved = Vec::with_capacity(cases.len());
                let mut ok = true;
                for &c in cases {
                    match arena.node(c) {
                        Node::Sym(s) => resolved.push((s.clone(), None)),
                        Node::Tup(items) => {
                            let Some((&h, payload)) = items.split_first() else {
                                ok = false;
                                break;
                            };
                            let Node::Sym(case) = arena.node(h) else {
                                ok = false;
                                break;
                            };

                            match payload {
                                [] => resolved.push((case.clone(), None)),
                                [one] => match crate::wit::type_text(arena, *one) {
                                    Ok(t) => resolved.push((case.clone(), Some(t))),
                                    Err(_) => {
                                        ok = false;
                                        break;
                                    }
                                },
                                _ => {
                                    ok = false;
                                    break;
                                }
                            }
                        }
                        _ => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    if resolved.iter().all(|(_, p)| p.is_none()) {
                        let names = resolved.into_iter().map(|(n, _)| n).collect();
                        defs.push((name.clone(), TypeDef::Enum(names)));
                    } else {
                        defs.push((name.clone(), TypeDef::Variant(resolved)));
                    }
                }
            }

            _ => {
                if let Ok(t) = crate::wit::type_text(arena, *node) {
                    aliases.push((name.clone(), t));
                }
            }
        }
    }
    (defs, aliases)
}

pub(crate) fn import_iface(path: &str) -> String {
    match path.split_once('/') {
        Some((_, iface)) => iface.to_string(),
        None => "api".to_string(),
    }
}

pub(crate) fn is_external_iface(iface: &str) -> bool {
    iface.contains(':')
}

pub(crate) fn external_versioned_in(path: &str, deps: &HashMap<String, Dep>) -> String {
    if path.contains('@') {
        return path.to_owned();
    }
    if let Some((pkg, _iface)) = path.split_once('/')
        && let Some(dep) = deps.get(pkg)
        && let Some((_base, ver)) = dep.package.split_once('@')
    {
        return format!("{path}@{ver}");
    }
    path.to_owned()
}

pub(crate) fn versioned_iface(pkg: &str, iface: &str) -> String {
    match pkg.split_once('@') {
        Some((base, ver)) => format!("{base}/{iface}@{ver}"),
        None => format!("{pkg}/{iface}"),
    }
}

pub(crate) fn dep_func_op(name: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    if let Some(rest) = name.strip_prefix("[constructor]") {
        return Cow::Borrowed(rest);
    }
    if let Some(rest) = name.strip_prefix("[resource-drop]") {
        return Cow::Owned(format!("drop-{rest}"));
    }
    for prefix in ["[method]", "[static]"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return Cow::Borrowed(rest.rsplit_once('.').map(|(_, op)| op).unwrap_or(rest));
        }
    }
    Cow::Borrowed(name)
}

pub(crate) fn dep_func_qualified(name: &str) -> Option<String> {
    if let Some(rest) = name.strip_prefix("[constructor]") {
        return Some(rest.to_string());
    }
    for prefix in ["[method]", "[static]"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return Some(rest.replacen('.', "-", 1));
        }
    }
    None
}

pub(crate) fn resolve_dep_func<'a>(
    dep: &'a Dep,
    iface: &str,
    fname: &str,
) -> Result<&'a crate::wit::FuncSig, String> {
    let in_iface = || dep.funcs.iter().filter(|f| f.iface == iface);

    if let Some(f) = in_iface()
        .find(|f| f.name == fname || dep_func_qualified(&f.name).as_deref() == Some(fname))
    {
        return Ok(f);
    }

    let mut bare = in_iface().filter(|f| dep_func_op(&f.name) == *fname);
    let first = bare.next().ok_or(format!(
        "`{}` does not export `{fname}` in `{iface}`",
        dep.package
    ))?;
    if let Some(second) = bare.next() {
        return Err(format!(
            "`{fname}` is ambiguous in `{}/{iface}`: matches both `{}` and `{}`; \
             use the resource-qualified name (e.g. `{}`)",
            dep.package,
            first.name,
            second.name,
            dep_func_qualified(&first.name).unwrap_or_else(|| first.name.clone()),
        ));
    }
    Ok(first)
}

pub(crate) fn dep_case(dep: &Dep, name: &str) -> Option<bool> {
    dep.type_defs.iter().find_map(|(_, def)| match def {
        TypeDef::Enum(cases) => cases.iter().any(|c| c == name).then_some(false),
        TypeDef::Variant(cases) => cases
            .iter()
            .find(|(c, _)| c == name)
            .map(|(_, p)| p.is_some()),
        _ => None,
    })
}

pub(crate) fn validate_dependency_versions(
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> Result<(), String> {
    for imp in &info.imports {
        if crate::wit::is_macro_only(imp) {
            continue;
        }
        if let Some(version) = &imp.version
            && let Some(dep) = deps.get(&imp.package)
            && dep.package != format!("{}@{version}", imp.package)
        {
            return Err(format!(
                "import `{}` requests version `{version}`, but resolved `{}`",
                imp.path, dep.package
            ));
        }
    }
    for interface in info.exports.iter().map(|sig| sig.iface.as_str()).chain(
        info.resources
            .iter()
            .filter_map(|resource| resource.iface.as_deref()),
    ) {
        if let Some((path, version)) = interface.split_once('@')
            && let Some((package, _)) = path.split_once('/')
            && let Some(dep) = deps.get(package)
            && dep.package != format!("{package}@{version}")
        {
            return Err(format!(
                "export `{}` conflicts with resolved `{}`",
                interface, dep.package
            ));
        }
    }
    Ok(())
}

pub(crate) fn import_signatures(
    info: &crate::wit::FileInfo,
    dependencies: &HashMap<String, Dep>,
) -> crate::check::ImportSigs {
    let mut signatures = crate::check::ImportSigs::new();
    for import in &info.imports {
        if crate::wit::is_macro_only(import) {
            continue;
        }
        if let Some(dependency) = dependencies.get(&import.package) {
            let interface = import
                .path
                .split_once('/')
                .map(|(_, name)| name)
                .unwrap_or("api");
            signatures.extend(crate::wit::import_sigs_for(
                &import.alias,
                &dependency.funcs,
                interface,
            ));
        }
    }
    signatures.extend(crate::wit::functor_import_sigs(&info.functors));
    signatures
}
