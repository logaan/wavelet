use super::{R, args_n, payload_parts, want_list, want_str};
use crate::interp::err;
use crate::value::{Value, print_value};

pub(super) fn call(name: &str, arg: Value) -> R<Value> {
    match name {
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
                        ));
                    }
                }
            }
            Ok(Value::Str(out))
        }
        "split" => {
            let mut a = args_n(arg, 2, name)?;
            let sep = want_str(a.pop().unwrap(), name)?;
            let s = want_str(a.pop().unwrap(), name)?;
            Ok(Value::Lst(
                s.split(&sep).map(|p| Value::Str(p.to_string())).collect(),
            ))
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
        _ => unreachable!("builtin routed to the wrong family"),
    }
}
