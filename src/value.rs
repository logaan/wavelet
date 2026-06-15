use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use crate::form::{Arena, Node, NodeId};

#[derive(Clone)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Dec(f64),
    Char(char),
    Str(String),
    Tup(Vec<Value>),
    Lst(Vec<Value>),
    Rec(Vec<(String, Value)>),
    Flg(Vec<String>),
    /// variant case; payload-less cases double as symbols under quote
    Variant(String, Option<Rc<Value>>),
    Closure(Rc<Closure>),
    Macro(Rc<Closure>),
    Builtin(&'static str),
    Cell(Rc<RefCell<Value>>),
}

pub struct Closure {
    pub params: Vec<Param>,
    pub body: NodeId,
    pub arena: Rc<Arena>,
    pub env: Env,
}

pub struct Param {
    pub name: String,
    pub ty: Option<String>,
}

pub fn unit() -> Value {
    Value::Rec(vec![])
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        use Value::*;
        match (self, other) {
            (Bool(a), Bool(b)) => a == b,
            (Int(a), Int(b)) => a == b,
            (Dec(a), Dec(b)) => a == b,
            (Char(a), Char(b)) => a == b,
            (Str(a), Str(b)) => a == b,
            (Tup(a), Tup(b)) | (Lst(a), Lst(b)) => a == b,
            (Rec(a), Rec(b)) => a == b,
            (Flg(a), Flg(b)) => a == b,
            (Variant(a, p), Variant(b, q)) => a == b && p == q,
            (Closure(a), Closure(b)) | (Macro(a), Macro(b)) => Rc::ptr_eq(a, b),
            (Builtin(a), Builtin(b)) => a == b,
            (Cell(a), Cell(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&print_value(self))
    }
}

#[derive(Clone)]
pub struct Env(Rc<EnvInner>);

struct EnvInner {
    vars: RefCell<HashMap<String, Value>>,
    parent: Option<Env>,
}

impl Env {
    pub fn root() -> Env {
        Env(Rc::new(EnvInner { vars: RefCell::new(HashMap::new()), parent: None }))
    }

    pub fn child(&self) -> Env {
        Env(Rc::new(EnvInner {
            vars: RefCell::new(HashMap::new()),
            parent: Some(self.clone()),
        }))
    }

    pub fn define(&self, name: impl Into<String>, value: Value) {
        self.0.vars.borrow_mut().insert(name.into(), value);
    }

    pub fn lookup(&self, name: &str) -> Option<Value> {
        if let Some(v) = self.0.vars.borrow().get(name) {
            return Some(v.clone());
        }
        self.0.parent.as_ref().and_then(|p| p.lookup(name))
    }
}

/// `Quote`: a form, as data. Calls are tuples whose first element is the head,
/// bare names payload-less variant cases (symbols) (§2.3).
pub fn form_to_value(arena: &Arena, id: NodeId) -> Value {
    match arena.node(id) {
        Node::Bool(b) => Value::Bool(*b),
        Node::Int(n) => Value::Int(*n),
        Node::Dec(f) => Value::Dec(*f),
        Node::Char(c) => Value::Char(*c),
        Node::Str(s) => Value::Str(s.clone()),
        Node::Sym(s) => Value::Variant(s.clone(), None),
        Node::Qsym(a, n) => Value::Variant(format!("{a}/{n}"), None),
        Node::Tup(items) => Value::Tup(items.iter().map(|&i| form_to_value(arena, i)).collect()),
        Node::Lst(items) => Value::Lst(items.iter().map(|&i| form_to_value(arena, i)).collect()),
        Node::Rec(fields) => Value::Rec(
            fields.iter().map(|(k, v)| (k.clone(), form_to_value(arena, *v))).collect(),
        ),
        Node::Flg(names) => Value::Flg(names.clone()),
    }
}

/// Inverse of `form_to_value`: turn macro output back into nodes.
pub fn value_to_form(value: &Value, arena: &mut Arena) -> Result<NodeId, String> {
    let sp = (0, 0);
    let node = match value {
        Value::Bool(b) => Node::Bool(*b),
        Value::Int(n) => Node::Int(*n),
        Value::Dec(f) => Node::Dec(*f),
        Value::Char(c) => Node::Char(*c),
        Value::Str(s) => Node::Str(s.clone()),
        Value::Variant(name, None) => sym_node(name),
        Value::Variant(name, Some(p)) => {
            // A payloaded runtime variant serializes back to a 1-argument call
            // form: `ok(x)` ⇒ `Tup[Sym(ok), value_to_form(x)]`.
            let head = arena.add(sym_node(name), sp);
            let payload = value_to_form(p, arena)?;
            Node::Tup(vec![head, payload])
        }
        Value::Tup(items) => Node::Tup(values_to_forms(items, arena)?),
        Value::Lst(items) => Node::Lst(values_to_forms(items, arena)?),
        Value::Rec(fields) => {
            let mut out = Vec::with_capacity(fields.len());
            for (k, v) in fields {
                out.push((k.clone(), value_to_form(v, arena)?));
            }
            Node::Rec(out)
        }
        Value::Flg(names) => Node::Flg(names.clone()),
        Value::Closure(_) | Value::Macro(_) | Value::Builtin(_) | Value::Cell(_) => {
            return Err("this value cannot appear in code".into());
        }
    };
    Ok(arena.add(node, sp))
}

fn values_to_forms(items: &[Value], arena: &mut Arena) -> Result<Vec<NodeId>, String> {
    items.iter().map(|v| value_to_form(v, arena)).collect()
}

fn sym_node(name: &str) -> Node {
    match name.split_once('/') {
        Some((a, n)) => Node::Qsym(a.to_string(), n.to_string()),
        None => Node::Sym(name.to_string()),
    }
}

/// Canonical WAVE text for a runtime value.
pub fn print_value(v: &Value) -> String {
    let mut out = String::new();
    write_value(v, &mut out);
    out
}

fn write_value(v: &Value, out: &mut String) {
    match v {
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(n) => out.push_str(&n.to_string()),
        Value::Dec(f) => {
            if f.is_nan() {
                out.push_str("nan");
            } else if *f == f64::INFINITY {
                out.push_str("inf");
            } else if *f == f64::NEG_INFINITY {
                out.push_str("-inf");
            } else {
                out.push_str(&format!("{f:?}"));
            }
        }
        Value::Char(c) => out.push_str(&format!("{c:?}")),
        Value::Str(s) => out.push_str(&format!("{s:?}")),
        Value::Variant(name, None) => out.push_str(name),
        Value::Variant(name, Some(p)) => {
            out.push_str(name);
            out.push('(');
            write_value(p, out);
            out.push(')');
        }
        Value::Tup(items) => write_value_seq(items, '(', ')', out),
        Value::Lst(items) => write_value_seq(items, '[', ']', out),
        Value::Rec(fields) => {
            out.push('{');
            for (i, (k, v)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(k);
                out.push_str(": ");
                write_value(v, out);
            }
            out.push('}');
        }
        Value::Flg(names) => {
            out.push('{');
            for (i, n) in names.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(n);
            }
            out.push('}');
        }
        Value::Closure(_) => out.push_str("<fn>"),
        Value::Macro(_) => out.push_str("<macro>"),
        Value::Builtin(name) => {
            out.push_str("<builtin ");
            out.push_str(name);
            out.push('>');
        }
        Value::Cell(c) => {
            out.push_str("cell(");
            write_value(&c.borrow(), out);
            out.push(')');
        }
    }
}

fn write_value_seq(items: &[Value], open: char, close: char, out: &mut String) {
    out.push(open);
    for (i, v) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_value(v, out);
    }
    out.push(close);
}
