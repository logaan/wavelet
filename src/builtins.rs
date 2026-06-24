use std::cell::RefCell;
use std::rc::Rc;

use crate::form::{Arena, Node, NodeId};
use crate::interp::{err, EvalError, Interp};
use crate::value::{form_to_value, print_value, unit, Env, Value};

type R<T> = Result<T, EvalError>;

pub const NAMES: &[&str] = &[
    "eq", "lt", "le", "gt", "ge", "not",
    "add", "sub", "mul", "div", "rem", "neg", "min", "max", "abs",
    "len", "empty", "get", "put", "push", "concat", "head", "tail",
    "reverse", "range", "map", "filter", "fold", "zip",
    "str-cat", "upper", "lower", "split", "join", "contains",
    "to-string", "read",
    "to-u8", "to-u16", "to-u32", "to-u64", "to-s8", "to-s16", "to-s32", "to-s64",
    "to-f32", "to-f64",
    "apply", "gensym", "expand",
    "form-kind", "rec-key", "rec-val",
    "some", "ok", "err",
    "cell-new", "cell-get", "cell-set", "drop",
];

/// The functor `Set` operation builtins, kept out of [`NAMES`] so they are *not*
/// installed under bare names: only a functor `Import` reaches them, via the
/// alias-qualified bindings (`alias/new`, `alias/add`, …) that
/// [`crate::runner::bind_functor`] defines as `Value::Builtin`s. The interpreter
/// dispatches any `Value::Builtin(name)` through [`call`] regardless of whether
/// the name was installed, so listing them here is all that is required to make
/// them callable. The runtime backing is a [`SetHandle`].
pub const SET_OPS: &[&str] = &["set-new", "set-add", "set-contains", "set-size"];

pub fn install(env: &Env) {
    for name in NAMES {
        env.define(*name, Value::Builtin(name));
    }
    env.define("none", Value::Variant("none".into(), None));
    env.define("pi", Value::Dec(std::f64::consts::PI));
}

/// Which built-in functor template an `Import` instantiates, identified from its
/// `pkg:`. The compiler knows these intrinsically — there is no real
/// `wavelet:coll/*` component in the tree. Mirrors `wit::FunctorKind` (the WIT
/// side); kept here so the always-compiled interpreter path (`eval_snippet`,
/// shared with the wasm playground) can bind functor ops without depending on
/// the native-only `wit`/`runner` modules.
pub enum FunctorKind {
    /// `wavelet:coll/set`
    Set,
}

/// A functor instantiation as the interpreter needs to see it: its alias and
/// which template it names. The element type is a compile-time/WIT concern that
/// does not affect runtime behaviour — the interpreter's set is element-agnostic
/// and uses `Value` equality.
pub struct FunctorImport {
    pub alias: String,
    pub kind: FunctorKind,
}

/// Recognize an `Import` record payload as a functor instantiation, keyed on its
/// `pkg:` naming a known functor package (`wit::parse_functor` is the authority
/// for the WIT side; this mirrors its package test). Returns `None` for any
/// ordinary import. The `as:` alias defaults to the trailing path segment, as in
/// `wit`/`runner::parse_import`.
pub fn parse_functor_import(arena: &Arena, payload: NodeId) -> Option<FunctorImport> {
    let Node::Rec(fields) = arena.node(payload) else {
        return None;
    };
    let pkg = fields.iter().find_map(|(k, v)| match (k.as_str(), arena.node(*v)) {
        ("pkg", Node::Str(s)) => Some(s.clone()),
        _ => None,
    })?;
    let path = pkg.split('@').next().unwrap_or(&pkg);
    let kind = if path.ends_with("coll/set") {
        FunctorKind::Set
    } else {
        return None;
    };
    let alias = fields
        .iter()
        .find_map(|(k, v)| match (k.as_str(), arena.node(*v)) {
            ("as", Node::Sym(s)) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).to_string());
    Some(FunctorImport { alias, kind })
}

