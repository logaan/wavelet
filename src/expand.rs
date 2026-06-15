//! Ahead-of-time macro expansion (§2.4): rewrite a file's form tree with all
//! user macros expanded, so later pipeline stages (WIT synthesis, the wasm
//! emitter) never see them. The interpreter still expands lazily at eval
//! time; this pass exists for the compile pipeline.
//!
//! `DefMacro` forms are evaluated (registering the macro) and dropped from
//! the output. Macro bodies run in an environment containing the builtins
//! and previously defined macros only — calling file-local *functions* at
//! expand time is future work (macro components, §6.3).

use std::rc::Rc;

use crate::form::{Arena, Node, NodeId};
use crate::interp::Interp;
use crate::value::{Env, Value};

pub fn expand_file(arena: Arena, roots: &[NodeId]) -> Result<(Arena, Vec<NodeId>), String> {
    let interp = Interp::new();
    let env = Env::root();
    crate::builtins::install(&env);

    let arena = Rc::new(arena);
    let mut out = Arena::new();
    let mut new_roots = Vec::new();
    for &root in roots {
        if is_def_macro(&arena, root) {
            interp.eval(&arena, root, &env).map_err(|e| e.to_string())?;
            continue;
        }
        new_roots.push(expand_form(&interp, &env, &arena, root, &mut out)?);
    }
    Ok((out, new_roots))
}

fn is_def_macro(arena: &Arena, id: NodeId) -> bool {
    if let Node::Call(head, _) = arena.node(id) {
        return matches!(arena.node(*head), Node::Sym(s) if s == "def-macro-MACRO");
    }
    false
}

fn expand_form(
    interp: &Interp,
    env: &Env,
    arena: &Rc<Arena>,
    id: NodeId,
    out: &mut Arena,
) -> Result<NodeId, String> {
    if let Node::Call(head, payload) = arena.node(id) {
        if let Node::Sym(name) = arena.node(*head) {
            // forms quoted at runtime are not expanded at compile time
            if name == "quote-MACRO" || name == "quasi-MACRO" {
                return Ok(copy_form(arena, id, out));
            }
            if let Some(Value::Macro(mac)) = env.lookup(name) {
                let (expanded_arena, expanded) = interp
                    .expand_once(&mac, arena, std::slice::from_ref(payload))
                    .map_err(|e| format!("expanding `{}`: {e}", name.trim_end_matches("-MACRO")))?;
                return expand_form(interp, env, &expanded_arena, expanded, out);
            }
        }
    }
    descend(interp, env, arena, id, out)
}

fn descend(
    interp: &Interp,
    env: &Env,
    arena: &Rc<Arena>,
    id: NodeId,
    out: &mut Arena,
) -> Result<NodeId, String> {
    let span = arena.span(id);
    let node = match arena.node(id).clone() {
        Node::Call(head, payload) => {
            let h = expand_form(interp, env, arena, head, out)?;
            let p = expand_form(interp, env, arena, payload, out)?;
            Node::Call(h, p)
        }
        Node::Tup(items) => Node::Tup(
            items
                .iter()
                .map(|&x| expand_form(interp, env, arena, x, out))
                .collect::<Result<_, _>>()?,
        ),
        Node::Lst(items) => Node::Lst(
            items
                .iter()
                .map(|&x| expand_form(interp, env, arena, x, out))
                .collect::<Result<_, _>>()?,
        ),
        Node::Rec(fields) => {
            let mut nf = Vec::with_capacity(fields.len());
            for (k, v) in fields {
                nf.push((k, expand_form(interp, env, arena, v, out)?));
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
        Node::Call(head, payload) => {
            let h = copy_form(arena, head, out);
            let p = copy_form(arena, payload, out);
            Node::Call(h, p)
        }
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
