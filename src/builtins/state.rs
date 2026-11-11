use super::{R, args_n};
use crate::interp::err;
use crate::value::{Value, print_value, unit};
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn call(name: &str, arg: Value) -> R<Value> {
    match name {
        "cell-new" => Ok(Value::Cell(Rc::new(RefCell::new(arg)))),
        "cell-get" => match &arg {
            Value::Cell(c) => Ok(c.borrow().clone()),
            other => err(format!(
                "`cell-get` expects a cell, got {}",
                print_value(other)
            )),
        },
        "cell-set" => {
            let mut a = args_n(arg, 2, name)?;
            let v = a.pop().unwrap();
            match a.pop().unwrap() {
                Value::Cell(c) => {
                    *c.borrow_mut() = v;
                    Ok(unit())
                }
                other => err(format!(
                    "`cell-set` expects a cell, got {}",
                    print_value(&other)
                )),
            }
        }
        "drop" => Ok(unit()),

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
        _ => unreachable!("builtin routed to the wrong family"),
    }
}

fn set_elems(v: &Value, name: &str) -> R<Vec<Value>> {
    match v {
        Value::Cell(c) => match &*c.borrow() {
            Value::Lst(items) => Ok(items.clone()),
            other => err(format!(
                "`{name}` expects a set handle, got a cell of {}",
                print_value(other)
            )),
        },
        other => err(format!(
            "`{name}` expects a set handle, got {}",
            print_value(other)
        )),
    }
}

fn set_store(v: &Value, elems: Vec<Value>) {
    if let Value::Cell(c) = v {
        *c.borrow_mut() = Value::Lst(elems);
    }
}
