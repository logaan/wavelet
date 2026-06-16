pub type NodeId = u32;

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Bool(bool),
    Int(i64),
    Dec(f64),
    Char(char),
    Str(String),
    Sym(String),
    Qsym(String, String),
    /// A parenthesized form `(a b c …)`: the single call/application/tuple node.
    /// In evaluation position it is a call (head = items[0]); under `Quote` it is
    /// a tuple value.
    Tup(Vec<NodeId>),
    Lst(Vec<NodeId>),
    Rec(Vec<(String, NodeId)>),
    Flg(Vec<String>),
}

#[derive(Debug, Default)]
pub struct Arena {
    pub nodes: Vec<Node>,
    pub spans: Vec<(u32, u32)>,
}

impl Arena {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, node: Node, span: (u32, u32)) -> NodeId {
        let id = self.nodes.len() as NodeId;
        self.nodes.push(node);
        self.spans.push(span);
        id
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    pub fn span(&self, id: NodeId) -> (u32, u32) {
        self.spans[id as usize]
    }
}
