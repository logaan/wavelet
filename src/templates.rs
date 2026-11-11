use std::collections::{HashMap, HashSet};

use crate::form::{Arena, Node, NodeId};
use crate::lexer::ReadError;

struct Template {
    parameters: Vec<String>,
    declarations: Vec<NodeId>,
    names: HashSet<String>,
}

fn form<'a>(arena: &'a Arena, id: NodeId, name: &str) -> Option<&'a [NodeId]> {
    let Node::Tup(items) = arena.node(id) else {
        return None;
    };
    let (&head, rest) = items.split_first()?;
    matches!(arena.node(head), Node::Sym(s) if s == name).then_some(rest)
}

fn error(arena: &Arena, id: NodeId, msg: impl Into<String>) -> ReadError {
    ReadError {
        msg: msg.into(),
        at: arena.span(id).0,
    }
}

fn declaration_name(arena: &Arena, id: NodeId) -> Option<&str> {
    for head in ["def-MACRO", "deftype-MACRO", "defresource-MACRO"] {
        if let Some([name, _]) = form(arena, id, head)
            && let Node::Sym(name) = arena.node(*name)
        {
            return Some(name);
        }
    }
    None
}

pub fn instantiate(
    mut arena: Arena,
    roots: Vec<NodeId>,
) -> Result<(Arena, Vec<NodeId>), ReadError> {
    let mut templates = HashMap::new();
    let mut occupied: HashSet<String> = roots
        .iter()
        .filter_map(|&id| declaration_name(&arena, id).map(str::to_owned))
        .collect();
    for &root in &roots {
        let Some(parts) = form(&arena, root, "deffunctor-MACRO") else {
            continue;
        };
        let [name, parameters, body] = parts else {
            return Err(error(
                &arena,
                root,
                "DefFunctor expects a package string, type parameters, and declarations",
            ));
        };
        let Node::Str(name) = arena.node(*name) else {
            return Err(error(&arena, root, "DefFunctor package must be a string"));
        };
        if name.starts_with("wavelet:") {
            return Err(error(
                &arena,
                root,
                "DefFunctor cannot replace a built-in wavelet package",
            ));
        }
        let Node::Flg(parameters) = arena.node(*parameters) else {
            return Err(error(
                &arena,
                root,
                "DefFunctor parameters must be a flags literal such as {elem}",
            ));
        };
        if parameters.is_empty()
            || parameters.iter().collect::<HashSet<_>>().len() != parameters.len()
        {
            return Err(error(
                &arena,
                root,
                "DefFunctor requires distinct type parameter names",
            ));
        }
        let Node::Lst(declarations) = arena.node(*body) else {
            return Err(error(
                &arena,
                root,
                "DefFunctor body must be a list of declarations",
            ));
        };
        let mut names = HashSet::new();
        for &declaration in declarations {
            if let Some(name) = declaration_name(&arena, declaration) {
                if parameters.iter().any(|p| p == name) || !names.insert(name.to_owned()) {
                    return Err(error(
                        &arena,
                        declaration,
                        format!("duplicate or parameter-shadowing template declaration `{name}`"),
                    ));
                }
            } else if form(&arena, declaration, "export-MACRO").is_none() {
                return Err(error(
                    &arena,
                    declaration,
                    "DefFunctor supports Def, DefType, DefResource, and Export declarations",
                ));
            }
        }
        let template = Template {
            parameters: parameters.clone(),
            declarations: declarations.clone(),
            names,
        };
        if templates.insert(name.clone(), template).is_some() {
            return Err(error(
                &arena,
                root,
                format!("duplicate DefFunctor `{name}`"),
            ));
        }
    }
    if templates.is_empty() {
        return Ok((arena, roots));
    }
    let mut output = Vec::new();
    for root in roots {
        if form(&arena, root, "deffunctor-MACRO").is_some() {
            continue;
        }
        let Some([spec]) = form(&arena, root, "instantiate-MACRO") else {
            output.push(root);
            continue;
        };
        let Node::Rec(fields) = arena.node(*spec) else {
            return Err(error(&arena, root, "Instantiate expects a record"));
        };
        let field = |key| {
            fields
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, value)| *value)
        };
        let Some(package) = field("pkg") else {
            return Err(error(&arena, root, "Instantiate requires pkg"));
        };
        let Node::Str(package) = arena.node(package) else {
            return Err(error(&arena, root, "Instantiate pkg must be a string"));
        };
        let Some(template) = templates.get(package) else {
            output.push(root);
            continue;
        };
        let Some(alias) = field("as") else {
            return Err(error(&arena, root, "Instantiate requires an as alias"));
        };
        let Node::Sym(alias) = arena.node(alias) else {
            return Err(error(&arena, root, "Instantiate as must be a name"));
        };
        let Some(arguments) = field("with") else {
            return Err(error(
                &arena,
                root,
                "Instantiate requires with type arguments",
            ));
        };
        let Node::Rec(arguments) = arena.node(arguments) else {
            return Err(error(
                &arena,
                root,
                "Instantiate with must be a type argument record",
            ));
        };
        if arguments.len() != template.parameters.len()
            || template
                .parameters
                .iter()
                .any(|p| arguments.iter().filter(|(n, _)| n == p).count() != 1)
        {
            return Err(error(
                &arena,
                root,
                format!(
                    "Instantiate requires exactly these type arguments: {}",
                    template.parameters.join(", ")
                ),
            ));
        }
        for (_, ty) in arguments {
            crate::wit::type_text(&arena, *ty).map_err(|msg| error(&arena, *ty, msg))?;
        }
        let replacements: HashMap<String, NodeId> = arguments.iter().cloned().collect();
        let renames: HashMap<String, String> = template
            .names
            .iter()
            .map(|name| (name.clone(), format!("{alias}-{name}")))
            .collect();
        for name in renames.values() {
            if !occupied.insert(name.clone()) {
                return Err(error(
                    &arena,
                    root,
                    format!("Instantiate generated duplicate name `{name}`"),
                ));
            }
        }
        for &declaration in &template.declarations {
            output.push(copy(
                &mut arena,
                declaration,
                &replacements,
                &renames,
                &HashSet::new(),
            )?);
        }
    }
    Ok((arena, output))
}

