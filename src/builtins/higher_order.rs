use super::{R, args_n, want_list};
use crate::interp::{Interp, err};
use crate::value::{Value, print_value};

pub(super) fn call(interp: &Interp, name: &str, arg: Value) -> R<Value> {
    match name {
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
                        ));
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
                acc = interp.apply(&f, Value::Tup(vec![acc, v]))?;
            }
            Ok(acc)
        }
        _ => unreachable!("builtin routed to the wrong family"),
    }
}
