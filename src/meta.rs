//! The `wavelet:meta@0.1.0` `code` wire type and conversions to/from the
//! in-memory form arena (`src/form.rs`).
//!
//! Homoiconicity has to survive the component boundary (design.md §6.2), so "a
//! form" is itself a WIT type. WIT has no recursive types, so the wire encoding
//! is an arena — a flat node table plus a root index. The canonical WIT text
//! lives in `wit/meta/code.wit`; the [`Node`]/[`Tree`] types below are a plain
//! Rust mirror of it, deliberately independent of any wasm-runtime crate's
//! generated bindings (Step 3 maps these to/from the runtime's dynamic `Val`s).
//!
//! This module is native-only, gated like `emit`/`build`/`wit`/`tools`, because
//! it only matters on the compile-time/composition side of the project.
//!
//! ## Relationship to `form::Node`
//!
//! The wire [`Node`] variant set matches `form::Node` one-for-one:
//!
//! | `form::Node`            | wire [`Node`]            |
//! |------------------------|--------------------------|
//! | `Bool(bool)`           | `BoolVal(bool)`          |
//! | `Int(i64)`             | `IntVal(i64)` (wire `s64`)|
//! | `Dec(f64)`             | `DecVal(f64)`            |
//! | `Char(char)`           | `CharVal(char)`          |
//! | `Str(String)`          | `StrVal(String)`         |
//! | `Sym(String)`          | `Sym(String)`            |
//! | `Qsym(String, String)` | `Qsym(String, String)`   |
//! | `Tup(Vec<NodeId>)`     | `Tup(Vec<NodeId>)`       |
//! | `Lst(Vec<NodeId>)`     | `Lst(Vec<NodeId>)`       |
//! | `Rec(Vec<(String,_)>)` | `Rec(Vec<(String,_)>)`   |
//! | `Flg(Vec<String>)`     | `Flg(Vec<String>)`       |
//!
//! There is no `Call` node on either side: a call is a `Tup` whose head is
//! `items[0]` (design.md §6.2).

use crate::form::{Arena, Node as FormNode, NodeId};

/// A single node in the wire arena, mirroring the `node` variant in
/// `wit/meta/code.wit`. The `*Val` variant names follow the WIT
/// (`bool-val`, `int-val`, …) so the mapping to the canonical ABI is obvious.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    BoolVal(bool),
    /// Wire type is `s64`; `form::Node::Int` is `i64` — same width, no
    /// narrowing.
    IntVal(i64),
    DecVal(f64),
    CharVal(char),
    StrVal(String),
    Sym(String),
    /// `(alias, name)` — a qualified symbol such as `Dsl/Element`.
    Qsym(String, String),
    /// A parenthesized form; a call is a `Tup` whose head is `items[0]`.
    Tup(Vec<NodeId>),
    Lst(Vec<NodeId>),
    Rec(Vec<(String, NodeId)>),
    Flg(Vec<String>),
}

/// The `tree` record from `wit/meta/code.wit`: a flat node table, a root index,
/// and a parallel span table. This is the canonical interchange value that
/// crosses the component boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub root: NodeId,
    /// Source offsets `(start, end)`, parallel to `nodes`.
    pub spans: Vec<(u32, u32)>,
}

/// Flatten the sub-tree of `arena` reachable from `root` into a wire [`Tree`],
/// re-indexed so the result is self-contained.
///
/// **Reachable sub-tree, re-indexed (not the whole arena).** A `form::Arena`
/// produced by `reader::read_file` typically holds *many* top-level roots; a
/// macro call only ever wants to ship one form across the boundary. Emitting
/// just the reachable sub-tree lets a caller pass any node as the root and get a
/// minimal, dense `Tree` whose own `root` is index `0`. Node ids are remapped
/// during the walk; the resulting `nodes`/`spans` are in a deterministic
/// post-allocation order (children before the parent that references them).
///
/// **Spans are preserved** for every emitted node (foreign components will
/// usually ignore them, but the round trip must not lose them).
pub fn arena_to_tree(arena: &Arena, root: NodeId) -> Tree {
    let mut tree = Tree {
        nodes: Vec::new(),
        root: 0,
        spans: Vec::new(),
    };
    tree.root = copy_node(arena, root, &mut tree);
    tree
}

