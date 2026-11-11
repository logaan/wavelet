use super::{R, args_n, want_int, want_list};
use crate::interp::err;
use crate::value::{Value, print_value};

pub(super) fn call(name: &str, arg: Value) -> R<Value> {
    match name {
        "len" => match &arg {
            Value::Lst(v) | Value::Tup(v) => Ok(Value::Int(v.len() as i64)),
            Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
            other => err(format!(
                "`len` expects a list or string, got {}",
                print_value(other)
            )),
        },
        "empty" => match &arg {
            Value::Lst(v) | Value::Tup(v) => Ok(Value::Bool(v.is_empty())),
            Value::Str(s) => Ok(Value::Bool(s.is_empty())),
            other => err(format!(
                "`empty` expects a list or string, got {}",
                print_value(other)
            )),
        },
        "get" => get(arg),
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
                x.into_iter()
                    .zip(b)
                    .map(|(p, q)| Value::Tup(vec![p, q]))
                    .collect(),
            ))
        }
        _ => unreachable!("builtin routed to the wrong family"),
    }
}

fn get(arg: Value) -> R<Value> {
    let a = args_n(arg, 2, "get")?;
    let idx = want_int(&a[1], "get")?;
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
