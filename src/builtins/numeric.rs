use super::{R, args_n};
use crate::interp::{EvalError, err};
use crate::value::{Value, print_value};
use std::cmp::Ordering::*;

pub(super) fn call(name: &str, arg: Value) -> R<Value> {
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
        "add" => arith(name, arg, i64::checked_add, |x, y| x + y),
        "sub" => arith(name, arg, i64::checked_sub, |x, y| x - y),
        "mul" => arith(name, arg, i64::checked_mul, |x, y| x * y),
        "div" => arith(name, arg, i64::checked_div, |x, y| x / y),
        "rem" => arith(name, arg, i64::checked_rem, |x, y| x % y),
        "min" | "max" => {
            let a = args_n(arg, 2, name)?;
            let replace = if name == "min" { Greater } else { Less };
            if compare(name, &a[0], &a[1])? == replace {
                Ok(a[1].clone())
            } else {
                Ok(a[0].clone())
            }
        }
        "neg" => unary(name, arg, i64::checked_neg, |f| -f),
        "abs" => unary(name, arg, i64::checked_abs, f64::abs),
        _ => unreachable!("builtin routed to the wrong family"),
    }
}

pub(super) enum Num {
    I(i64),
    D(f64),
}

impl Num {
    pub(super) fn as_float(&self) -> f64 {
        match self {
            Self::I(n) => *n as f64,
            Self::D(f) => *f,
        }
    }
}

pub(super) fn want_num(v: &Value, name: &str) -> R<Num> {
    match v {
        Value::Int(n) => Ok(Num::I(*n)),
        Value::Dec(f) => Ok(Num::D(*f)),
        other => err(format!(
            "`{name}` expects a number, got {}",
            print_value(other)
        )),
    }
}

fn arith(
    name: &str,
    arg: Value,
    fi: fn(i64, i64) -> Option<i64>,
    fd: fn(f64, f64) -> f64,
) -> R<Value> {
    let a = args_n(arg, 2, name)?;
    match (want_num(&a[0], name)?, want_num(&a[1], name)?) {
        (Num::I(x), Num::I(y)) => match fi(x, y) {
            Some(n) => Ok(Value::Int(n)),
            None => err(format!("`{name}`: integer overflow or division by zero")),
        },
        (x, y) => Ok(Value::Dec(fd(x.as_float(), y.as_float()))),
    }
}

fn compare(name: &str, a: &Value, b: &Value) -> R<std::cmp::Ordering> {
    match (a, b) {
        (Value::Str(x), Value::Str(y)) => Ok(x.cmp(y)),
        (Value::Char(x), Value::Char(y)) => Ok(x.cmp(y)),
        _ => {
            let xf = want_num(a, name)?.as_float();
            let yf = want_num(b, name)?.as_float();
            match xf.partial_cmp(&yf) {
                Some(o) => Ok(o),
                None => err(format!("`{name}`: values are not comparable")),
            }
        }
    }
}

fn unary(name: &str, arg: Value, fi: fn(i64) -> Option<i64>, fd: fn(f64) -> f64) -> R<Value> {
    match want_num(&arg, name)? {
        Num::I(n) => fi(n).map(Value::Int).ok_or_else(|| EvalError {
            msg: format!("`{name}`: integer overflow"),
        }),
        Num::D(f) => Ok(Value::Dec(fd(f))),
    }
}
