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

    Variant(String, Option<Rc<Value>>),

    CaseCtor(String),
    Closure(Rc<Closure>),
    Macro(Rc<Closure>),
    Builtin(&'static str),
    Cell(Rc<RefCell<Value>>),

    Resource(Rc<ResourceInstance>),

    ResourceCtor(Rc<ResourceCtor>),

    ResourceMethod(Rc<ResourceMethod>),
}

pub struct ResourceInstance {
    pub name: String,
    pub rep: Value,

    pub consumed: std::cell::Cell<bool>,
    pub identity: Rc<()>,
    pub borrowed: bool,
}

pub struct ResourceCtor {
    pub name: String,
    pub ctor: Rc<Closure>,
    pub destructor: Option<Rc<Closure>>,
}

pub struct ResourceMethod {
    pub name: String,
    pub method: Rc<Closure>,
}

pub struct Closure {
    pub params: Vec<Param>,
    pub body: NodeId,
    pub arena: Rc<Arena>,
    pub env: Env,
}

pub struct Param {
    pub name: String,
    pub resource_type: Option<(String, bool)>,
}

pub fn unit() -> Value {
    Value::Rec(vec![])
}

pub fn int_fits(type_name: &str, n: i64) -> Option<bool> {
    let fits = match type_name {
        "u8" => (0..=u8::MAX as i64).contains(&n),
        "u16" => (0..=u16::MAX as i64).contains(&n),
        "u32" => (0..=u32::MAX as i64).contains(&n),
        "u64" => n >= 0,
        "s8" => (i8::MIN as i64..=i8::MAX as i64).contains(&n),
        "s16" => (i16::MIN as i64..=i16::MAX as i64).contains(&n),
        "s32" => (i32::MIN as i64..=i32::MAX as i64).contains(&n),
        "s64" => true,
        _ => return None,
    };
    Some(fits)
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
            (CaseCtor(a), CaseCtor(b)) => a == b,
            (Cell(a), Cell(b)) => Rc::ptr_eq(a, b),

            (Resource(a), Resource(b)) => Rc::ptr_eq(&a.identity, &b.identity),
            (ResourceCtor(a), ResourceCtor(b)) => Rc::ptr_eq(a, b),
            (ResourceMethod(a), ResourceMethod(b)) => Rc::ptr_eq(a, b),
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
        Env(Rc::new(EnvInner {
            vars: RefCell::new(HashMap::new()),
            parent: None,
        }))
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

pub fn form_to_value(arena: &Arena, id: NodeId) -> Value {
    match arena.node(id) {
        Node::Bool(b) => Value::Bool(*b),
        Node::Int(n) => Value::Int(*n),
        Node::Dec(f) => Value::Dec(*f),
        Node::Char(c) => Value::Char(*c),
        Node::Str(s) => Value::Str(s.clone()),
        Node::Sym(s) => Value::Variant(s.clone(), None),
        Node::Tup(items) => Value::Tup(items.iter().map(|&i| form_to_value(arena, i)).collect()),
        Node::Lst(items) => Value::Lst(items.iter().map(|&i| form_to_value(arena, i)).collect()),
        Node::Rec(fields) => Value::Rec(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), form_to_value(arena, *v)))
                .collect(),
        ),
        Node::Flg(names) => Value::Flg(names.clone()),
    }
}

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

        Value::CaseCtor(name) => sym_node(name),
        Value::Closure(_)
        | Value::Macro(_)
        | Value::Builtin(_)
        | Value::Cell(_)
        | Value::Resource(_)
        | Value::ResourceCtor(_)
        | Value::ResourceMethod(_) => {
            return Err("this value cannot appear in code".into());
        }
    };
    Ok(arena.add(node, sp))
}

fn values_to_forms(items: &[Value], arena: &mut Arena) -> Result<Vec<NodeId>, String> {
    items.iter().map(|v| value_to_form(v, arena)).collect()
}

fn sym_node(name: &str) -> Node {
    Node::Sym(name.replace('/', "-"))
}

pub fn print_value(v: &Value) -> String {
    let mut out = String::new();
    write_value(v, &mut out);
    out
}

