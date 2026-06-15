use std::cell::Cell;
use std::fmt;
use std::rc::Rc;

use crate::builtins;
use crate::form::{Arena, Node, NodeId};
use crate::value::{form_to_value, unit, value_to_form, Closure, Env, Param, Value};

#[derive(Debug)]
pub struct EvalError {
    pub msg: String,
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "eval error: {}", self.msg)
    }
}

impl std::error::Error for EvalError {}

pub fn err<T>(msg: impl Into<String>) -> Result<T, EvalError> {
    Err(EvalError { msg: msg.into() })
}

type R<T> = Result<T, EvalError>;

enum Step {
    Done(Value),
    /// Continue the eval loop at a new position: tail-call elimination (§5).
    Jump(Rc<Arena>, NodeId, Env),
}

pub struct Interp {
    pub gensym: Cell<u64>,
}

impl Default for Interp {
    fn default() -> Self {
        Self::new()
    }
}

impl Interp {
    pub fn new() -> Self {
        Self { gensym: Cell::new(0) }
    }

    pub fn eval(&self, arena: &Rc<Arena>, id: NodeId, env: &Env) -> R<Value> {
        let mut arena = arena.clone();
        let mut id = id;
        let mut env = env.clone();
        loop {
            match self.step(&arena, id, &env)? {
                Step::Done(v) => return Ok(v),
                Step::Jump(a, i, e) => {
                    arena = a;
                    id = i;
                    env = e;
                }
            }
        }
    }

    pub fn apply(&self, f: &Value, arg: Value) -> R<Value> {
        match self.apply_step(f, arg, None)? {
            Step::Done(v) => Ok(v),
            Step::Jump(a, i, e) => self.eval(&a, i, &e),
        }
    }

    fn step(&self, arena: &Rc<Arena>, id: NodeId, env: &Env) -> R<Step> {
        match arena.node(id) {
            Node::Bool(b) => Ok(Step::Done(Value::Bool(*b))),
            Node::Int(n) => Ok(Step::Done(Value::Int(*n))),
            Node::Dec(f) => Ok(Step::Done(Value::Dec(*f))),
            Node::Char(c) => Ok(Step::Done(Value::Char(*c))),
            Node::Str(s) => Ok(Step::Done(Value::Str(s.clone()))),
            Node::Flg(names) => Ok(Step::Done(Value::Flg(names.clone()))),
            Node::Sym(name) => match env.lookup(name) {
                Some(v) => Ok(Step::Done(v)),
                None => err(format!("unbound name `{name}`")),
            },
            Node::Qsym(alias, name) => {
                let q = format!("{alias}/{name}");
                match env.lookup(&q) {
                    Some(v) => Ok(Step::Done(v)),
                    None => err(format!("unbound qualified name `{q}` (missing import?)")),
                }
            }
            // A parenthesized form in evaluation position is a call (§4.1):
            // take items[0] as the head and apply it to the bundled args.
            Node::Tup(items) => self.step_call(arena, items, env),
            Node::Lst(items) => {
                let vals = self.eval_each(arena, items, env)?;
                Ok(Step::Done(Value::Lst(vals)))
            }
            Node::Rec(fields) => {
                let mut out = Vec::with_capacity(fields.len());
                for (k, v) in fields {
                    out.push((k.clone(), self.eval(arena, *v, env)?));
                }
                Ok(Step::Done(Value::Rec(out)))
            }
            // The reader no longer emits `Node::Call`; calls are tuples.
            Node::Call(..) => unreachable!("Node::Call is no longer produced by the reader"),
        }
    }

    fn eval_each(&self, arena: &Rc<Arena>, items: &[NodeId], env: &Env) -> R<Vec<Value>> {
        items.iter().map(|&i| self.eval(arena, i, env)).collect()
    }

