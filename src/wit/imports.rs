use super::{ImportInfo, import_roles, is_functor_pkg, record_pkg, strip_version};
use crate::form::{Arena, Node, NodeId};

pub(super) fn parse_import(arena: &Arena, payload: NodeId) -> Result<ImportInfo, String> {
    if let Node::Rec(fields) = arena.node(payload)
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
    match arena.node(payload) {
        Node::Str(package) => Ok(import_info(package.clone(), None, false, true, None)),
        Node::Rec(fields) => import_from_record(arena, fields),
        _ => Err("malformed Import".into()),
    }
}

pub(crate) fn import_from_record(
    arena: &Arena,
    fields: &[(String, NodeId)],
) -> Result<ImportInfo, String> {
    let mut package = None;
    let mut alias = None;
    let (macros, runtime) = import_roles(arena, fields)?;
    let mut from = None;
    for (key, value) in fields {
        match (key.as_str(), arena.node(*value)) {
            ("pkg", Node::Str(text)) => package = Some(text.clone()),
            ("as", Node::Sym(name)) => alias = Some(name.clone()),
            ("from", Node::Str(path)) => from = Some(path.clone()),
            _ => {}
        }
    }
    Ok(import_info(
        package.ok_or("malformed Import")?,
        alias,
        macros,
        runtime,
        from,
    ))
}

fn import_info(
    package: String,
    alias: Option<String>,
    macros: bool,
    runtime: bool,
    from: Option<String>,
) -> ImportInfo {
    let path = strip_version(&package);
    let package_path = path.split('/').next().unwrap_or(&path).to_string();
    let alias = alias.unwrap_or_else(|| path.rsplit('/').next().unwrap_or(&path).to_string());
    ImportInfo {
        path,
        version: package
            .split_once('@')
            .map(|(_, version)| version.to_owned()),
        package: package_path,
        alias,
        macros,
        runtime,
        from,
    }
}