pub fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 32 || ch == '\u{7f}' => {
                use std::fmt::Write;
                write!(out, "\\u{{{:x}}}", ch as u32).unwrap();
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

fn write_value(v: &Value, out: &mut String) {
    match v {
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(n) => out.push_str(&n.to_string()),
        Value::Dec(f) => out.push_str(&format_dec(*f)),

        Value::Char('\0') => out.push_str("'\\u{0}'"),
        Value::Char(c) => out.push_str(&format!("{c:?}")),
        Value::Str(s) => write_string(s, out),
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
        Value::CaseCtor(name) => {
            out.push_str("<constructor ");
            out.push_str(name);
            out.push('>');
        }
        Value::Cell(c) => {
            out.push_str("cell(");
            write_value(&c.borrow(), out);
            out.push(')');
        }

        Value::Resource(r) => {
            out.push('<');
            out.push_str(&r.name);
            out.push('>');
        }
        Value::ResourceCtor(c) => {
            out.push_str("<constructor ");
            out.push_str(&c.name);
            out.push('>');
        }
        Value::ResourceMethod(m) => {
            out.push_str("<method ");
            out.push_str(&m.name);
            out.push('>');
        }
    }
}

pub fn format_dec(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f == f64::INFINITY {
        return "inf".into();
    }
    if f == f64::NEG_INFINITY {
        return "-inf".into();
    }
    let mut out = String::new();
    if f.is_sign_negative() {
        out.push('-');
    }
    let mut x = f.abs();
    if x == 0.0 {
        out.push_str("0.0");
        return out;
    }

    let mut e: i32 = 0;
    while x >= 10.0 {
        x /= 10.0;
        e += 1;
    }
    while x < 1.0 {
        x *= 10.0;
        e -= 1;
    }

    let mut d = [0u8; 7];
    for digit in d.iter_mut() {
        let t = x as u8;
        *digit = t;
        x = (x - t as f64) * 10.0;
    }
    if d[6] >= 5 {
        let mut k = 5i32;
        loop {
            if k < 0 {
                d[0] = 1;
                e += 1;
                break;
            }
            if d[k as usize] == 9 {
                d[k as usize] = 0;
                k -= 1;
            } else {
                d[k as usize] += 1;
                break;
            }
        }
    }
    let mut last = 5usize;
    while last > 0 && d[last] == 0 {
        last -= 1;
    }
    let push_digit = |out: &mut String, d: u8| out.push((b'0' + d) as char);
    if (-4..=5).contains(&e) {
        if e >= 0 {
            for k in 0..=e as usize {
                push_digit(&mut out, d[k]);
            }
            out.push('.');
            if last > e as usize {
                for k in (e as usize + 1)..=last {
                    push_digit(&mut out, d[k]);
                }
            } else {
                out.push('0');
            }
        } else {
            out.push_str("0.");
            for _ in 0..(-e - 1) {
                out.push('0');
            }
            for k in 0..=last {
                push_digit(&mut out, d[k]);
            }
        }
    } else {
        push_digit(&mut out, d[0]);
        if last > 0 {
            out.push('.');
            for k in 1..=last {
                push_digit(&mut out, d[k]);
            }
        }
        out.push('e');
        out.push_str(&e.to_string());
    }
    out
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

#[cfg(test)]
mod tests {
    use super::format_dec;

    #[test]
    fn format_dec_specials() {
        assert_eq!(format_dec(f64::NAN), "nan");
        assert_eq!(format_dec(f64::INFINITY), "inf");
        assert_eq!(format_dec(f64::NEG_INFINITY), "-inf");
        assert_eq!(format_dec(0.0), "0.0");
        assert_eq!(format_dec(-0.0), "-0.0");
    }

    #[test]
    fn format_dec_fixed() {
        assert_eq!(format_dec(1.0), "1.0");
        assert_eq!(format_dec(-1.0), "-1.0");
        assert_eq!(format_dec(0.1), "0.1");
        assert_eq!(format_dec(2.5), "2.5");
        assert_eq!(format_dec(std::f64::consts::PI), "3.14159");
        assert_eq!(format_dec(2.0 / 3.0), "0.666667");
        assert_eq!(format_dec(100.0), "100.0");
        assert_eq!(format_dec(999999.0), "999999.0");
        assert_eq!(format_dec(123456.7), "123457.0");
        assert_eq!(format_dec(0.0001), "0.0001");
        assert_eq!(format_dec(-12.25), "-12.25");
    }

    #[test]
    fn format_dec_scientific() {
        assert_eq!(format_dec(1000000.0), "1e6");
        assert_eq!(format_dec(0.00001), "1e-5");
        assert_eq!(format_dec(1e300), "1e300");
        assert_eq!(format_dec(1.5e300), "1.5e300");
        assert_eq!(format_dec(-2.5e-7), "-2.5e-7");
        assert_eq!(format_dec(123456789.0), "1.23457e8");
    }

    #[test]
    fn format_dec_rounding_carries_across_a_magnitude() {
        assert_eq!(format_dec(999999.9), "1e6");
        assert_eq!(format_dec(9.9999999), "10.0");
    }

    #[test]
    fn char_nul_prints_the_wave_escape_not_rusts() {
        assert_eq!(super::print_value(&super::Value::Char('\0')), r"'\u{0}'");
        assert_eq!(super::print_value(&super::Value::Char('\n')), r"'\n'");
        assert_eq!(super::print_value(&super::Value::Char('\'')), r"'\''");
        assert_eq!(
            super::print_value(&super::Value::Char('\u{7f}')),
            r"'\u{7f}'"
        );
    }

    #[test]
    fn format_dec_reads_back_close() {
        for &x in &[
            0.1, 2.5, 1234.5678, 1e-8, 7.25e120, -3.9e-200, 123456.7, 0.00001,
        ] {
            let s = format_dec(x);
            let back: f64 = s.parse().unwrap();
            assert!(
                ((back - x) / x).abs() < 1e-5,
                "{x} printed {s} which reads back {back}"
            );
        }
    }
}