    fn step_call(&self, arena: &Rc<Arena>, items: &[NodeId], env: &Env) -> R<Step> {
        let Some((&head, args)) = items.split_first() else {
            return err("cannot evaluate empty form ()");
        };
        let name = match arena.node(head) {
            Node::Sym(s) => s.clone(),
            Node::Qsym(a, n) => format!("{a}/{n}"),
            _ => return err("call head must be a name (use apply for a computed function)"),
        };
        if let Some(step) = self.special_form(&name, arena, args, env)? {
            return Ok(step);
        }
        let f = match env.lookup(&name) {
            Some(v) => v,
            None => return err(format!("unbound name `{name}` in call position")),
        };
        if let Value::Macro(c) = &f {
            return self.expand_macro(c, arena, args, env);
        }
        let arg = self.bundle_args(arena, args, env)?;
        self.apply_step(&f, arg, Some(env))
    }

    /// §4.2 argument bundling at a call: 0 args ⇒ the empty tuple, 1 arg ⇒ that
    /// value directly, ≥2 args ⇒ a tuple. This reproduces the old payload shape,
    /// so `bind_params` is unchanged.
    fn bundle_args(&self, arena: &Rc<Arena>, args: &[NodeId], env: &Env) -> R<Value> {
        match args {
            [] => Ok(Value::Tup(vec![])),
            [one] => self.eval(arena, *one, env),
            many => Ok(Value::Tup(self.eval_each(arena, many, env)?)),
        }
    }

    /// `env` is the caller's environment, used only by builtins that need to
    /// see macro bindings (`expand`).
    fn apply_step(&self, f: &Value, arg: Value, env: Option<&Env>) -> R<Step> {
        match f {
            Value::Closure(c) => {
                let env = bind_params(c, arg)?;
                Ok(Step::Jump(c.arena.clone(), c.body, env))
            }
            Value::Builtin(name) => Ok(Step::Done(builtins::call(self, name, arg, env)?)),
            other => err(format!(
                "not callable: {}",
                crate::value::print_value(other)
            )),
        }
    }

