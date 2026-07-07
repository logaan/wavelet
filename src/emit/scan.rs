//! Pre-emission feature scan over a file's form trees: collects the
//! cross-component dep calls whose imports the core module must declare.

use super::*;

// ------------------------------------------------------------ feature scan

#[derive(Default)]
pub(crate) struct Features {
    /// unique (alias, func) cross-component calls, in first-use order
    pub(crate) dep_calls: Vec<(String, String)>,
}

/// Result of binding a call's argument forms to a callee's parameters.
pub(crate) enum BoundArgs {
    /// one argument form per parameter, in parameter order
    PerParam(Vec<NodeId>),
    /// the sole parameter receives every argument bundled as one tuple
    Bundle,
}

pub(crate) fn scan(arena: &Arena, id: NodeId, feats: &mut Features) {
    match arena.node(id) {
        // A call is a tuple whose head (items[0]) may be a cross-component
        // (Qsym) dependency; recurse over every element either way.
        Node::Tup(items) => {
            if let Some(&head) = items.first()
                && let Node::Qsym(alias, name) = arena.node(head)
            {
                let key = (alias.clone(), name.clone());
                if !feats.dep_calls.contains(&key) {
                    feats.dep_calls.push(key);
                }
            }
            for &x in items {
                scan(arena, x, feats);
            }
        }
        Node::Lst(xs) => {
            for &x in xs {
                scan(arena, x, feats);
            }
        }
        Node::Rec(fields) => {
            for (_, v) in fields {
                scan(arena, *v, feats);
            }
        }
        _ => {}
    }
}

pub(crate) fn features_of(arena: &Arena, info: &FileInfo) -> Features {
    let mut feats = Features::default();
    for (params, body) in info.defs.values() {
        let _ = params;
        scan(arena, *body, &mut feats);
    }
    for (_, expr) in &info.value_defs {
        scan(arena, *expr, &mut feats);
    }
    feats
}