/// Bind a functor's qualified operations into `env` as builtins. For the `Set`
/// functor, `alias/new`, `alias/add`, `alias/contains`, and `alias/size` map to
/// the `set-*` builtins, whose semantics mirror the `SET_OPS` WIT descriptor in
/// `wit.rs` (`new() -> set`, `add(set, elem)`, `contains(set, elem) -> bool`,
/// `size(set) -> u32`). The qualified name is what `Node::Qsym` evaluation and
/// call dispatch look up, so binding it here is all that is needed.
pub fn bind_functor(env: &Env, functor: &FunctorImport) {
    match functor.kind {
        FunctorKind::Set => {
            for (op, builtin) in [
                ("new", "set-new"),
                ("add", "set-add"),
                ("contains", "set-contains"),
                ("size", "set-size"),
            ] {
                env.define(format!("{}/{op}", functor.alias), Value::Builtin(builtin));
            }
        }
    }
}

fn args_n(arg: Value, n: usize, name: &str) -> R<Vec<Value>> {
    match arg {
        Value::Lst(v) | Value::Tup(v) if v.len() == n => Ok(v),
        other => err(format!(
            "`{name}` expects {n} arguments, got {}",
            print_value(&other)
        )),
    }
}

fn want_list(v: Value, name: &str) -> R<Vec<Value>> {
    match v {
        Value::Lst(items) => Ok(items),
        other => err(format!("`{name}` expects a list, got {}", print_value(&other))),
    }
}

/// The parts of a `str-cat`/sequence-style call, read from the bundled payload:
/// ≥2 args bundle to a `Tup`, a single list arg stays a `Lst`, and a single
/// scalar arg is one part on its own — so `str-cat(a b)`, `str-cat([a b])`, and
/// `str-cat(x)` are all valid (matching the wasm backend, where each argument is
/// one part).
fn payload_parts(v: Value) -> Vec<Value> {
    match v {
        Value::Lst(items) | Value::Tup(items) => items,
        other => vec![other],
    }
}

fn want_str(v: Value, name: &str) -> R<String> {
    match v {
        Value::Str(s) => Ok(s),
        other => err(format!("`{name}` expects a string, got {}", print_value(&other))),
    }
}

fn want_int(v: &Value, name: &str) -> R<i64> {
    match v {
        Value::Int(n) => Ok(*n),
        other => err(format!("`{name}` expects an integer, got {}", print_value(other))),
    }
}

enum Num {
    I(i64),
    D(f64),
}

fn want_num(v: &Value, name: &str) -> R<Num> {
    match v {
        Value::Int(n) => Ok(Num::I(*n)),
        Value::Dec(f) => Ok(Num::D(*f)),
        other => err(format!("`{name}` expects a number, got {}", print_value(other))),
    }
}

fn arith(name: &str, a: &Value, b: &Value, fi: fn(i64, i64) -> Option<i64>, fd: fn(f64, f64) -> f64) -> R<Value> {
    match (want_num(a, name)?, want_num(b, name)?) {
        (Num::I(x), Num::I(y)) => match fi(x, y) {
            Some(n) => Ok(Value::Int(n)),
            None => err(format!("`{name}`: integer overflow or division by zero")),
        },
        (x, y) => {
            let xf = match x { Num::I(n) => n as f64, Num::D(f) => f };
            let yf = match y { Num::I(n) => n as f64, Num::D(f) => f };
            Ok(Value::Dec(fd(xf, yf)))
        }
    }
}

fn compare(name: &str, a: &Value, b: &Value) -> R<std::cmp::Ordering> {
    match (a, b) {
        (Value::Str(x), Value::Str(y)) => Ok(x.cmp(y)),
        (Value::Char(x), Value::Char(y)) => Ok(x.cmp(y)),
        _ => {
            let xf = match want_num(a, name)? { Num::I(n) => n as f64, Num::D(f) => f };
            let yf = match want_num(b, name)? { Num::I(n) => n as f64, Num::D(f) => f };
            match xf.partial_cmp(&yf) {
                Some(o) => Ok(o),
                None => err(format!("`{name}`: values are not comparable")),
            }
        }
    }
}