fn copy(
    arena: &mut Arena,
    id: NodeId,
    replacements: &HashMap<String, NodeId>,
    renames: &HashMap<String, String>,
    bound: &HashSet<String>,
) -> Result<NodeId, ReadError> {
    let span = arena.span(id);
    let node = arena.node(id).clone();
    let node = match node {
        Node::Sym(name) if !bound.contains(&name) => {
            if let Some(&replacement) = replacements.get(&name) {
                return Ok(replacement);
            }
            let renamed = renames.get(&name).cloned().or_else(|| {
                renames
                    .iter()
                    .filter_map(|(old, new)| {
                        name.strip_prefix(&format!("{old}-"))
                            .map(|suffix| (old.len(), format!("{new}-{suffix}")))
                    })
                    .max_by_key(|(len, _)| *len)
                    .map(|(_, name)| name)
            });
            Node::Sym(renamed.unwrap_or(name))
        }
        Node::Tup(items) => {
            let mut nested = bound.clone();
            if items.len() == 3
                && matches!(arena.node(items[0]), Node::Sym(s) if s == "fn-MACRO" || s == "let-MACRO")
            {
                match arena.node(items[1]) {
                    Node::Rec(fields) => nested.extend(fields.iter().map(|(name, _)| name.clone())),
                    Node::Flg(names) => nested.extend(names.iter().cloned()),
                    _ => {}
                }
                let head = copy(arena, items[0], replacements, renames, bound)?;
                let params = copy(arena, items[1], replacements, renames, bound)?;
                let body = copy(arena, items[2], replacements, renames, &nested)?;
                Node::Tup(vec![head, params, body])
            } else {
                Node::Tup(
                    items
                        .into_iter()
                        .map(|child| copy(arena, child, replacements, renames, bound))
                        .collect::<Result<_, _>>()?,
                )
            }
        }
        Node::Lst(items) => Node::Lst(
            items
                .into_iter()
                .map(|child| copy(arena, child, replacements, renames, bound))
                .collect::<Result<_, _>>()?,
        ),
        Node::Rec(fields) => Node::Rec(
            fields
                .into_iter()
                .map(|(name, child)| Ok((name, copy(arena, child, replacements, renames, bound)?)))
                .collect::<Result<_, ReadError>>()?,
        ),
        other => other,
    };
    Ok(arena.add(node, span))
}