    fn special_form(
        &self,
        name: &str,
        arena: &Rc<Arena>,
        args: &[NodeId],
        env: &Env,
    ) -> R<Option<Step>> {
        let step = match name {
            "def-MACRO" => {
                let [name_id, expr] = args2(args, "Def")?;
                let Node::Sym(n) = arena.node(name_id) else {
                    return err("Def expects a name");
                };
                let v = self.eval(arena, expr, env)?;
                env.define(n.clone(), v);
                Step::Done(unit())
            }
            "fn-MACRO" => {
                let [params_id, body] = args2(args, "Fn")?;
                let params = parse_params(arena, params_id)?;
                Step::Done(Value::Closure(Rc::new(Closure {
                    params,
                    body,
                    arena: arena.clone(),
                    env: env.clone(),
                })))
            }
            "if-MACRO" => {
                let [c, t, e] = args3(args, "If")?;
                match self.eval(arena, c, env)? {
                    Value::Bool(true) => Step::Jump(arena.clone(), t, env.clone()),
                    Value::Bool(false) => Step::Jump(arena.clone(), e, env.clone()),
                    v => return err(format!(
                        "If condition must be a bool, got {}",
                        crate::value::print_value(&v)
                    )),
                }
            }
            "let-MACRO" => {
                let [bindings, body] = args2(args, "Let")?;
                let child = env.child();
                match arena.node(bindings) {
                    Node::Rec(fields) => {
                        for (k, v) in fields {
                            let val = self.eval(arena, *v, &child)?;
                            child.define(k.clone(), val);
                        }
                    }
                    Node::Flg(names) if names.is_empty() => {}
                    _ => return err("Let expects a record of bindings"),
                }
                Step::Jump(arena.clone(), body, child)
            }
            "do-MACRO" => {
                let [list] = args1(args, "Do")?;
                let Node::Lst(items) = arena.node(list) else {
                    return err("Do expects a list of expressions");
                };
                match items.split_last() {
                    None => Step::Done(unit()),
                    Some((last, init)) => {
                        for &i in init {
                            self.eval(arena, i, env)?;
                        }
                        Step::Jump(arena.clone(), *last, env.clone())
                    }
                }
            }
            "match-MACRO" => {
                let [scrut, clauses] = args2(args, "Match")?;
                let v = self.eval(arena, scrut, env)?;
                let Node::Lst(items) = arena.node(clauses) else {
                    return err("Match expects a list of (pattern result) clauses");
                };
                for &clause in items {
                    let Node::Tup(pair) = arena.node(clause) else {
                        return err("each Match clause must be a (pattern result) tuple");
                    };
                    if pair.len() != 2 {
                        return err("each Match clause must be a (pattern result) tuple");
                    }
                    let child = env.child();
                    if match_pattern(arena, pair[0], &v, &child, env)? {
                        return Ok(Some(Step::Jump(arena.clone(), pair[1], child)));
                    }
                }
                return err(format!(
                    "no Match clause for {}",
                    crate::value::print_value(&v)
                ));
            }
            "quote-MACRO" => {
                let [form] = args1(args, "Quote")?;
                Step::Done(form_to_value(arena, form))
            }
            "quasi-MACRO" => {
                let [form] = args1(args, "Quasi")?;
                Step::Done(self.quasi(arena, form, env, 1)?)
            }
            "unquote-MACRO" | "splice-MACRO" => {
                return err("Unquote/Splice are only valid inside Quasi");
            }
            "def-macro-MACRO" => {
                let [name_id, params_id, body] = args3(args, "DefMacro")?;
                let Node::Sym(n) = arena.node(name_id) else {
                    return err("DefMacro expects a name");
                };
                let params = parse_params(arena, params_id)?;
                let mac = Value::Macro(Rc::new(Closure {
                    params,
                    body,
                    arena: arena.clone(),
                    env: env.clone(),
                }));
                env.define(format!("{n}-MACRO"), mac);
                Step::Done(unit())
            }
            "the-MACRO" => {
                let [ty, expr] = args2(args, "The")?;
                let v = self.eval(arena, expr, env)?;
                if let Node::Sym(t) = arena.node(ty) {
                    if !check_type(t, &v) {
                        return err(format!(
                            "The: {} does not conform to type `{t}`",
                            crate::value::print_value(&v)
                        ));
                    }
                }
                Step::Done(v)
            }
            "package-MACRO" | "import-MACRO" | "export-MACRO" | "def-type-MACRO" => {
                return err(format!(
                    "`{}` is only allowed at the top level of a file",
                    name.trim_end_matches("-MACRO")
                ));
            }
            _ => return Ok(None),
        };
        Ok(Some(step))
    }

    /// Expand a user macro: bind argument *forms* (as data) to the macro's
    /// parameters, evaluate its body, and jump into the resulting form (§6.3).
    fn expand_macro(
        &self,
        mac: &Rc<Closure>,
        arena: &Rc<Arena>,
        args: &[NodeId],
        use_env: &Env,
    ) -> R<Step> {
        let (out, root) = self.expand_once(mac, arena, args)?;
        Ok(Step::Jump(out, root, use_env.clone()))
    }

    /// One macro expansion step: bind the argument *forms* to the macro's
    /// parameters, evaluate the body, and return the resulting form in a fresh
    /// arena. Also used by the ahead-of-time expander (`crate::expand`).
    pub fn expand_once(
        &self,
        mac: &Rc<Closure>,
        arena: &Rc<Arena>,
        args: &[NodeId],
    ) -> R<(Rc<Arena>, NodeId)> {
        let n = mac.params.len();
        let env = mac.env.child();
        if n == args.len() {
            for (param, &arg) in mac.params.iter().zip(args) {
                env.define(param.name.clone(), form_to_value(arena, arg));
            }
        } else if n == 1 {
            // A 1-param macro receiving several explicit args gets them as a
            // tuple form (rare, but well-defined).
            let tup = Value::Tup(args.iter().map(|&a| form_to_value(arena, a)).collect());
            env.define(mac.params[0].name.clone(), tup);
        } else {
            return err(format!("macro expects {n} arguments"));
        }
        let result = self.eval(&mac.arena, mac.body, &env)?;
        let mut out = Arena::new();
        let root = value_to_form(&result, &mut out).map_err(|msg| EvalError { msg })?;
        Ok((Rc::new(out), root))
    }

