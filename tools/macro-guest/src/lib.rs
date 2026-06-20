//! A *produced* `wavelet:meta/macros` component (Step 9, strategy A:
//! interpreter-in-a-component).
//!
//! Unlike the hand-written fixture (`tests/fixtures/macros/src/lib.rs`), whose
//! `manifest`/`expand` are bespoke Rust, this guest is **generic**: it embeds a
//! Wavelet macro-library source (`build.rs` pulls it in from `WAVELET_MACRO_SRC`)
//! and runs it through the bundled Wavelet interpreter — the semantics oracle —
//! via `wavelet::macrolib`. So a component built from any `.wvl` macro file
//! expands its macros *exactly* as the local ahead-of-time expander
//! (`wavelet::expand::expand_file`) would.
//!
//! ## What it does at each export
//!
//! - `manifest()` -> `wavelet::macrolib::manifest(SOURCE)`: the `(name, arity)`
//!   pairs of the embedded file's `DefMacro`s (unsuffixed names).
//! - `expand(name, args)`: lift the canonical-ABI `tree` into a
//!   `wavelet::form::Arena`, call `wavelet::macrolib::expand_one(SOURCE, name,
//!   arena, root)` for **one** expansion step (the consumer recurses to
//!   fixpoint), and lower the resulting form back into a `tree`.
//!
//! ## The args-tree contract (PINNED — shared with Steps 3/9)
//!
//! `args` is the whole call form: a `tup` whose element 0 is the macro head and
//! elements `1..` are the arguments. `expand_one` reads it that way; this guest
//! ships the lifted arena straight through, so element indexing happens once,
//! inside the shared `macrolib`.

#[allow(warnings)]
mod macro_lib;

use macro_lib::exports::wavelet::meta::macros::Guest;
use macro_lib::wavelet::meta::code::{Node as WireNode, Tree};

use wavelet::form::{Arena, Node as FormNode, NodeId};

/// The macro-library source embedded by `build.rs` (from `WAVELET_MACRO_SRC`).
const SOURCE: &str = include_str!(concat!(env!("OUT_DIR"), "/macro_source.wvl"));

struct Component;

impl Guest for Component {
    fn manifest() -> Vec<(String, u32)> {
        // A malformed embedded source is a build-time bug in the producer; an
        // empty manifest is the safe surface (the consumer then finds no macros).
        wavelet::macrolib::manifest(SOURCE).unwrap_or_default()
    }

    fn expand(name: String, args: Tree) -> Result<Tree, String> {
        let (arena, root) = tree_to_arena(&args)?;
        let (out, out_root) = wavelet::macrolib::expand_one(SOURCE, &name, arena, root)?;
        Ok(arena_to_tree(&out, out_root))
    }
}

// ---------------------------------------------------------------------------
// Marshalling: wire `Tree` <-> `wavelet::form::Arena`
//
// This mirrors the native `wavelet::meta` module (which is gated off wasm), but
// against the wit-bindgen-generated `Tree`/`Node` types. The node-variant set
// lines up one-for-one with `form::Node`.
// ---------------------------------------------------------------------------

/// Lift the canonical-ABI `tree` into an in-memory `form::Arena`.
///
/// The wire arena already uses dense ids that list children before parents
/// (`wavelet::meta::arena_to_tree` guarantees it), so we can copy the node table
/// verbatim and keep its ids — exactly like `meta::tree_to_arena`.
fn tree_to_arena(tree: &Tree) -> Result<(Arena, NodeId), String> {
    let mut arena = Arena::new();
    for (i, node) in tree.nodes.iter().enumerate() {
        let span = tree.spans.get(i).copied().unwrap_or((0, 0));
        let form = match node {
            WireNode::BoolVal(b) => FormNode::Bool(*b),
            WireNode::IntVal(n) => FormNode::Int(*n),
            WireNode::DecVal(d) => FormNode::Dec(*d),
            WireNode::CharVal(c) => FormNode::Char(*c),
            WireNode::StrVal(s) => FormNode::Str(s.clone()),
            WireNode::Sym(s) => FormNode::Sym(s.clone()),
            WireNode::Qsym((a, n)) => FormNode::Qsym(a.clone(), n.clone()),
            WireNode::Tup(items) => FormNode::Tup(items.clone()),
            WireNode::Lst(items) => FormNode::Lst(items.clone()),
            WireNode::Rec(fields) => FormNode::Rec(fields.clone()),
            WireNode::Flg(names) => FormNode::Flg(names.clone()),
        };
        arena.add(form, span);
    }
    if (tree.root as usize) >= tree.nodes.len() {
        return Err("args tree root out of bounds".to_string());
    }
    Ok((arena, tree.root))
}

/// Lower the sub-tree of `arena` reachable from `root` into a canonical-ABI
/// `tree`, re-indexed dense with children before parents (children-first, so the
/// result round-trips through `tree_to_arena`). Mirrors
/// `wavelet::meta::arena_to_tree`.
fn arena_to_tree(arena: &Arena, root: NodeId) -> Tree {
    let mut nodes = Vec::new();
    let mut spans = Vec::new();
    let root = copy_node(arena, root, &mut nodes, &mut spans);
    Tree { nodes, root, spans }
}

fn copy_node(
    arena: &Arena,
    id: NodeId,
    nodes: &mut Vec<WireNode>,
    spans: &mut Vec<(u32, u32)>,
) -> NodeId {
    let span = arena.span(id);
    let node = match arena.node(id) {
        FormNode::Bool(b) => WireNode::BoolVal(*b),
        FormNode::Int(n) => WireNode::IntVal(*n),
        FormNode::Dec(d) => WireNode::DecVal(*d),
        FormNode::Char(c) => WireNode::CharVal(*c),
        FormNode::Str(s) => WireNode::StrVal(s.clone()),
        FormNode::Sym(s) => WireNode::Sym(s.clone()),
        FormNode::Qsym(a, n) => WireNode::Qsym((a.clone(), n.clone())),
        FormNode::Tup(items) => {
            let kids = items.iter().map(|&c| copy_node(arena, c, nodes, spans)).collect();
            WireNode::Tup(kids)
        }
        FormNode::Lst(items) => {
            let kids = items.iter().map(|&c| copy_node(arena, c, nodes, spans)).collect();
            WireNode::Lst(kids)
        }
        FormNode::Rec(fields) => {
            let kids = fields
                .iter()
                .map(|(k, v)| (k.clone(), copy_node(arena, *v, nodes, spans)))
                .collect();
            WireNode::Rec(kids)
        }
        FormNode::Flg(names) => WireNode::Flg(names.clone()),
    };
    let id = nodes.len() as NodeId;
    nodes.push(node);
    spans.push(span);
    id
}

macro_lib::export!(Component with_types_in macro_lib);
