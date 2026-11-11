use std::rc::Rc;

use crate::form::{Arena, Node, NodeId};
use crate::interp::Interp;
use crate::value::{Env, Value};

pub trait ForeignExpander {
    fn expand_call(
        &mut self,
        alias: Option<&str>,
        name: &str,
        arena: &Arena,
        call_id: NodeId,
    ) -> Option<Result<(Arena, NodeId), String>>;
}

pub fn expand_file(
    arena: Arena,
    roots: &[NodeId],
    mut foreign: Option<&mut dyn ForeignExpander>,
) -> Result<(Arena, Vec<NodeId>), String> {
    let interp = Interp::new();
    let env = Env::root();
    crate::builtins::install(&env);

    let arena = Rc::new(arena);

    let resources = resource_names(&arena, roots);
    let mut out = Arena::new();
    let mut new_roots = Vec::new();
    for &root in roots {
        if is_def_macro(&arena, root) {
            interp.eval(&arena, root, &env).map_err(|e| e.to_string())?;
            continue;
        }

        reject_derive(&arena, root, &resources)?;
        new_roots.push(expand_form(
            &interp,
            &env,
            &arena,
            root,
            &mut out,
            &mut foreign,
        )?);
    }
    Ok((out, new_roots))
}

fn resource_names(arena: &Arena, roots: &[NodeId]) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for &root in roots {
        if let Node::Tup(items) = arena.node(root)
            && let Some(&head) = items.first()
            && matches!(arena.node(head), Node::Sym(s) if s == "defresource-MACRO")
            && let Some(&name_id) = items.get(1)
            && let Node::Sym(name) = arena.node(name_id)
        {
            out.insert(name.clone());
        }
    }
    out
}

fn is_def_macro(arena: &Arena, id: NodeId) -> bool {
    if let Node::Tup(items) = arena.node(id)
        && let Some(&head) = items.first()
    {
        return matches!(arena.node(head), Node::Sym(s) if s == "defmacro-MACRO");
    }
    false
}

fn reject_derive(
    arena: &Arena,
    root: NodeId,
    resources: &std::collections::HashSet<String>,
) -> Result<(), String> {
    let Node::Tup(items) = arena.node(root) else {
        return Ok(());
    };
    if !matches!(items.first().map(|id| arena.node(*id)), Some(Node::Sym(head)) if head == "derive-MACRO")
    {
        return Ok(());
    }
    let [_, classes, type_id] = items.as_slice() else {
        return Err("malformed Derive: expected `Derive {classes} typename`".into());
    };
    let Node::Flg(class_names) = arena.node(*classes) else {
        return Err("malformed Derive: the classes must be a flags form".into());
    };
    let Node::Sym(tname) = arena.node(*type_id) else {
        return Err("malformed Derive: the type must be a bare type name".into());
    };
    if resources.contains(tname) {
        return Err(format!(
            "`Derive` cannot be used on the resource type `{tname}`: resources have identity equality and are opaque"
        ));
    }
    for class in class_names {
        if !matches!(class.as_str(), "Eq" | "Ord" | "Show" | "Hash") {
            return Err(format!(
                "unknown derive class `{class}` (expected Eq, Ord, Show, or Hash)"
            ));
        }
    }
    Err("`Derive` is not implemented; define equality, ordering, hashing, and display functions explicitly".into())
}

fn expand_form(
    interp: &Interp,
    env: &Env,
    arena: &Rc<Arena>,
    id: NodeId,
    out: &mut Arena,
    foreign: &mut Option<&mut dyn ForeignExpander>,
) -> Result<NodeId, String> {
    if let Node::Tup(items) = arena.node(id)
        && let Some(&head) = items.first()
        && let Node::Sym(name) = arena.node(head)
    {
        if name == "quote-MACRO" || name == "quasi-MACRO" {
            return Ok(copy_form(arena, id, out));
        }

        if let Some(Value::Macro(mac)) = env.lookup(name) {
            let bare = name.trim_end_matches("-MACRO");
            if let Some(fx) = foreign.as_deref_mut()
                && let Some(result) = fx.expand_call(None, bare, arena, id)
            {
                let (expanded_arena, expanded) =
                    result.map_err(|e| format!("expanding `{bare}`: {e}"))?;
                let expanded_arena = Rc::new(expanded_arena);
                return expand_form(interp, env, &expanded_arena, expanded, out, foreign);
            }
            let (expanded_arena, expanded) = interp
                .expand_once(&mac, arena, &items[1..])
                .map_err(|e| format!("expanding `{bare}`: {e}"))?;
            return expand_form(interp, env, &expanded_arena, expanded, out, foreign);
        }

        if let Some(fx) = foreign.as_deref_mut() {
            let macro_name = name.trim_end_matches("-MACRO");
            if let Some(result) = fx.expand_call(None, macro_name, arena, id) {
                let (expanded_arena, expanded) =
                    result.map_err(|e| format!("expanding `{macro_name}`: {e}"))?;
                let expanded_arena = Rc::new(expanded_arena);
                return expand_form(interp, env, &expanded_arena, expanded, out, foreign);
            }
        }
    }
    descend(interp, env, arena, id, out, foreign)
}

fn descend(
    interp: &Interp,
    env: &Env,
    arena: &Rc<Arena>,
    id: NodeId,
    out: &mut Arena,
    foreign: &mut Option<&mut dyn ForeignExpander>,
) -> Result<NodeId, String> {
    let span = arena.span(id);
    let node = match arena.node(id).clone() {
        Node::Tup(items) => {
            let mut kids = Vec::with_capacity(items.len());
            for x in items {
                kids.push(expand_form(interp, env, arena, x, out, foreign)?);
            }
            Node::Tup(kids)
        }
        Node::Lst(items) => {
            let mut kids = Vec::with_capacity(items.len());
            for x in items {
                kids.push(expand_form(interp, env, arena, x, out, foreign)?);
            }
            Node::Lst(kids)
        }
        Node::Rec(fields) => {
            let mut nf = Vec::with_capacity(fields.len());
            for (k, v) in fields {
                nf.push((k, expand_form(interp, env, arena, v, out, foreign)?));
            }
            Node::Rec(nf)
        }
        leaf => leaf,
    };
    Ok(out.add(node, span))
}

fn copy_form(arena: &Arena, id: NodeId, out: &mut Arena) -> NodeId {
    let span = arena.span(id);
    let node = match arena.node(id).clone() {
        Node::Tup(items) => Node::Tup(items.iter().map(|&x| copy_form(arena, x, out)).collect()),
        Node::Lst(items) => Node::Lst(items.iter().map(|&x| copy_form(arena, x, out)).collect()),
        Node::Rec(fields) => Node::Rec(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), copy_form(arena, *v, out)))
                .collect(),
        ),
        leaf => leaf,
    };
    out.add(node, span)
}
