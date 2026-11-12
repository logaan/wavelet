use super::{R, args_n};
use crate::interp::err;
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
                "lt" => ord == Some(Less),
                "le" => matches!(ord, Some(Less | Equal)),
                "gt" => ord == Some(Greater),
                _ => matches!(ord, Some(Greater | Equal)),
            };
            Ok(Value::Bool(b))
        }
        "not" => match arg {
            Value::Bool(b) => Ok(Value::Bool(!b)),
            other => err(format!("`not` expects a bool, got {}", print_value(&other))),
        },
        "add" => arith(name, arg, |x, y| Some(x.wrapping_add(y)), |x, y| x + y),
        "sub" => arith(name, arg, |x, y| Some(x.wrapping_sub(y)), |x, y| x - y),
        "mul" => arith(name, arg, |x, y| Some(x.wrapping_mul(y)), |x, y| x * y),
        "div" => arith(name, arg, i64::checked_div, |x, y| x / y),
        "rem" => {
            let a = args_n(arg, 2, name)?;
            match (&a[0], &a[1]) {
                (Value::Int(x), Value::Int(y)) if *y != 0 => Ok(Value::Int(x.wrapping_rem(*y))),
                (Value::Int(_), Value::Int(_)) => err("`rem`: integer division by zero"),
                _ => err("`rem` requires integer operands"),
            }
        }
        "min" | "max" => {
            let a = args_n(arg, 2, name)?;
            match (&a[0], &a[1]) {
                (Value::Dec(_), _) | (_, Value::Dec(_)) => {
                    let x = want_num(&a[0], name)?.as_float();
                    let y = want_num(&a[1], name)?.as_float();
                    let result = if x.is_nan() || y.is_nan() {
                        f64::NAN
                    } else if x == 0.0 && y == 0.0 {
                        let bits = if name == "min" {
                            x.to_bits() | y.to_bits()
                        } else {
                            x.to_bits() & y.to_bits()
                        };
                        f64::from_bits(bits)
                    } else if name == "min" {
                        x.min(y)
                    } else {
                        x.max(y)
                    };
                    Ok(Value::Dec(result))
                }
                _ => {
                    let replace = if name == "min" { Greater } else { Less };
                    if compare(name, &a[0], &a[1])? == Some(replace) {
                        Ok(a[1].clone())
                    } else {
                        Ok(a[0].clone())
                    }
                }
            }
        }
        "neg" => unary(name, arg, i64::wrapping_neg, |f| -f),
        "abs" => unary(name, arg, i64::wrapping_abs, f64::abs),
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

fn compare(name: &str, a: &Value, b: &Value) -> R<Option<std::cmp::Ordering>> {
    match (a, b) {
        (Value::Str(x), Value::Str(y)) => Ok(Some(x.cmp(y))),
        (Value::Char(x), Value::Char(y)) => Ok(Some(x.cmp(y))),
        (Value::Int(x), Value::Int(y)) => Ok(Some(x.cmp(y))),
        _ => {
            let xf = want_num(a, name)?.as_float();
            let yf = want_num(b, name)?.as_float();
            Ok(xf.partial_cmp(&yf))
        }
    }
}

fn unary(name: &str, arg: Value, fi: fn(i64) -> i64, fd: fn(f64) -> f64) -> R<Value> {
    match want_num(&arg, name)? {
        Num::I(n) => Ok(Value::Int(fi(n))),
        Num::D(f) => Ok(Value::Dec(fd(f))),
    }
}
