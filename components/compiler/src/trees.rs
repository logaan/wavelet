use crate::bindings::wavelet::meta::code as guest;
use wavelet::meta as core;

pub fn to_guest(tree: &core::Tree) -> guest::Tree {
    guest::Tree {
        root: tree.root,
        spans: tree.spans.clone(),
        nodes: tree
            .nodes
            .iter()
            .map(|node| match node {
                core::Node::BoolVal(value) => guest::Node::BoolVal(*value),
                core::Node::IntVal(value) => guest::Node::IntVal(*value),
                core::Node::DecVal(value) => guest::Node::DecVal(*value),
                core::Node::CharVal(value) => guest::Node::CharVal(*value),
                core::Node::StrVal(value) => guest::Node::StrVal(value.clone()),
                core::Node::Sym(value) => guest::Node::Sym(value.clone()),
                core::Node::Tup(value) => guest::Node::Tup(value.clone()),
                core::Node::Lst(value) => guest::Node::Lst(value.clone()),
                core::Node::Rec(value) => guest::Node::Rec(value.clone()),
                core::Node::Flg(value) => guest::Node::Flg(value.clone()),
            })
            .collect(),
    }
}

pub fn from_guest(tree: guest::Tree) -> core::Tree {
    core::Tree {
        root: tree.root,
        spans: tree.spans,
        nodes: tree
            .nodes
            .into_iter()
            .map(|node| match node {
                guest::Node::BoolVal(value) => core::Node::BoolVal(value),
                guest::Node::IntVal(value) => core::Node::IntVal(value),
                guest::Node::DecVal(value) => core::Node::DecVal(value),
                guest::Node::CharVal(value) => core::Node::CharVal(value),
                guest::Node::StrVal(value) => core::Node::StrVal(value),
                guest::Node::Sym(value) => core::Node::Sym(value),
                guest::Node::Tup(value) => core::Node::Tup(value),
                guest::Node::Lst(value) => core::Node::Lst(value),
                guest::Node::Rec(value) => core::Node::Rec(value),
                guest::Node::Flg(value) => core::Node::Flg(value),
            })
            .collect(),
    }
}
