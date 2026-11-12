use super::*;

pub(super) fn infer(arena: &Arena, sigs: &mut HashMap<String, Vec<Sig>>) {
    for _ in 0..sigs.len().max(1) {
        let previous = sigs.clone();
        let mut changed = false;
        for candidates in sigs.values_mut() {
            for signature in candidates {
                let Some(body) = signature.body else { continue };
                for index in 0..signature.params.len() {
                    if signature.params[index].1 != Type::Unknown {
                        continue;
                    }
                    let name = &signature.params[index].0;
                    let mut constraints = Vec::new();
                    collect(
                        arena,
                        body,
                        name,
                        &signature.params,
                        &previous,
                        &mut constraints,
                    );
                    let mut ty = Type::Unknown;
                    for candidate in constraints {
                        let Some(unified) = unify(&HashMap::new(), &ty, &candidate) else {
                            ty = Type::Unknown;
                            break;
                        };
                        ty = unified;
                    }
                    if ty != Type::Unknown {
                        signature.params[index].1 = ty;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
}

fn collect(
    arena: &Arena,
    id: NodeId,
    name: &str,
    params: &[(String, Type)],
    sigs: &HashMap<String, Vec<Sig>>,
    output: &mut Vec<Type>,
) {
    match arena.node(id) {
        Node::Tup(items) if !items.is_empty() => {
            if let Node::Sym(callee) = arena.node(items[0]) {
                if matches!(callee.as_str(), "quote-MACRO" | "quasi-MACRO" | "fn-MACRO") {
                    return;
                }
                if callee == "let-MACRO" && items.len() == 3 {
                    if let Node::Rec(fields) = arena.node(items[1]) {
                        for (binding, value) in fields {
                            collect(arena, *value, name, params, sigs, output);
                            if binding == name {
                                return;
                            }
                        }
                    }
                    collect(arena, items[2], name, params, sigs, output);
                    return;
                }
                for (index, &arg) in items[1..].iter().enumerate() {
                    if !matches!(arena.node(arg), Node::Sym(n) if n == name) {
                        continue;
                    }
                    let constraint = if let Some(candidates) = sigs.get(callee) {
                        if candidates.len() == 1 {
                            candidates[0].params.get(index).map(|(_, ty)| ty.clone())
                        } else {
                            None
                        }
                    } else {
                        match callee.as_str() {
                            "add" | "sub" | "mul" | "div" | "rem" | "neg" | "abs" | "min"
                            | "max" => {
                                let peer = items[1..]
                                    .iter()
                                    .filter(|&&other| other != arg)
                                    .find_map(|&other| match arena.node(other) {
                                        Node::Dec(_) => Some(Type::FloatLit),
                                        Node::Sym(n) => params
                                            .iter()
                                            .find(|(p, ty)| {
                                                p == n && ty.numeric() && *ty != Type::Unknown
                                            })
                                            .map(|(_, ty)| ty.clone()),
                                        _ => None,
                                    });
                                Some(peer.unwrap_or(Type::IntLit(None)))
                            }
                            "str-cat" | "split" => Some(Type::String),
                            "not" => Some(Type::Bool),
                            "eq" | "lt" | "le" | "gt" | "ge" => items[1..]
                                .iter()
                                .filter(|&&other| other != arg)
                                .find_map(|&other| match arena.node(other) {
                                    Node::Int(_) => Some(Type::IntLit(None)),
                                    Node::Dec(_) => Some(Type::FloatLit),
                                    Node::Str(_) => Some(Type::String),
                                    Node::Char(_) => Some(Type::Char),
                                    Node::Bool(_) => Some(Type::Bool),
                                    _ => None,
                                }),
                            _ => None,
                        }
                    };
                    if let Some(ty) = constraint {
                        output.push(ty)
                    }
                }
            }
            for &child in items {
                collect(arena, child, name, params, sigs, output)
            }
        }
        Node::Lst(items) => {
            for &child in items {
                collect(arena, child, name, params, sigs, output)
            }
        }
        Node::Rec(fields) => {
            for (_, child) in fields {
                collect(arena, *child, name, params, sigs, output)
            }
        }
        _ => {}
    }
}