    /// `Quasi`: build a form, evaluating `Unquote` holes and splicing
    /// `Splice` holes into the enclosing sequence.
    /// `depth` counts enclosing Quasis: Unquote/Splice fire at depth 1 and are
    /// rebuilt as data (one level shallower) at greater depths; a nested Quasi
    /// is rebuilt with its contents processed one level deeper.
    fn quasi(&self, arena: &Rc<Arena>, id: NodeId, env: &Env, depth: u32) -> R<Value> {
        match arena.node(id) {
            // Calls are tuples now. Treat the Unquote/Splice/Quasi heads
            // specially (all arity 1, so a 2-element tuple); every other tuple
            // is rebuilt element-wise as a tuple value.
            Node::Tup(items) => {
                if items.len() == 2 {
                    if let Node::Sym(name) = arena.node(items[0]) {
                        let arg = items[1];
                        match name.as_str() {
                            "unquote-MACRO" if depth == 1 => return self.eval(arena, arg, env),
                            "splice-MACRO" if depth == 1 => {
                                return err("Splice must appear inside a sequence");
                            }
                            "unquote-MACRO" | "splice-MACRO" if depth > 1 => {
                                let inner = self.quasi(arena, arg, env, depth - 1)?;
                                return Ok(Value::Tup(vec![
                                    Value::Variant(name.clone(), None),
                                    inner,
                                ]));
                            }
                            "quasi-MACRO" => {
                                let inner = self.quasi(arena, arg, env, depth + 1)?;
                                return Ok(Value::Tup(vec![
                                    Value::Variant(name.clone(), None),
                                    inner,
                                ]));
                            }
                            _ => {}
                        }
                    }
                }
                Ok(Value::Tup(self.quasi_seq(arena, items, env, depth)?))
            }
            Node::Call(..) => unreachable!("Node::Call is no longer produced by the reader"),
            Node::Lst(items) => Ok(Value::Lst(self.quasi_seq(arena, items, env, depth)?)),
            Node::Rec(fields) => {
                let mut out = Vec::with_capacity(fields.len());
                for (k, v) in fields {
                    out.push((k.clone(), self.quasi(arena, *v, env, depth)?));
                }
                Ok(Value::Rec(out))
            }
            _ => Ok(form_to_value(arena, id)),
        }
    }

    fn quasi_seq(
        &self,
        arena: &Rc<Arena>,
        items: &[NodeId],
        env: &Env,
        depth: u32,
    ) -> R<Vec<Value>> {
        let mut out = Vec::with_capacity(items.len());
        for &item in items {
            if depth == 1 {
                // A splice is `(Splice expr)` ⇒ the tuple `[splice-MACRO, expr]`.
                if let Node::Tup(tup) = arena.node(item) {
                    if tup.len() == 2 {
                        if let Node::Sym(s) = arena.node(tup[0]) {
                            if s == "splice-MACRO" {
                                match self.eval(arena, tup[1], env)? {
                                    Value::Lst(vs) => out.extend(vs),
                                    v => {
                                        return err(format!(
                                            "Splice expects a list, got {}",
                                            crate::value::print_value(&v)
                                        ))
                                    }
                                }
                                continue;
                            }
                        }
                    }
                }
            }
            out.push(self.quasi(arena, item, env, depth)?);
        }
        Ok(out)
    }
}

