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

        if let Some(derived) = derive_roots(&arena, root, &mut out, &resources)? {
            new_roots.extend(derived);
            continue;
        }
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

fn derive_roots(
    arena: &Arena,
    root: NodeId,
    out: &mut Arena,
    resources: &std::collections::HashSet<String>,
) -> Result<Option<Vec<NodeId>>, String> {
    let Node::Tup(items) = arena.node(root) else {
        return Ok(None);
    };
    let Some(&head) = items.first() else {
        return Ok(None);
    };
    if !matches!(arena.node(head), Node::Sym(s) if s == "derive-MACRO") {
        return Ok(None);
    }
    let (classes, type_id) = match (items.get(1), items.get(2)) {
        (Some(&c), Some(&t)) => (c, t),
        _ => return Err("malformed Derive: expected `Derive {classes} typename`".into()),
    };
    let Node::Flg(class_names) = arena.node(classes) else {
        return Err("malformed Derive: the classes must be a `{…}` flags form".into());
    };
    let Node::Sym(tname) = arena.node(type_id) else {
        return Err("malformed Derive: the type must be a bare type name".into());
    };

    if resources.contains(tname) {
        return Err(format!(
            "`Derive` cannot be used on the resource type `{tname}`: resources have              identity equality and are opaque (not `Eq`/`Ord`/`Hash`/`Show`)"
        ));
    }

    let span = arena.span(root);
    let mut roots = Vec::new();
    for class in class_names {
        let (op, params, body) = derived_op(out, class, tname, span)?;
        let mangled = format!("{op}-{tname}");
        roots.push(make_def(out, &mangled, params, body, span));
        roots.push(make_export(out, &mangled, span));
    }
    Ok(Some(roots))
}

fn derived_op(
    out: &mut Arena,
    class: &str,
    tname: &str,
    span: (u32, u32),
) -> Result<(&'static str, NodeId, NodeId), String> {
    let ty = |out: &mut Arena| out.add(Node::Sym(tname.to_string()), span);
    match class {
        "Eq" => {
            let (a, b) = (ty(out), ty(out));
            let params = out.add(Node::Rec(vec![("a".into(), a), ("b".into(), b)]), span);
            let body = out.add(Node::Bool(true), span);
            Ok(("eq", params, body))
        }

        "Ord" => {
            let (a, b) = (ty(out), ty(out));
            let params = out.add(Node::Rec(vec![("a".into(), a), ("b".into(), b)]), span);
            let body = the_int(out, "s32", 0, span);
            Ok(("compare", params, body))
        }

        "Show" => {
            let v = ty(out);
            let params = out.add(Node::Rec(vec![("v".into(), v)]), span);
            let body = out.add(Node::Str(String::new()), span);
            Ok(("show", params, body))
        }

        "Hash" => {
            let v = ty(out);
            let params = out.add(Node::Rec(vec![("v".into(), v)]), span);
            let body = the_int(out, "u32", 0, span);
            Ok(("hash", params, body))
        }
        other => Err(format!(
            "unknown derive class `{other}` (expected Eq, Ord, Show, or Hash)"
        )),
    }
}

fn the_int(out: &mut Arena, ty: &str, n: i64, span: (u32, u32)) -> NodeId {
    let head = out.add(Node::Sym("the-MACRO".into()), span);
    let ty_id = out.add(Node::Sym(ty.into()), span);
    let n_id = out.add(Node::Int(n), span);
    out.add(Node::Tup(vec![head, ty_id, n_id]), span)
}

fn make_def(out: &mut Arena, name: &str, params: NodeId, body: NodeId, span: (u32, u32)) -> NodeId {
    let fn_head = out.add(Node::Sym("fn-MACRO".into()), span);
    let fn_form = out.add(Node::Tup(vec![fn_head, params, body]), span);
    let def_head = out.add(Node::Sym("def-MACRO".into()), span);
    let name_id = out.add(Node::Sym(name.to_string()), span);
    out.add(Node::Tup(vec![def_head, name_id, fn_form]), span)
}

fn make_export(out: &mut Arena, name: &str, span: (u32, u32)) -> NodeId {
    let head = out.add(Node::Sym("export-MACRO".into()), span);
    let name_id = out.add(Node::Sym(name.to_string()), span);
    out.add(Node::Tup(vec![head, name_id]), span)
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
