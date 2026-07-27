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
    /// constructor of a payloaded `DefType` variant case (4.1): applying it
    /// wraps the bundled argument as `Variant(case, Some(payload))`
    CaseCtor(String),
    Closure(Rc<Closure>),
    Macro(Rc<Closure>),
    Builtin(&'static str),
    Cell(Rc<RefCell<Value>>),
    /// A live handle to a user-declared resource (`DefResource`, 4.5). Two
    /// handles are equal iff they are the same instance (identity equality);
    /// it prints opaquely as `<name>` and is not form-serializable.
    Resource(Rc<ResourceInstance>),
    /// The constructor of a user-declared resource: applying it runs the `New`
    /// member and wraps the resulting rep in a fresh handle.
    ResourceCtor(Rc<ResourceCtor>),
    /// An instance method of a user-declared resource: applying it unwraps the
    /// receiver handle to its rep, binds `self`, and runs the method body.
    ResourceMethod(Rc<ResourceMethod>),
}

/// A live user-declared resource instance (4.5). `rep` is the value the `New`
/// member returned (e.g. a `cell` for the counter). Inside the defining
/// component the resource type denotes this rep; the handle wrapper carries the
/// nominal type name (for opaque printing) and identity.
pub struct ResourceInstance {
    pub name: String,
    pub rep: Value,
    /// Set once the handle has been moved through an `own`-typed parameter; a
    /// later use traps (reserved for own-consumption modelling).
    pub consumed: std::cell::Cell<bool>,
}

/// The constructor of a user-declared resource: the `New` member's closure plus
/// the resource type name to stamp onto each fresh handle.
pub struct ResourceCtor {
    pub name: String,
    pub ctor: Rc<Closure>,
}

/// An instance method of a user-declared resource: the method's closure (whose
/// first parameter is `self`) plus the resource type name it belongs to.
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
}

pub fn unit() -> Value {
    Value::Rec(vec![])
}

/// Whether the integer `n` fits within the WIT integer type named `type_name`.
///
/// This is the single source of truth for the per-width integer bounds: the
/// compile-time checks ([`crate::check`]'s `int_in_range` and the `to-*`
/// builtin signatures) delegate here.
///
/// Returns `None` when `type_name` is not one of the eight integer types, and
/// `Some(in_range)` otherwise. Note `u64` is the only unsigned type without an
/// upper bound in this `i64` representation — it merely rejects negatives,
/// matching the `to-u64` builtin's `n >= 0` check — and `s64` accepts any `i64`.
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
            // Resources are identity-equal: same instance (4.5). Constructors
            // and methods compare by closure identity like other callables.
            (Resource(a), Resource(b)) => Rc::ptr_eq(a, b),
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
            fields
                .iter()
                .map(|(k, v)| (k.clone(), form_to_value(arena, *v)))
                .collect(),
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
        // An unapplied case constructor serializes as its bare case name, so a
        // macro can emit a constructor reference (it re-reads as the same
        // constructor when the DefType is in scope).
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
        Value::Dec(f) => out.push_str(&format_dec(*f)),
        // `{c:?}` emits WAVE-valid escapes except for NUL, which Rust spells
        // `\0` — not an escape in WAVE (or in our own reader); spell it
        // `\u{0}` so every printed char reads back.
        Value::Char('\0') => out.push_str("'\\u{0}'"),
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
        // A resource handle prints opaquely as `<name>` (4.6): the nominal type
        // only, following the closure/case-constructor angle-bracket convention.
        // A resource type is not `Show`-derivable, so well-typed code never
        // reaches this; it is here for the repl/debug and to stay a faithful
        // oracle.
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

/// Indicative float text: six significant decimal digits, not
/// shortest-round-trip.
///
/// Values print in fixed notation when the decimal exponent is in `-4..=5`
/// (`0.0001`, `3.14159`, `123457.0`) and as `d.de±x` scientific outside it
/// (`1e6`, `2.5e-7`), with `nan`/`inf`/`-inf` spelled out and a fractional
/// part always present in fixed notation so a float never reads as an int.
/// Exact digits are traded away deliberately (5.6/0.2): the emitted `to_str`
/// helper in `src/emit/helpers.rs` hand-implements this same algorithm over
/// the same IEEE-754 double ops in the same order, so both pipelines produce
/// identical text without shipping a shortest-round-trip formatter in every
/// component. Any change here must be mirrored there.
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
    // Normalize into [1, 10), tracking the decimal exponent. Loop count is
    // bounded by the f64 exponent range (~640 for the deepest subnormal).
    let mut e: i32 = 0;
    while x >= 10.0 {
        x /= 10.0;
        e += 1;
    }
    while x < 1.0 {
        x *= 10.0;
        e -= 1;
    }
    // Seven digits: six significant plus one to round by. The subtraction of
    // the integer part is exact, so each step stays in [0, 10).
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
                // 9.99999x rounds up a magnitude: 1.00000 with e + 1.
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
        // WAVE (and our reader) has no `\0` escape, so NUL must not follow
        // Rust's `{c:?}` spelling. Everything else `{c:?}` emits is WAVE.
        assert_eq!(super::print_value(&super::Value::Char('\0')), r"'\u{0}'");
        assert_eq!(super::print_value(&super::Value::Char('\n')), r"'\n'");
        assert_eq!(super::print_value(&super::Value::Char('\'')), r"'\''");
        assert_eq!(super::print_value(&super::Value::Char('\u{7f}')), r"'\u{7f}'");
    }

    #[test]
    fn format_dec_reads_back_close() {
        // Not round-trip, but the printed text must lex as a Wavelet float
        // and stay within 6-significant-digit relative error.
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