/// §4.2 parameter binding: record payloads bind by name, list/tuple payloads
/// by order, and a sole parameter receives the payload directly.
fn bind_params(c: &Closure, arg: Value) -> R<Env> {
    let env = c.env.child();
    let n = c.params.len();
    match (n, &arg) {
        (0, Value::Lst(v)) | (0, Value::Tup(v)) if v.is_empty() => return Ok(env),
        (0, Value::Rec(f)) if f.is_empty() => return Ok(env),
        (0, Value::Flg(f)) if f.is_empty() => return Ok(env),
        (0, _) => return err("function takes no arguments"),
        _ => {}
    }
    if let Value::Rec(fields) = &arg {
        let names: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
        let param_names: Vec<&str> = c.params.iter().map(|p| p.name.as_str()).collect();
        let mut sorted_a = names.clone();
        let mut sorted_b = param_names.clone();
        sorted_a.sort_unstable();
        sorted_b.sort_unstable();
        if sorted_a == sorted_b {
            for p in &c.params {
                let v = fields.iter().find(|(k, _)| *k == p.name).unwrap().1.clone();
                bind_one(&env, p, v)?;
            }
            return Ok(env);
        }
    }
    if n == 1 {
        bind_one(&env, &c.params[0], arg)?;
        return Ok(env);
    }
    match arg {
        Value::Lst(vs) | Value::Tup(vs) if vs.len() == n => {
            for (p, v) in c.params.iter().zip(vs) {
                bind_one(&env, p, v)?;
            }
            Ok(env)
        }
        other => err(format!(
            "cannot bind {} parameters from payload {}",
            n,
            crate::value::print_value(&other)
        )),
    }
}

fn bind_one(env: &Env, p: &Param, v: Value) -> R<()> {
    if let Some(ty) = &p.ty {
        if !check_type(ty, &v) {
            return err(format!(
                "parameter `{}`: {} does not conform to type `{ty}`",
                p.name,
                crate::value::print_value(&v)
            ));
        }
    }
    env.define(p.name.clone(), v);
    Ok(())
}

fn parse_params(arena: &Arena, id: NodeId) -> R<Vec<Param>> {
    match arena.node(id) {
        Node::Flg(names) => Ok(names
            .iter()
            .map(|n| Param { name: n.clone(), ty: None })
            .collect()),
        Node::Rec(fields) => Ok(fields
            .iter()
            .map(|(k, v)| Param {
                name: k.clone(),
                ty: match arena.node(*v) {
                    Node::Sym(t) => Some(t.clone()),
                    _ => None,
                },
            })
            .collect()),
        _ => err("Fn expects parameter braces"),
    }
}

fn check_type(ty: &str, v: &Value) -> bool {
    match ty {
        "string" => matches!(v, Value::Str(_)),
        "bool" => matches!(v, Value::Bool(_)),
        "char" => matches!(v, Value::Char(_)),
        "f32" | "f64" => matches!(v, Value::Dec(_) | Value::Int(_)),
        "u8" => matches!(v, Value::Int(n) if (0..=u8::MAX as i64).contains(n)),
        "u16" => matches!(v, Value::Int(n) if (0..=u16::MAX as i64).contains(n)),
        "u32" => matches!(v, Value::Int(n) if (0..=u32::MAX as i64).contains(n)),
        "u64" | "s64" => matches!(v, Value::Int(_)),
        "s8" => matches!(v, Value::Int(n) if (i8::MIN as i64..=i8::MAX as i64).contains(n)),
        "s16" => matches!(v, Value::Int(n) if (i16::MIN as i64..=i16::MAX as i64).contains(n)),
        "s32" => matches!(v, Value::Int(n) if (i32::MIN as i64..=i32::MAX as i64).contains(n)),
        _ => true,
    }
}

