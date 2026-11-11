use crate::host::{HostComponent, Val};
use crate::meta::{Node, Tree};

const MACROS_INTERFACE: &str = "wavelet:meta/macros@0.1.0";

pub fn node_to_val(node: &Node) -> Val {
    let (case, payload): (&str, Val) = match node {
        Node::BoolVal(b) => ("bool-val", Val::Bool(*b)),
        Node::IntVal(n) => ("int-val", Val::S64(*n)),
        Node::DecVal(d) => ("dec-val", Val::Float64(*d)),
        Node::CharVal(c) => ("char-val", Val::Char(*c)),
        Node::StrVal(s) => ("str-val", Val::String(s.clone())),
        Node::Sym(s) => ("sym", Val::String(s.clone())),
        Node::Tup(ids) => ("tup", id_list_val(ids)),
        Node::Lst(ids) => ("lst", id_list_val(ids)),
        Node::Rec(fields) => (
            "rec",
            Val::List(
                fields
                    .iter()
                    .map(|(k, v)| Val::Tuple(vec![Val::String(k.clone()), Val::U32(*v)]))
                    .collect(),
            ),
        ),
        Node::Flg(names) => (
            "flg",
            Val::List(names.iter().map(|n| Val::String(n.clone())).collect()),
        ),
    };
    Val::Variant(case.to_string(), Some(Box::new(payload)))
}

fn id_list_val(ids: &[crate::form::NodeId]) -> Val {
    Val::List(ids.iter().map(|id| Val::U32(*id)).collect())
}

pub fn tree_to_val(tree: &Tree) -> Val {
    let nodes = Val::List(tree.nodes.iter().map(node_to_val).collect());
    let spans = Val::List(
        tree.spans
            .iter()
            .map(|(s, e)| Val::Tuple(vec![Val::U32(*s), Val::U32(*e)]))
            .collect(),
    );
    Val::Record(vec![
        ("nodes".to_string(), nodes),
        ("root".to_string(), Val::U32(tree.root)),
        ("spans".to_string(), spans),
    ])
}

pub fn val_to_node(val: &Val) -> Result<Node, String> {
    let Val::Variant(case, payload) = val else {
        return Err(format!("expected a `node` variant, got {val:?}"));
    };
    let payload = payload
        .as_deref()
        .ok_or_else(|| format!("`node` case `{case}` is missing its payload"))?;
    let node = match case.as_str() {
        "bool-val" => Node::BoolVal(expect_bool(payload, case)?),
        "int-val" => Node::IntVal(expect_s64(payload, case)?),
        "dec-val" => Node::DecVal(expect_f64(payload, case)?),
        "char-val" => Node::CharVal(expect_char(payload, case)?),
        "str-val" => Node::StrVal(expect_string(payload, case)?),
        "sym" => Node::Sym(expect_string(payload, case)?),
        "tup" => Node::Tup(expect_id_list(payload, case)?),
        "lst" => Node::Lst(expect_id_list(payload, case)?),
        "rec" => Node::Rec(expect_rec_fields(payload, case)?),
        "flg" => Node::Flg(expect_string_list(payload, case)?),
        other => return Err(format!("unknown `node` variant case `{other}`")),
    };
    Ok(node)
}

