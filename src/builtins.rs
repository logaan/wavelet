use std::cell::RefCell;
use std::rc::Rc;

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
    "print", "println", "read-line", "args", "env",
    "apply", "gensym", "expand",
    "form-kind", "rec-key", "rec-val",
    "some", "ok", "err",
    "cell-new", "cell-get", "cell-set", "drop",
];

pub fn install(env: &Env) {
    for name in NAMES {
        env.define(*name, Value::Builtin(name));
    }
    env.define("none", Value::Variant("none".into(), None));
    env.define("pi", Value::Dec(std::f64::consts::PI));
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
            let parts = want_list(arg, name)?;
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
        "print" | "println" => {
            let text = match &arg {
                Value::Str(s) => s.clone(),
                other => print_value(other),
            };
            crate::emit_output(&text, name == "println");
            Ok(unit())
        }
        "read-line" => {
            let mut line = String::new();
            match std::io::stdin().read_line(&mut line) {
                Ok(_) => Ok(Value::Str(line.trim_end_matches(['\n', '\r']).to_string())),
                Err(e) => err(format!("read-line: {e}")),
            }
        }
        "args" => Ok(Value::Lst(
            interp.prog_args.iter().map(|a| Value::Str(a.clone())).collect(),
        )),
        "env" => Ok(Value::Lst(
            std::env::vars()
                .map(|(k, v)| Value::Tup(vec![Value::Str(k), Value::Str(v)]))
                .collect(),
        )),
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
            match &arg {
                Value::Variant(head, payload) => match env.lookup(head) {
                    Some(Value::Macro(mac)) => {
                        let mut arena = crate::form::Arena::new();
                        let pid = match payload {
                            Some(p) => crate::value::value_to_form(p, &mut arena)
                                .map_err(|msg| EvalError { msg })?,
                            None => arena.add(crate::form::Node::Lst(vec![]), (0, 0)),
                        };
                        let arena = Rc::new(arena);
                        let (out, root) = interp.expand_once(&mac, &arena, pid)?;
                        Ok(form_to_value(&out, root))
                    }
                    _ => Ok(arg.clone()),
                },
                _ => Ok(arg.clone()),
            }
        }
        "form-kind" => {
            let kind = match &arg {
                Value::Bool(_) => "bool",
                Value::Int(_) => "int",
                Value::Dec(_) => "dec",
                Value::Char(_) => "char",
                Value::Str(_) => "str",
                Value::Variant(_, None) => "sym",
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
        _ => err(format!("unknown builtin `{name}`")),
    }
}
