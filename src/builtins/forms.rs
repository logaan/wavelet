use super::R;
use crate::interp::{EvalError, Interp, err};
use crate::value::{Env, Value, form_to_value, print_value};
use std::rc::Rc;

pub(super) fn call(interp: &Interp, name: &str, arg: Value, env: Option<&Env>) -> R<Value> {
    match name {
        "gensym" => {
            let n = interp.gensym.get();
            interp.gensym.set(n + 1);
            Ok(Value::Variant(format!("g{n}-gen"), None))
        }
        "expand" => {
            let Some(env) = env else {
                return err("`expand` needs an evaluation context (call it directly)");
            };

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
                arg_nodes.push(
                    crate::value::value_to_form(v, &mut arena).map_err(|msg| EvalError { msg })?,
                );
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
            other => err(format!(
                "`rec-key` expects a non-empty record, got {}",
                print_value(other)
            )),
        },
        "rec-val" => match &arg {
            Value::Rec(fields) if !fields.is_empty() => Ok(fields[0].1.clone()),
            other => err(format!(
                "`rec-val` expects a non-empty record, got {}",
                print_value(other)
            )),
        },
        _ => unreachable!("builtin routed to the wrong family"),
    }
}
