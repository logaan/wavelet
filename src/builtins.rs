mod collections;
mod convert;
mod forms;
mod higher_order;
mod numeric;
mod state;
mod text;

use std::rc::Rc;

use crate::form::{Arena, Node, NodeId};
use crate::interp::{EvalError, Interp, err};
use crate::value::{Env, Value, print_value};

type R<T> = Result<T, EvalError>;

pub const NAMES: &[&str] = &[
    "eq",
    "lt",
    "le",
    "gt",
    "ge",
    "not",
    "add",
    "sub",
    "mul",
    "div",
    "rem",
    "neg",
    "min",
    "max",
    "abs",
    "len",
    "empty",
    "get",
    "put",
    "push",
    "concat",
    "head",
    "tail",
    "reverse",
    "range",
    "map",
    "filter",
    "fold",
    "zip",
    "str-cat",
    "split",
    "join",
    "contains",
    "to-string",
    "to-u8",
    "to-u16",
    "to-u32",
    "to-u64",
    "to-s8",
    "to-s16",
    "to-s32",
    "to-s64",
    "to-f32",
    "to-f64",
    "to-char",
    "gensym",
    "expand",
    "form-kind",
    "rec-key",
    "rec-val",
    "some",
    "ok",
    "err",
    "cell-new",
    "cell-get",
    "cell-set",
    "drop",
    "tuple0",
    "tuple1",
    "tuple2",
    "tuple3",
    "tuple4",
    "tuple5",
    "tuple6",
    "tuple7",
    "tuple8",
    "tuple9",
    "tuple10",
    "tuple11",
    "tuple12",
    "tuple13",
    "tuple14",
    "tuple15",
    "tuple16",
];

pub fn tuple_ctor_arity(name: &str) -> Option<usize> {
    let n: usize = name.strip_prefix("tuple")?.parse().ok()?;
    (NAMES.contains(&name)).then_some(n)
}

pub const SET_OPS: &[&str] = &["set-new", "set-add", "set-contains", "set-size"];

pub fn install(env: &Env) {
    for name in NAMES {
        env.define(*name, Value::Builtin(name));
    }
    env.define("none", Value::Variant("none".into(), None));
    env.define("pi", Value::Dec(std::f64::consts::PI));
    install_stdlib_macros(env);
}

const STDLIB_MACROS: &[(&str, &[&str], &str)] = &[
    (
        "if-MACRO",
        &["c", "t", "e"],
        "Quasi Match The bool Unquote(c) [(true Unquote(t)) (false Unquote(e))]",
    ),
    (
        "do-MACRO",
        &["stmts"],
        "Match empty(stmts) [
          (true  {})
          (false Match empty(tail(stmts)) [
            (true  head(stmts))
            (false Quasi Match Unquote(head(stmts)) [(_ Do [Splice(tail(stmts))])])
          ])
        ]",
    ),
];

fn install_stdlib_macros(env: &Env) {
    use crate::value::{Closure, Param};
    for (name, params, body_src) in STDLIB_MACROS {
        let (arena, roots) =
            crate::reader::read_file(body_src).expect("bundled stdlib macro body must read");
        assert_eq!(roots.len(), 1, "a stdlib macro body must be a single form");
        let closure = Closure {
            params: params
                .iter()
                .map(|p| Param {
                    name: (*p).to_string(),
                    resource_type: None,
                })
                .collect(),
            body: roots[0],
            arena: Rc::new(arena),
            env: env.clone(),
        };
        env.define((*name).to_string(), Value::Macro(Rc::new(closure)));
    }
}

pub enum FunctorKind {
    Set,
}

pub struct FunctorImport {
    pub alias: String,
    pub kind: FunctorKind,
}

pub fn parse_functor_import(arena: &Arena, payload: NodeId) -> Option<FunctorImport> {
    let Node::Rec(fields) = arena.node(payload) else {
        return None;
    };
    let pkg = fields
        .iter()
        .find_map(|(k, v)| match (k.as_str(), arena.node(*v)) {
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

pub fn bind_functor(env: &Env, functor: &FunctorImport) {
    match functor.kind {
        FunctorKind::Set => {
            for (op, builtin) in [
                ("new", "set-new"),
                ("add", "set-add"),
                ("contains", "set-contains"),
                ("size", "set-size"),
            ] {
                env.define(format!("{}-{op}", functor.alias), Value::Builtin(builtin));
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
        other => err(format!(
            "`{name}` expects a list, got {}",
            print_value(&other)
        )),
    }
}

fn payload_parts(v: Value) -> Vec<Value> {
    match v {
        Value::Lst(items) | Value::Tup(items) => items,
        other => vec![other],
    }
}

fn want_str(v: Value, name: &str) -> R<String> {
    match v {
        Value::Str(s) => Ok(s),
        other => err(format!(
            "`{name}` expects a string, got {}",
            print_value(&other)
        )),
    }
}

fn want_int(v: &Value, name: &str) -> R<i64> {
    match v {
        Value::Int(n) => Ok(*n),
        other => err(format!(
            "`{name}` expects an integer, got {}",
            print_value(other)
        )),
    }
}

pub fn call(interp: &Interp, name: &str, arg: Value, env: Option<&Env>) -> R<Value> {
    match name {
        "eq" | "lt" | "le" | "gt" | "ge" | "not" | "add" | "sub" | "mul" | "div" | "rem"
        | "neg" | "min" | "max" | "abs" => numeric::call(name, arg),
        "len" | "empty" | "get" | "put" | "push" | "concat" | "head" | "tail" | "reverse"
        | "range" | "zip" => collections::call(name, arg),
        "map" | "filter" | "fold" => higher_order::call(interp, name, arg),
        "str-cat" | "split" | "join" | "contains" | "to-string" => text::call(name, arg),
        "to-u8" | "to-u16" | "to-u32" | "to-u64" | "to-s8" | "to-s16" | "to-s32" | "to-s64"
        | "to-f32" | "to-f64" | "to-char" => convert::call(name, arg),
        "gensym" | "expand" | "form-kind" | "rec-key" | "rec-val" => {
            forms::call(interp, name, arg, env)
        }
        "cell-new" | "cell-get" | "cell-set" | "drop" | "set-new" | "set-add" | "set-contains"
        | "set-size" => state::call(name, arg),
        "some" => Ok(Value::Variant("some".into(), Some(Rc::new(arg)))),

        "ok" | "err" => Ok(match arg {
            Value::Tup(items) if items.is_empty() => Value::Variant(name.into(), None),
            payload => Value::Variant(name.into(), Some(Rc::new(payload))),
        }),
        _ if tuple_ctor_arity(name) == Some(1) => Ok(Value::Tup(vec![arg])),
        _ if tuple_ctor_arity(name).is_some() => {
            let n = tuple_ctor_arity(name).unwrap();
            Ok(Value::Tup(args_n(arg, n, name)?))
        }

        _ => err(format!("unknown builtin `{name}`")),
    }
}
