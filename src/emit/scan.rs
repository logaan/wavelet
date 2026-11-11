use super::*;

#[derive(Default)]
pub(crate) struct Features {
    pub(crate) dep_calls: Vec<(String, String)>,
}

pub(crate) enum BoundArgs {
    PerParam(Vec<NodeId>),

    Bundle,
}

pub(crate) fn scan(arena: &Arena, id: NodeId, feats: &mut Features, info: &FileInfo) {
    match arena.node(id) {
        Node::Tup(items) => {
            if let Some(&head) = items.first()
                && let Node::Sym(name) = arena.node(head)
                && let Some((alias, name)) = crate::wit::qualified_name(info, name)
            {
                let key = (alias.clone(), name.clone());
                if !feats.dep_calls.contains(&key) {
                    feats.dep_calls.push(key);
                }
            }
            for &x in items {
                scan(arena, x, feats, info);
            }
        }
        Node::Lst(xs) => {
            for &x in xs {
                scan(arena, x, feats, info);
            }
        }
        Node::Rec(fields) => {
            for (_, v) in fields {
                scan(arena, *v, feats, info);
            }
        }
        _ => {}
    }
}

pub(crate) fn features_of(arena: &Arena, info: &FileInfo) -> Features {
    let mut feats = Features::default();
    for (params, body) in info.defs.values() {
        let _ = params;
        scan(arena, *body, &mut feats, info);
    }
    for (_, expr) in &info.value_defs {
        scan(arena, *expr, &mut feats, info);
    }
    feats
}