pub fn call(interp: &Interp, name: &str, arg: Value, env: Option<&Env>) -> R<Value> {
    use std::cmp::Ordering::*;
    match name {
        "eq" => {
            let a = args_n(arg, 2, name)?;
            Ok(Value::Bool(a[0] == a[1]))
        }
        "lt" | "le" | "gt" | "ge" => {
            let a = args_n(arg, 2, name)?;
            let ord = compare(name, &a[0], &a[1])?;
            let b = match name {
                "lt" => ord == Less,
                "le" => ord != Greater,
                "gt" => ord == Greater,
                _ => ord != Less,
            };
            Ok(Value::Bool(b))
        }
        "not" => match arg {
            Value::Bool(b) => Ok(Value::Bool(!b)),
            other => err(format!("`not` expects a bool, got {}", print_value(&other))),
        },
        "add" => { let a = args_n(arg, 2, name)?; arith(name, &a[0], &a[1], i64::checked_add, |x, y| x + y) }
        "sub" => { let a = args_n(arg, 2, name)?; arith(name, &a[0], &a[1], i64::checked_sub, |x, y| x - y) }
        "mul" => { let a = args_n(arg, 2, name)?; arith(name, &a[0], &a[1], i64::checked_mul, |x, y| x * y) }
        "div" => { let a = args_n(arg, 2, name)?; arith(name, &a[0], &a[1], i64::checked_div, |x, y| x / y) }
        "rem" => { let a = args_n(arg, 2, name)?; arith(name, &a[0], &a[1], i64::checked_rem, |x, y| x % y) }
        "min" => { let a = args_n(arg, 2, name)?; if compare(name, &a[0], &a[1])? == Greater { Ok(a[1].clone()) } else { Ok(a[0].clone()) } }
        "max" => { let a = args_n(arg, 2, name)?; if compare(name, &a[0], &a[1])? == Less { Ok(a[1].clone()) } else { Ok(a[0].clone()) } }
        "neg" => match want_num(&arg, name)? {
            Num::I(n) => Ok(Value::Int(-n)),
            Num::D(f) => Ok(Value::Dec(-f)),
        },
        "abs" => match want_num(&arg, name)? {
            Num::I(n) => Ok(Value::Int(n.abs())),
            Num::D(f) => Ok(Value::Dec(f.abs())),
        },
        "len" => match &arg {
            Value::Lst(v) | Value::Tup(v) => Ok(Value::Int(v.len() as i64)),
            Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
            other => err(format!("`len` expects a list or string, got {}", print_value(other))),
        },
        "empty" => match &arg {
            Value::Lst(v) | Value::Tup(v) => Ok(Value::Bool(v.is_empty())),
            Value::Str(s) => Ok(Value::Bool(s.is_empty())),
            other => err(format!("`empty` expects a list or string, got {}", print_value(other))),
        },
        "get" => {
            let a = args_n(arg, 2, name)?;
            let idx = want_int(&a[1], name)?;
            match &a[0] {
                Value::Lst(v) | Value::Tup(v) => match v.get(idx as usize) {
                    Some(x) => Ok(x.clone()),
                    None => err(format!("`get`: index {idx} out of range")),
                },
                Value::Rec(fields) => err(format!(
                    "`get` on a record is not supported (record has {} fields)",
                    fields.len()
                )),
                other => err(format!("`get` expects a list, got {}", print_value(other))),
            }
        }
        "put" => {
            let mut a = args_n(arg, 3, name)?;
            let v = a.pop().unwrap();
            let idx = want_int(&a[1], name)? as usize;
            let mut lst = want_list(a.swap_remove(0), name)?;
            if idx >= lst.len() {
                return err(format!("`put`: index {idx} out of range"));
            }
            lst[idx] = v;
            Ok(Value::Lst(lst))
        }
        "push" => {
            let mut a = args_n(arg, 2, name)?;
            let v = a.pop().unwrap();
            let mut lst = want_list(a.pop().unwrap(), name)?;
            lst.push(v);
            Ok(Value::Lst(lst))
        }
        "concat" => {
            let mut a = args_n(arg, 2, name)?;
            let b = want_list(a.pop().unwrap(), name)?;
            let mut lst = want_list(a.pop().unwrap(), name)?;
            lst.extend(b);
            Ok(Value::Lst(lst))
        }
        "head" => {
            let lst = want_list(arg, name)?;
            match lst.into_iter().next() {
                Some(v) => Ok(v),
                None => err("`head` of empty list"),
            }
        }
        "tail" => {
            let lst = want_list(arg, name)?;
            if lst.is_empty() {
                return err("`tail` of empty list");
            }
            Ok(Value::Lst(lst[1..].to_vec()))
        }
        "reverse" => {
            let mut lst = want_list(arg, name)?;
            lst.reverse();
            Ok(Value::Lst(lst))
        }
        "range" => {
            let a = args_n(arg, 2, name)?;
            let lo = want_int(&a[0], name)?;
            let hi = want_int(&a[1], name)?;
            Ok(Value::Lst((lo..hi).map(Value::Int).collect()))
        }
        "zip" => {
            let mut a = args_n(arg, 2, name)?;
            let b = want_list(a.pop().unwrap(), name)?;
            let x = want_list(a.pop().unwrap(), name)?;
            Ok(Value::Lst(
                x.into_iter().zip(b).map(|(p, q)| Value::Tup(vec![p, q])).collect(),
            ))
        }
        "map" => {
            let mut a = args_n(arg, 2, name)?;
            let lst = want_list(a.pop().unwrap(), name)?;
            let f = a.pop().unwrap();
            let mut out = Vec::with_capacity(lst.len());
            for v in lst {
                out.push(interp.apply(&f, v)?);
            }
            Ok(Value::Lst(out))
        }
        "filter" => {
            let mut a = args_n(arg, 2, name)?;
            let lst = want_list(a.pop().unwrap(), name)?;
            let f = a.pop().unwrap();
            let mut out = Vec::new();
            for v in lst {
                match interp.apply(&f, v.clone())? {
                    Value::Bool(true) => out.push(v),
                    Value::Bool(false) => {}
                    other => {
                        return err(format!(
                            "`filter` predicate must return a bool, got {}",
                            print_value(&other)
                        ))
                    }
                }
            }
            Ok(Value::Lst(out))
        }
        "fold" => {
            let mut a = args_n(arg, 3, name)?;
            let lst = want_list(a.pop().unwrap(), name)?;
            let mut acc = a.pop().unwrap();
            let f = a.pop().unwrap();
            for v in lst {
                acc = interp.apply(&f, Value::Lst(vec![acc, v]))?;
            }
            Ok(acc)
        }
        "str-cat" => {
            let parts = payload_parts(arg);
            let mut out = String::new();
            for p in parts {
                match p {
                    Value::Str(s) => out.push_str(&s),
                    Value::Char(c) => out.push(c),
                    other => {
                        return err(format!(
                            "`str-cat` expects strings, got {} (use to-string)",
                            print_value(&other)
                        ))
                    }
                }
            }
            Ok(Value::Str(out))
        }
        "upper" => Ok(Value::Str(want_str(arg, name)?.to_uppercase())),
        "lower" => Ok(Value::Str(want_str(arg, name)?.to_lowercase())),
        "split" => {
            let mut a = args_n(arg, 2, name)?;
            let sep = want_str(a.pop().unwrap(), name)?;
            let s = want_str(a.pop().unwrap(), name)?;
            Ok(Value::Lst(s.split(&sep).map(|p| Value::Str(p.to_string())).collect()))
        }
        "join" => {
            let mut a = args_n(arg, 2, name)?;
            let sep = want_str(a.pop().unwrap(), name)?;
            let parts = want_list(a.pop().unwrap(), name)?;
            let strs: R<Vec<String>> = parts.into_iter().map(|p| want_str(p, name)).collect();
            Ok(Value::Str(strs?.join(&sep)))
        }
        "contains" => {
            let mut a = args_n(arg, 2, name)?;
            let sub = want_str(a.pop().unwrap(), name)?;
            let s = want_str(a.pop().unwrap(), name)?;
            Ok(Value::Bool(s.contains(&sub)))
        }
        "to-string" => Ok(Value::Str(print_value(&arg))),
        "read" => {
            let s = want_str(arg, name)?;
            match crate::read_file(&s) {
                Ok((arena, roots)) if roots.len() == 1 => {
                    Ok(Value::Variant("ok".into(), Some(Rc::new(form_to_value(&arena, roots[0])))))
                }
                Ok(_) => Ok(Value::Variant(
                    "err".into(),
                    Some(Rc::new(Value::Str("expected exactly one form".into()))),
                )),
                Err(e) => Ok(Value::Variant("err".into(), Some(Rc::new(Value::Str(e.to_string()))))),
            }
        }
        "to-u8" | "to-u16" | "to-u32" | "to-u64" | "to-s8" | "to-s16" | "to-s32" | "to-s64" => {
            let n = match &arg {
                Value::Int(n) => *n,
                Value::Dec(f) if f.fract() == 0.0 => *f as i64,
                other => return err(format!("`{name}` expects a number, got {}", print_value(other))),
            };
            let ok = match name {
                "to-u8" => (0..=u8::MAX as i64).contains(&n),
                "to-u16" => (0..=u16::MAX as i64).contains(&n),
                "to-u32" => (0..=u32::MAX as i64).contains(&n),
                "to-u64" => n >= 0,
                "to-s8" => (i8::MIN as i64..=i8::MAX as i64).contains(&n),
                "to-s16" => (i16::MIN as i64..=i16::MAX as i64).contains(&n),
                "to-s32" => (i32::MIN as i64..=i32::MAX as i64).contains(&n),
                _ => true,
            };
            if ok {
                Ok(Value::Int(n))
            } else {
                err(format!("`{name}`: {n} out of range"))
            }
        }
        "to-f32" | "to-f64" => match want_num(&arg, name)? {
            Num::I(n) => Ok(Value::Dec(n as f64)),
            Num::D(f) => Ok(Value::Dec(f)),
        },
        "apply" => {
            let mut a = args_n(arg, 2, name)?;
            let payload = a.pop().unwrap();
            let f = a.pop().unwrap();
            interp.apply(&f, payload)
        }
        "gensym" => {
            let n = interp.gensym.get();
            interp.gensym.set(n + 1);
            Ok(Value::Variant(format!("g{n}-gen"), None))
        }
        "expand" => {
            let Some(env) = env else {
                return err("`expand` needs an evaluation context (call it directly)");
            };
            // A quoted macro use is a tuple whose first element is the macro-name
            // symbol, e.g. `Quote And(p q)` ⇒ `(and-MACRO, p, q)`. Anything else
            // (an atom, a bare symbol, or an unknown head) is returned unchanged.
            let Value::Tup(items) = &arg else {
                return Ok(arg.clone());
            };
            let Some((Value::Variant(mac_name, None), rest)) = items.split_first() else {
                return Ok(arg.clone());
            };
            let Some(Value::Macro(mac)) = env.lookup(mac_name) else {
                return Ok(arg.clone());
            };
            let mut arena = crate::form::Arena::new();
            let mut arg_nodes = Vec::with_capacity(rest.len());
            for v in rest {
                arg_nodes
                    .push(crate::value::value_to_form(v, &mut arena).map_err(|msg| EvalError { msg })?);
            }
            let arena = Rc::new(arena);
            let (out, root) = interp.expand_once(&mac, &arena, &arg_nodes)?;
            Ok(form_to_value(&out, root))
        }
        "form-kind" => {
            let kind = match &arg {
                Value::Bool(_) => "bool",
                Value::Int(_) => "int",
                Value::Dec(_) => "dec",
                Value::Char(_) => "char",
                Value::Str(_) => "str",
                Value::Variant(_, None) => "sym",
                // A runtime variant carrying a payload (ok/err/some/…). Quoted
                // calls are tuples now, so they report "tup", not "call".
                Value::Variant(_, Some(_)) => "call",
                Value::Tup(_) => "tup",
                Value::Lst(_) => "lst",
                Value::Rec(_) => "rec",
                Value::Flg(_) => "flg",
                _ => return err("`form-kind` expects a form"),
            };
            Ok(Value::Str(kind.to_string()))
        }
        "rec-key" => match &arg {
            Value::Rec(fields) if !fields.is_empty() => {
                Ok(Value::Variant(fields[0].0.clone(), None))
            }
            other => err(format!("`rec-key` expects a non-empty record, got {}", print_value(other))),
        },
        "rec-val" => match &arg {
            Value::Rec(fields) if !fields.is_empty() => Ok(fields[0].1.clone()),
            other => err(format!("`rec-val` expects a non-empty record, got {}", print_value(other))),
        },
        "some" => Ok(Value::Variant("some".into(), Some(Rc::new(arg)))),
        "ok" => Ok(Value::Variant("ok".into(), Some(Rc::new(arg)))),
        "err" => Ok(Value::Variant("err".into(), Some(Rc::new(arg)))),
        "cell-new" => Ok(Value::Cell(Rc::new(RefCell::new(arg)))),
        "cell-get" => match &arg {
            Value::Cell(c) => Ok(c.borrow().clone()),
            other => err(format!("`cell-get` expects a cell, got {}", print_value(other))),
        },
        "cell-set" => {
            let mut a = args_n(arg, 2, name)?;
            let v = a.pop().unwrap();
            match a.pop().unwrap() {
                Value::Cell(c) => {
                    *c.borrow_mut() = v;
                    Ok(unit())
                }
                other => err(format!("`cell-set` expects a cell, got {}", print_value(&other))),
            }
        }
        "drop" => Ok(unit()),
        // ---- functor `Set` operations (see `SET_OPS`) -----------------------
        // A set is backed by a `Value::Cell` holding a `Value::Lst` of its
        // distinct elements: the `Cell`'s `Rc` gives the handle its shared,
        // mutable identity (so `add` is observed by a later `contains`/`size` on
        // the same handle), and element membership reuses `Value`'s `PartialEq`,
        // which is exactly the equality the `eq` builtin computes — so the set's
        // notion of "same element" agrees with the language's `eq`/`compare`.
        "set-new" => Ok(Value::Cell(Rc::new(RefCell::new(Value::Lst(Vec::new()))))),
        "set-add" => {
            let a = args_n(arg, 2, name)?;
            let mut elems = set_elems(&a[0], name)?;
            let elem = a[1].clone();
            if !elems.contains(&elem) {
                elems.push(elem);
                set_store(&a[0], elems);
            }
            Ok(unit())
        }
        "set-contains" => {
            let a = args_n(arg, 2, name)?;
            let elems = set_elems(&a[0], name)?;
            Ok(Value::Bool(elems.contains(&a[1])))
        }
        "set-size" => {
            let elems = set_elems(&arg, name)?;
            Ok(Value::Int(elems.len() as i64))
        }
        _ => err(format!("unknown builtin `{name}`")),
    }
}

/// Read the distinct elements out of a set handle (a `Value::Cell` of a `Lst`).
fn set_elems(v: &Value, name: &str) -> R<Vec<Value>> {
    match v {
        Value::Cell(c) => match &*c.borrow() {
            Value::Lst(items) => Ok(items.clone()),
            other => err(format!(
                "`{name}` expects a set handle, got a cell of {}",
                print_value(other)
            )),
        },
        other => err(format!("`{name}` expects a set handle, got {}", print_value(other))),
    }
}

/// Replace a set handle's elements in place, preserving the handle's identity.
fn set_store(v: &Value, elems: Vec<Value>) {
    if let Value::Cell(c) = v {
        *c.borrow_mut() = Value::Lst(elems);
    }
}