/// Recursively copy `id` (and its descendants) from `arena` into `tree`,
/// returning the new id of `id` in `tree`. Children are copied first so a
/// parent's child-id list refers to already-allocated nodes.
fn copy_node(arena: &Arena, id: NodeId, tree: &mut Tree) -> NodeId {
    let span = arena.span(id);
    let node = match arena.node(id) {
        FormNode::Bool(b) => Node::BoolVal(*b),
        FormNode::Int(n) => Node::IntVal(*n),
        FormNode::Dec(d) => Node::DecVal(*d),
        FormNode::Char(c) => Node::CharVal(*c),
        FormNode::Str(s) => Node::StrVal(s.clone()),
        FormNode::Sym(s) => Node::Sym(s.clone()),
        FormNode::Qsym(alias, name) => Node::Qsym(alias.clone(), name.clone()),
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

/// Rebuild an in-memory `(Arena, root)` from a wire [`Tree`].
///
/// The arena is populated in the same order the wire nodes appear; because a
/// wire `Tree` produced by [`arena_to_tree`] lists children before parents,
/// child ids are always already valid when a parent references them, but the
/// implementation does not rely on that — it copies the node table verbatim and
/// keeps the ids the wire used, so any well-formed `Tree` round-trips. Spans are
/// carried over verbatim.
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
            Node::Qsym(alias, name) => FormNode::Qsym(alias.clone(), name.clone()),
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

    /// Structurally compare the sub-tree rooted at `a` in `arena_a` with the
    /// one rooted at `b` in `arena_b`. Ids will differ (the round trip
    /// re-indexes), so we compare shape + payload + spans, not raw ids.
    fn forms_eq(
        arena_a: &Arena,
        a: NodeId,
        arena_b: &Arena,
        b: NodeId,
    ) -> bool {
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
            (FormNode::Qsym(x1, x2), FormNode::Qsym(y1, y2)) => x1 == y1 && x2 == y2,
            (FormNode::Tup(xs), FormNode::Tup(ys))
            | (FormNode::Lst(xs), FormNode::Lst(ys)) => {
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

    /// Read `src`, then assert every top-level form round-trips
    /// arena → tree → arena as a structural identity (payload + spans).
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
        // bool / int / dec / char / str / sym
        assert_roundtrips("true false 42 -7 3.14 'a' \"hi\" foo");
    }

    #[test]
    fn roundtrip_qsym() {
        // Qualified symbols round-trip purely on their (alias, name) strings;
        // the exact macro-head reading rules are orthogonal to this conversion.
        assert_roundtrips("(f dsl/element other/thing)");
    }

    #[test]
    fn roundtrip_collections() {
        // call/tuple, list, record, flags.
        assert_roundtrips("(add 1 2)");
        assert_roundtrips("[1 2 3]");
        assert_roundtrips("{a: 1 b: 2}"); // record: `name:` fields
        assert_roundtrips("{read write}"); // flags: bare names
        assert_roundtrips("{}"); // empty flags
    }

    #[test]
    fn roundtrip_deeply_nested() {
        assert_roundtrips("(a [b {c: (d dsl/elem) g: [1 2.0]}] \"s\" 'z')");
    }

    /// The checked-in WIT must parse as a well-formed `wavelet:meta@0.1.0`
    /// package, so the canonical wire definition can't silently rot.
    #[test]
    fn meta_code_wit_parses() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wit/meta/code.wit");
        let contents = std::fs::read_to_string(path).expect("read wit/meta/code.wit");
        wit_parser::UnresolvedPackageGroup::parse(path, &contents)
            .expect("wit/meta/code.wit must parse");
    }

    #[test]
    fn reachable_subtree_is_dense_and_root_zero_for_leaf() {
        // arena_to_tree emits only the reachable sub-tree, re-indexed.
        let (arena, roots) = read_file("(a (b c) d)").expect("read");
        let root = roots[0];
        let FormNode::Tup(items) = arena.node(root) else {
            panic!("expected tup");
        };
        // Ship just the inner `(b c)` sub-tree.
        let inner = items[1];
        let tree = arena_to_tree(&arena, inner);
        // inner is `(b c)` -> 3 nodes: `b`, `c`, and the tup itself.
        assert_eq!(tree.nodes.len(), 3);
        // The tup is allocated last (children first), so root is the last node.
        assert_eq!(tree.root, 2);
        let (arena2, root2) = tree_to_arena(&tree);
        assert!(forms_eq(&arena, inner, &arena2, root2));
    }
}