pub fn val_to_tree(val: &Val) -> Result<Tree, String> {
    let Val::Record(fields) = val else {
        return Err(format!("expected a `tree` record, got {val:?}"));
    };
    let mut nodes_v = None;
    let mut root_v = None;
    let mut spans_v = None;
    for (name, v) in fields {
        match name.as_str() {
            "nodes" => nodes_v = Some(v),
            "root" => root_v = Some(v),
            "spans" => spans_v = Some(v),
            _ => {}
        }
    }
    let nodes_v = nodes_v.ok_or("`tree` record missing field `nodes`")?;
    let root_v = root_v.ok_or("`tree` record missing field `root`")?;
    let spans_v = spans_v.ok_or("`tree` record missing field `spans`")?;

    let Val::List(node_vals) = nodes_v else {
        return Err(format!("`tree.nodes` is not a list, got {nodes_v:?}"));
    };
    let nodes = node_vals
        .iter()
        .map(val_to_node)
        .collect::<Result<Vec<_>, _>>()?;

    let root = expect_u32(root_v, "tree.root")?;

    let Val::List(span_vals) = spans_v else {
        return Err(format!("`tree.spans` is not a list, got {spans_v:?}"));
    };
    let spans = span_vals
        .iter()
        .map(|v| {
            let Val::Tuple(parts) = v else {
                return Err(format!("`tree.spans` element is not a tuple, got {v:?}"));
            };
            if parts.len() != 2 {
                return Err(format!(
                    "`tree.spans` tuple has {} elements, expected 2",
                    parts.len()
                ));
            }
            Ok((
                expect_u32(&parts[0], "span.start")?,
                expect_u32(&parts[1], "span.end")?,
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Tree { nodes, root, spans })
}

pub fn val_to_result_tree(val: &Val) -> Result<Tree, String> {
    let Val::Result(r) = val else {
        return Err(format!("expected a `result<tree, string>`, got {val:?}"));
    };
    match r {
        Ok(Some(payload)) => val_to_tree(payload),
        Ok(None) => Err("`expand` returned `ok` with no `tree` payload".to_string()),
        Err(Some(payload)) => {
            let msg = expect_string(payload, "result::err")?;
            Err(msg)
        }
        Err(None) => Err("`expand` returned `err` with no message".to_string()),
    }
}

fn expect_bool(v: &Val, ctx: &str) -> Result<bool, String> {
    match v {
        Val::Bool(b) => Ok(*b),
        _ => Err(format!("`{ctx}` payload is not a bool, got {v:?}")),
    }
}

fn expect_s64(v: &Val, ctx: &str) -> Result<i64, String> {
    match v {
        Val::S64(n) => Ok(*n),
        _ => Err(format!("`{ctx}` payload is not an s64, got {v:?}")),
    }
}

fn expect_f64(v: &Val, ctx: &str) -> Result<f64, String> {
    match v {
        Val::Float64(d) => Ok(*d),
        _ => Err(format!("`{ctx}` payload is not a float64, got {v:?}")),
    }
}

fn expect_char(v: &Val, ctx: &str) -> Result<char, String> {
    match v {
        Val::Char(c) => Ok(*c),
        _ => Err(format!("`{ctx}` payload is not a char, got {v:?}")),
    }
}

fn expect_string(v: &Val, ctx: &str) -> Result<String, String> {
    match v {
        Val::String(s) => Ok(s.clone()),
        _ => Err(format!("`{ctx}` payload is not a string, got {v:?}")),
    }
}

fn expect_u32(v: &Val, ctx: &str) -> Result<u32, String> {
    match v {
        Val::U32(n) => Ok(*n),
        _ => Err(format!("`{ctx}` is not a u32, got {v:?}")),
    }
}

fn expect_id_list(v: &Val, ctx: &str) -> Result<Vec<crate::form::NodeId>, String> {
    let Val::List(items) = v else {
        return Err(format!("`{ctx}` payload is not a list, got {v:?}"));
    };
    items.iter().map(|i| expect_u32(i, ctx)).collect()
}

fn expect_string_list(v: &Val, ctx: &str) -> Result<Vec<String>, String> {
    let Val::List(items) = v else {
        return Err(format!("`{ctx}` payload is not a list, got {v:?}"));
    };
    items.iter().map(|i| expect_string(i, ctx)).collect()
}

fn expect_rec_fields(v: &Val, ctx: &str) -> Result<Vec<(String, crate::form::NodeId)>, String> {
    let Val::List(items) = v else {
        return Err(format!("`{ctx}` payload is not a list, got {v:?}"));
    };
    items
        .iter()
        .map(|item| {
            let Val::Tuple(parts) = item else {
                return Err(format!("`{ctx}` field is not a tuple, got {item:?}"));
            };
            if parts.len() != 2 {
                return Err(format!(
                    "`{ctx}` field tuple has {} elements, expected 2",
                    parts.len()
                ));
            }
            Ok((expect_string(&parts[0], ctx)?, expect_u32(&parts[1], ctx)?))
        })
        .collect()
}

pub struct MacroComponent {
    host: HostComponent,
}

impl MacroComponent {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let host = HostComponent::from_bytes(bytes)?;
        let mut me = MacroComponent { host };
        me.check_interface()?;
        Ok(me)
    }

    pub fn from_file(path: &std::path::Path) -> Result<Self, String> {
        let host = HostComponent::from_file(path)?;
        let mut me = MacroComponent { host };
        me.check_interface()?;
        Ok(me)
    }

    fn check_interface(&mut self) -> Result<(), String> {
        self.host.instance_func(MACROS_INTERFACE, "manifest")?;
        self.host.instance_func(MACROS_INTERFACE, "expand")?;
        Ok(())
    }

    pub fn manifest(&mut self) -> Result<Vec<(String, u32)>, String> {
        let out = self.host.call_instance(MACROS_INTERFACE, "manifest", &[])?;
        let [Val::List(items)] = out.as_slice() else {
            return Err(format!(
                "`manifest` returned an unexpected result shape: {out:?}"
            ));
        };
        items
            .iter()
            .map(|item| {
                let Val::Tuple(parts) = item else {
                    return Err(format!(
                        "`manifest` entry is not a (string, u32) tuple: {item:?}"
                    ));
                };
                if parts.len() != 2 {
                    return Err(format!(
                        "`manifest` entry tuple has {} elements, expected 2",
                        parts.len()
                    ));
                }
                let name = expect_string(&parts[0], "manifest.name")?;
                let arity = expect_u32(&parts[1], "manifest.arity")?;
                Ok((name, arity))
            })
            .collect()
    }

    pub fn expand(&mut self, name: &str, args: &Tree) -> Result<Tree, String> {
        let args_val = tree_to_val(args);
        let out = self.host.call_instance(
            MACROS_INTERFACE,
            "expand",
            &[Val::String(name.to_string()), args_val],
        )?;
        let [result_val] = out.as_slice() else {
            return Err(format!(
                "`expand` returned an unexpected result shape: {out:?}"
            ));
        };
        val_to_result_tree(result_val)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form::NodeId;
    use crate::meta::{arena_to_tree, tree_to_arena};
    use crate::printer::print;
    use crate::reader::read_file;

    fn fixture() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/macros.wasm"
        ))
        .expect("fixture macros.wasm present (see tests/fixtures/macros/README)")
    }

    fn load() -> MacroComponent {
        MacroComponent::from_bytes(&fixture()).expect("fixture is a macro component")
    }

    fn args_tree(call_src: &str) -> Tree {
        let (arena, roots) = read_file(call_src).expect("read call");
        assert_eq!(roots.len(), 1);

        arena_to_tree(&arena, roots[0])
    }

    fn print_tree(tree: &Tree) -> String {
        let (arena, root) = tree_to_arena(tree);
        print(&arena, root)
    }

    #[test]
    fn manifest_lists_name_arity_pairs() {
        let mut m = load();
        let mut got = m.manifest().expect("manifest call");
        got.sort();
        assert_eq!(
            got,
            vec![
                ("boom".to_string(), 0u32),
                ("identity".to_string(), 1u32),
                ("unless".to_string(), 2u32),
            ]
        );
    }

    #[test]
    fn expand_identity_returns_argument_unchanged() {
        let mut m = load();
        let args = args_tree("identity(add(1 2))");
        let out = m.expand("identity", &args).expect("identity expands");
        assert_eq!(print_tree(&out), "(add, 1, 2)");
    }

    #[test]
    fn expand_unless_rewrites_to_if() {
        let mut m = load();
        let args = args_tree(r#"unless(false "ran")"#);
        let out = m.expand("unless", &args).expect("unless expands");
        assert_eq!(print_tree(&out), r#"(if-MACRO, false, {}, "ran")"#);
    }

    #[test]
    fn expand_error_surfaces_message() {
        let mut m = load();
        let args = args_tree("boom()");
        let err = m.expand("boom", &args).expect_err("boom errors");
        assert!(err.contains("boom"), "unexpected error: {err}");
    }

    #[test]
    fn expand_unknown_macro_is_an_error() {
        let mut m = load();
        let args = args_tree("nope()");
        let err = m.expand("nope", &args).expect_err("unknown macro errors");
        assert!(!err.is_empty(), "expected a non-empty error");
    }

    fn assert_node_roundtrips(node: Node) {
        let val = node_to_val(&node);
        let back = val_to_node(&val).expect("node lifts back");
        assert_eq!(back, node, "node round trip diverged via {val:?}");
    }

    #[test]
    fn every_node_variant_roundtrips_through_val() {
        assert_node_roundtrips(Node::BoolVal(true));
        assert_node_roundtrips(Node::BoolVal(false));
        assert_node_roundtrips(Node::IntVal(-7));
        assert_node_roundtrips(Node::IntVal(i64::MAX));
        assert_node_roundtrips(Node::DecVal(2.75));
        assert_node_roundtrips(Node::CharVal('☃'));
        assert_node_roundtrips(Node::StrVal("hi\tthere".to_string()));
        assert_node_roundtrips(Node::Sym("foo-bar".to_string()));
        assert_node_roundtrips(Node::Tup(vec![0, 1, 2]));
        assert_node_roundtrips(Node::Tup(vec![]));
        assert_node_roundtrips(Node::Lst(vec![3, 4]));
        assert_node_roundtrips(Node::Rec(vec![("a".to_string(), 0), ("b".to_string(), 1)]));
        assert_node_roundtrips(Node::Flg(vec!["read".to_string(), "write".to_string()]));
        assert_node_roundtrips(Node::Flg(vec![]));
    }

    #[test]
    fn whole_tree_roundtrips_through_val() {
        let src = r#"(a [b {c: (d dsl/elem) g: [1 2.0]}] "s" 'z' {read write})"#;
        let (arena, roots) = read_file(src).expect("read");
        let tree = arena_to_tree(&arena, roots[0]);
        let val = tree_to_val(&tree);
        let back = val_to_tree(&val).expect("tree lifts back");
        assert_eq!(back, tree, "tree round trip diverged");
    }

    #[test]
    fn result_ok_and_err_lift() {
        let tree = Tree {
            nodes: vec![Node::Sym("x".to_string())],
            root: 0 as NodeId,
            spans: vec![(0, 1)],
        };
        let ok_val = Val::Result(Ok(Some(Box::new(tree_to_val(&tree)))));
        assert_eq!(val_to_result_tree(&ok_val).unwrap(), tree);

        let err_val = Val::Result(Err(Some(Box::new(Val::String("boom".into())))));
        assert_eq!(
            val_to_result_tree(&err_val).unwrap_err(),
            "boom".to_string()
        );
    }
}