/// §4.2 patterns: literals match by equality, a bare name binds (unless it is
/// bound to a payload-less variant case, which matches by equality), call
/// shapes destructure variant cases, sequences destructure their counterparts.
fn match_pattern(
    arena: &Rc<Arena>,
    pat: NodeId,
    v: &Value,
    binds: &Env,
    scope: &Env,
) -> R<bool> {
    match arena.node(pat) {
        Node::Bool(b) => Ok(matches!(v, Value::Bool(x) if x == b)),
        Node::Int(n) => Ok(matches!(v, Value::Int(x) if x == n)),
        Node::Dec(f) => Ok(matches!(v, Value::Dec(x) if x == f)),
        Node::Char(c) => Ok(matches!(v, Value::Char(x) if x == c)),
        Node::Str(s) => Ok(matches!(v, Value::Str(x) if x == s)),
        Node::Flg(names) => Ok(matches!(v, Value::Flg(x) if x == names)),
        Node::Sym(name) => {
            if let Some(Value::Variant(case, None)) = scope.lookup(name) {
                if case == *name {
                    return Ok(matches!(v, Value::Variant(c, None) if *c == case));
                }
            }
            binds.define(name.clone(), v.clone());
            Ok(true)
        }
        Node::Qsym(..) => err("qualified names cannot appear in patterns"),
        Node::Call(..) => unreachable!("Node::Call is no longer produced by the reader"),
        // A tuple pattern is disambiguated by the scrutinee value: against a
        // variant it is a variant-case pattern `(case …rest)`; against a tuple
        // value it destructures element-wise.
        Node::Tup(pats) => match v {
            Value::Variant(cval, payload)
                if !pats.is_empty()
                    && matches!(arena.node(pats[0]), Node::Sym(c) if c == cval) =>
            {
                let rest = &pats[1..];
                match (rest.len(), payload) {
                    (0, None) => Ok(true),
                    (0, _) => Ok(false),
                    (1, Some(p)) => match_pattern(arena, rest[0], p, binds, scope),
                    (1, None) => Ok(false),
                    (_, Some(p)) => match &**p {
                        Value::Tup(vs) if vs.len() == rest.len() => {
                            match_all(arena, rest, vs, binds, scope)
                        }
                        _ => Ok(false),
                    },
                    (_, None) => Ok(false),
                }
            }
            Value::Tup(vs) if vs.len() == pats.len() => {
                match_all(arena, pats, vs, binds, scope)
            }
            _ => Ok(false),
        },
        Node::Lst(pats) => match v {
            Value::Lst(vs) if vs.len() == pats.len() => {
                match_all(arena, pats, vs, binds, scope)
            }
            _ => Ok(false),
        },
        Node::Rec(fields) => match v {
            Value::Rec(vfields) => {
                for (k, p) in fields {
                    match vfields.iter().find(|(vk, _)| vk == k) {
                        Some((_, vv)) => {
                            if !match_pattern(arena, *p, vv, binds, scope)? {
                                return Ok(false);
                            }
                        }
                        None => return Ok(false),
                    }
                }
                Ok(true)
            }
            _ => Ok(false),
        },
    }
}

fn match_all(
    arena: &Rc<Arena>,
    pats: &[NodeId],
    vs: &[Value],
    binds: &Env,
    scope: &Env,
) -> R<bool> {
    for (&p, v) in pats.iter().zip(vs) {
        if !match_pattern(arena, p, v, binds, scope)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn args1(args: &[NodeId], what: &str) -> R<[NodeId; 1]> {
    match args {
        [a] => Ok([*a]),
        _ => err(format!("{what} expects 1 argument")),
    }
}

fn args2(args: &[NodeId], what: &str) -> R<[NodeId; 2]> {
    match args {
        [a, b] => Ok([*a, *b]),
        _ => err(format!("{what} expects 2 arguments")),
    }
}

fn args3(args: &[NodeId], what: &str) -> R<[NodeId; 3]> {
    match args {
        [a, b, c] => Ok([*a, *b, *c]),
        _ => err(format!("{what} expects 3 arguments")),
    }
}
