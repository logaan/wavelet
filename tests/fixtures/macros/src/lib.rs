#[allow(warnings)]
mod macro_lib;

use macro_lib::exports::wavelet::meta::macros::Guest;
use macro_lib::wavelet::meta::code::{Node, NodeId, Tree};

struct Component;

impl Guest for Component {
    fn manifest() -> Vec<(String, u32)> {
        vec![
            ("identity".to_string(), 1),
            ("unless".to_string(), 2),
            ("boom".to_string(), 0),
        ]
    }

    fn expand(name: String, args: Tree) -> Result<Tree, String> {
        match name.as_str() {
            "identity" => expand_identity(&args),
            "unless" => expand_unless(&args),
            "boom" => Err("boom: this macro always fails".to_string()),
            other => Err(format!("unknown macro `{other}`")),
        }
    }
}

fn arg_ids(tree: &Tree) -> Result<Vec<NodeId>, String> {
    let root = tree
        .nodes
        .get(tree.root as usize)
        .ok_or("args tree root out of bounds")?;
    match root {
        Node::Tup(items) => Ok(items.iter().copied().skip(1).collect()),
        _ => Err("args tree root is not a call (tup)".to_string()),
    }
}

fn expand_identity(args: &Tree) -> Result<Tree, String> {
    let ids = arg_ids(args)?;
    let [x] = ids.as_slice() else {
        return Err(format!("identity expects 1 argument, got {}", ids.len()));
    };
    Ok(Tree {
        nodes: args.nodes.clone(),
        root: *x,
        spans: args.spans.clone(),
    })
}

fn expand_unless(args: &Tree) -> Result<Tree, String> {
    let ids = arg_ids(args)?;
    let [c, body] = ids.as_slice() else {
        return Err(format!("unless expects 2 arguments, got {}", ids.len()));
    };

    let mut b = Builder::default();
    let head = b.push(Node::Sym("if-MACRO".to_string()), (0, 0));
    let c2 = b.copy_from(args, *c);
    let empty = b.push(Node::Flg(vec![]), (0, 0));
    let body2 = b.copy_from(args, *body);
    let root = b.push(Node::Tup(vec![head, c2, empty, body2]), (0, 0));
    Ok(b.finish(root))
}

#[derive(Default)]
struct Builder {
    nodes: Vec<Node>,
    spans: Vec<(u32, u32)>,
}

impl Builder {
    fn push(&mut self, node: Node, span: (u32, u32)) -> NodeId {
        let id = self.nodes.len() as NodeId;
        self.nodes.push(node);
        self.spans.push(span);
        id
    }

    fn copy_from(&mut self, src: &Tree, id: NodeId) -> NodeId {
        let span = src.spans.get(id as usize).copied().unwrap_or((0, 0));
        let node = match &src.nodes[id as usize] {
            Node::BoolVal(b) => Node::BoolVal(*b),
            Node::IntVal(n) => Node::IntVal(*n),
            Node::DecVal(d) => Node::DecVal(*d),
            Node::CharVal(c) => Node::CharVal(*c),
            Node::StrVal(s) => Node::StrVal(s.clone()),
            Node::Sym(s) => Node::Sym(s.clone()),
            Node::Tup(items) => {
                let kids = items.iter().map(|c| self.copy_from(src, *c)).collect();
                Node::Tup(kids)
            }
            Node::Lst(items) => {
                let kids = items.iter().map(|c| self.copy_from(src, *c)).collect();
                Node::Lst(kids)
            }
            Node::Rec(fields) => {
                let kids = fields
                    .iter()
                    .map(|(k, v)| (k.clone(), self.copy_from(src, *v)))
                    .collect();
                Node::Rec(kids)
            }
            Node::Flg(names) => Node::Flg(names.clone()),
        };
        self.push(node, span)
    }

    fn finish(self, root: NodeId) -> Tree {
        Tree {
            nodes: self.nodes,
            root,
            spans: self.spans,
        }
    }
}

macro_lib::export!(Component with_types_in macro_lib);
