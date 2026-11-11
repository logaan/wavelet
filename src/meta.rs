use crate::form::{Arena, Node as FormNode, NodeId};

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    BoolVal(bool),

    IntVal(i64),
    DecVal(f64),
    CharVal(char),
    StrVal(String),
    Sym(String),

    Tup(Vec<NodeId>),
    Lst(Vec<NodeId>),
    Rec(Vec<(String, NodeId)>),
    Flg(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub root: NodeId,

    pub spans: Vec<(u32, u32)>,
}

pub fn arena_to_tree(arena: &Arena, root: NodeId) -> Tree {
    let mut tree = Tree {
        nodes: Vec::new(),
        root: 0,
        spans: Vec::new(),
    };
    tree.root = copy_node(arena, root, &mut tree);
    tree
}

fn copy_node(arena: &Arena, id: NodeId, tree: &mut Tree) -> NodeId {
    let span = arena.span(id);
    let node = match arena.node(id) {
        FormNode::Bool(b) => Node::BoolVal(*b),
        FormNode::Int(n) => Node::IntVal(*n),
        FormNode::Dec(d) => Node::DecVal(*d),
        FormNode::Char(c) => Node::CharVal(*c),
        FormNode::Str(s) => Node::StrVal(s.clone()),
        FormNode::Sym(s) => Node::Sym(s.clone()),
        FormNode::Tup(items) => {
            let kids = copy_kids(arena, items, tree);
            Node::Tup(kids)
        }
        FormNode::Lst(items) => {
            let kids = copy_kids(arena, items, tree);
            Node::Lst(kids)
        }
        FormNode::Rec(fields) => {
            let kids = fields
                .iter()
                .map(|(k, v)| (k.clone(), copy_node(arena, *v, tree)))
                .collect();
            Node::Rec(kids)
        }
        FormNode::Flg(names) => Node::Flg(names.clone()),
    };
    push(tree, node, span)
}

fn copy_kids(arena: &Arena, ids: &[NodeId], tree: &mut Tree) -> Vec<NodeId> {
    ids.iter().map(|id| copy_node(arena, *id, tree)).collect()
}

fn push(tree: &mut Tree, node: Node, span: (u32, u32)) -> NodeId {
    let id = tree.nodes.len() as NodeId;
    tree.nodes.push(node);
    tree.spans.push(span);
    id
}

pub fn tree_to_arena(tree: &Tree) -> (Arena, NodeId) {
    let mut arena = Arena::new();
    for (node, span) in tree.nodes.iter().zip(tree.spans.iter()) {
        let form = match node {
            Node::BoolVal(b) => FormNode::Bool(*b),
            Node::IntVal(n) => FormNode::Int(*n),
            Node::DecVal(d) => FormNode::Dec(*d),
            Node::CharVal(c) => FormNode::Char(*c),
            Node::StrVal(s) => FormNode::Str(s.clone()),
            Node::Sym(s) => FormNode::Sym(s.clone()),
            Node::Tup(items) => FormNode::Tup(items.clone()),
            Node::Lst(items) => FormNode::Lst(items.clone()),
            Node::Rec(fields) => FormNode::Rec(fields.clone()),
            Node::Flg(names) => FormNode::Flg(names.clone()),
        };
        arena.add(form, *span);
    }
    (arena, tree.root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form::Node as FormNode;
    use crate::reader::read_file;

    fn forms_eq(arena_a: &Arena, a: NodeId, arena_b: &Arena, b: NodeId) -> bool {
        if arena_a.span(a) != arena_b.span(b) {
            return false;
        }
        match (arena_a.node(a), arena_b.node(b)) {
            (FormNode::Bool(x), FormNode::Bool(y)) => x == y,
            (FormNode::Int(x), FormNode::Int(y)) => x == y,
            (FormNode::Dec(x), FormNode::Dec(y)) => x == y,
            (FormNode::Char(x), FormNode::Char(y)) => x == y,
            (FormNode::Str(x), FormNode::Str(y)) => x == y,
            (FormNode::Sym(x), FormNode::Sym(y)) => x == y,
            (FormNode::Tup(xs), FormNode::Tup(ys)) | (FormNode::Lst(xs), FormNode::Lst(ys)) => {
                xs.len() == ys.len()
                    && xs
                        .iter()
                        .zip(ys)
                        .all(|(x, y)| forms_eq(arena_a, *x, arena_b, *y))
            }
            (FormNode::Rec(xs), FormNode::Rec(ys)) => {
                xs.len() == ys.len()
                    && xs.iter().zip(ys).all(|((kx, vx), (ky, vy))| {
                        kx == ky && forms_eq(arena_a, *vx, arena_b, *vy)
                    })
            }
            (FormNode::Flg(xs), FormNode::Flg(ys)) => xs == ys,
            _ => false,
        }
    }

    fn assert_roundtrips(src: &str) {
        let (arena, roots) = read_file(src).expect("read");
        assert!(!roots.is_empty(), "fixture produced no forms: {src:?}");
        for root in roots {
            let tree = arena_to_tree(&arena, root);
            assert_eq!(tree.nodes.len(), tree.spans.len(), "spans parallel nodes");
            assert!((tree.root as usize) < tree.nodes.len(), "root in bounds");
            let (arena2, root2) = tree_to_arena(&tree);
            assert!(
                forms_eq(&arena, root, &arena2, root2),
                "round trip diverged for root {root} in {src:?}",
            );
        }
    }

    #[test]
    fn roundtrip_atoms() {
        assert_roundtrips("true false 42 -7 3.14 'a' \"hi\" foo");
    }

    #[test]
    fn roundtrip_normalized_symbol() {
        assert_roundtrips("(f dsl/element other/thing)");
    }

    #[test]
    fn roundtrip_collections() {
        assert_roundtrips("(add 1 2)");
        assert_roundtrips("[1 2 3]");
        assert_roundtrips("{a: 1 b: 2}");
        assert_roundtrips("{read write}");
        assert_roundtrips("{}");
    }

    #[test]
    fn roundtrip_deeply_nested() {
        assert_roundtrips("(a [b {c: (d dsl/elem) g: [1 2.0]}] \"s\" 'z')");
    }

    #[test]
    fn meta_code_wit_parses() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wit/meta/code.wit");
        let contents = std::fs::read_to_string(path).expect("read wit/meta/code.wit");
        wit_parser::UnresolvedPackageGroup::parse(path, &contents)
            .expect("wit/meta/code.wit must parse");
    }

    #[test]
    fn reachable_subtree_is_dense_and_root_zero_for_leaf() {
        let (arena, roots) = read_file("(a (b c) d)").expect("read");
        let root = roots[0];
        let FormNode::Tup(items) = arena.node(root) else {
            panic!("expected tup");
        };

        let inner = items[1];
        let tree = arena_to_tree(&arena, inner);

        assert_eq!(tree.nodes.len(), 3);

        assert_eq!(tree.root, 2);
        let (arena2, root2) = tree_to_arena(&tree);
        assert!(forms_eq(&arena, inner, &arena2, root2));
    }
}
