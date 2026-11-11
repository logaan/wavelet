use std::rc::Rc;

use crate::form::{Arena, Node, NodeId};
use crate::interp::Interp;
use crate::value::{Env, Value};

fn macro_env(src: &str) -> Result<(Env, Rc<Arena>), String> {
    let (arena, roots) = crate::reader::read_file(src).map_err(|e| e.to_string())?;
    let arena = Rc::new(arena);
    let interp = Interp::new();
    let env = Env::root();
    crate::builtins::install(&env);
    for &root in &roots {
        if is_def_macro(&arena, root) {
            interp.eval(&arena, root, &env).map_err(|e| e.to_string())?;
        }
    }
    Ok((env, arena))
}

fn is_def_macro(arena: &Arena, id: NodeId) -> bool {
    if let Node::Tup(items) = arena.node(id)
        && let Some(&head) = items.first()
    {
        return matches!(arena.node(head), Node::Sym(s) if s == "defmacro-MACRO");
    }
    false
}

pub fn manifest(src: &str) -> Result<Vec<(String, u32)>, String> {
    let (arena, roots) = crate::reader::read_file(src).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for &root in &roots {
        let Node::Tup(items) = arena.node(root) else {
            continue;
        };
        if items.len() != 4 {
            continue;
        }
        let Node::Sym(h) = arena.node(items[0]) else {
            continue;
        };
        if h != "defmacro-MACRO" {
            continue;
        }
        let Node::Sym(name) = arena.node(items[1]) else {
            continue;
        };
        let arity = match arena.node(items[2]) {
            Node::Flg(names) => names.len(),
            Node::Rec(fields) => fields.len(),
            _ => continue,
        };
        out.push((name.clone(), arity as u32));
    }
    Ok(out)
}

pub fn expand_one(
    src: &str,
    name: &str,
    call_arena: Arena,
    call_id: NodeId,
) -> Result<(Arena, NodeId), String> {
    let (env, _src_arena) = macro_env(src)?;
    let key = format!("{name}-MACRO");
    let Some(Value::Macro(mac)) = env.lookup(&key) else {
        return Err(format!("unknown macro `{name}`"));
    };

    let Node::Tup(items) = call_arena.node(call_id) else {
        return Err("args tree root is not a call (tup)".to_string());
    };
    let args: Vec<NodeId> = items[1..].to_vec();

    let call_arena = Rc::new(call_arena);
    let interp = Interp::new();
    let (out, root) = interp
        .expand_once(&mac, &call_arena, &args)
        .map_err(|e| format!("expanding `{name}`: {e}"))?;

    let out = Rc::try_unwrap(out)
        .map_err(|_| "internal: expansion arena unexpectedly shared".to_string())?;
    Ok((out, root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printer::print;

    const LIB: &str = "\
Package \"demo:macros@0.1.0\"\n\
DefMacro identity {x} x\n\
DefMacro unless {cond body}\n\
  Quasi If Unquote(cond) {} Unquote(body)\n\
";

    #[test]
    fn manifest_reports_unsuffixed_names_and_arities() {
        let mut got = manifest(LIB).expect("manifest");
        got.sort();
        assert_eq!(
            got,
            vec![("identity".to_string(), 1u32), ("unless".to_string(), 2u32)]
        );
    }

    fn call(src: &str) -> (Arena, NodeId) {
        let (arena, roots) = crate::reader::read_file(src).expect("read call");
        (arena, *roots.last().unwrap())
    }

    #[test]
    fn expand_one_identity_returns_argument() {
        let (a, id) = call(&format!("{LIB}identity(add(1 2))"));
        let (out, root) = expand_one(LIB, "identity", a, id).expect("identity expands");
        assert_eq!(print(&out, root), "(add, 1, 2)");
    }

    #[test]
    fn expand_one_unless_expands_via_quasiquote() {
        let (a, id) = call(&format!("{LIB}unless(false \"ran\")"));
        let (out, root) = expand_one(LIB, "unless", a, id).expect("unless expands");
        assert_eq!(print(&out, root), r#"(if-MACRO, false, {}, "ran")"#);
    }

    #[test]
    fn expand_one_unknown_macro_errors() {
        let (a, id) = call(&format!("{LIB}nope()"));
        let err = expand_one(LIB, "nope", a, id).expect_err("unknown errors");
        assert!(err.contains("nope"), "unexpected: {err}");
    }

    #[test]
    fn expand_one_matches_local_expand_file() {
        let src = format!("{LIB}Identity add(1 2)");
        let (arena, roots) = crate::reader::read_file(&src).expect("read");
        let (out, new_roots) =
            crate::expand::expand_file(arena, &roots, None).expect("local expand");
        let local = print(&out, *new_roots.last().unwrap());

        let (a, id) = call(&src);
        let (cout, croot) = expand_one(LIB, "identity", a, id).expect("expand_one");
        assert_eq!(print(&cout, croot), local);
    }
}
