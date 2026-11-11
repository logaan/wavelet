use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::Rc;

use crate::builtins;
use crate::form::{Arena, Node, NodeId};
use crate::value::{
    Closure, Env, Param, ResourceCtor, ResourceInstance, ResourceMethod, Value, form_to_value,
    unit, value_to_form,
};

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

    Jump(Rc<Arena>, NodeId, Env),
}

pub struct Interp {
    pub gensym: Cell<u64>,
    resources: RefCell<Vec<(Value, Rc<Closure>)>>,
}

impl Drop for Interp {
    fn drop(&mut self) {
        let _ = self.teardown();
    }
}

impl Default for Interp {
    fn default() -> Self {
        Self::new()
    }
}

impl Interp {
    pub fn new() -> Self {
        Self {
            gensym: Cell::new(0),
            resources: RefCell::new(Vec::new()),
        }
    }

    pub fn teardown(&self) -> R<()> {
        let mut first_error = None;
        loop {
            let next = self.resources.borrow_mut().pop();
            let Some((rep, destructor)) = next else { break };
            if let Err(error) = self.apply(&Value::Closure(destructor), rep) {
                first_error.get_or_insert(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
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

    fn bundle_args(&self, arena: &Rc<Arena>, args: &[NodeId], env: &Env) -> R<Value> {
        match args {
            [] => Ok(Value::Tup(vec![])),
            [one] => self.eval(arena, *one, env),
            many => Ok(Value::Tup(self.eval_each(arena, many, env)?)),
        }
    }

    fn apply_step(&self, f: &Value, arg: Value, env: Option<&Env>) -> R<Step> {
        match f {
            Value::Closure(c) => {
                let env = bind_params(c, arg)?;
                Ok(Step::Jump(c.arena.clone(), c.body, env))
            }
            Value::Builtin(name) => Ok(Step::Done(builtins::call(self, name, arg, env)?)),

            Value::CaseCtor(case) => match arg {
                Value::Tup(items) if items.is_empty() => err(format!(
                    "variant case `{case}` takes a payload, got no arguments"
                )),
                payload => Ok(Step::Done(Value::Variant(
                    case.clone(),
                    Some(Rc::new(payload)),
                ))),
            },

            Value::ResourceCtor(rc) => {
                let rep = self.apply(&Value::Closure(rc.ctor.clone()), arg)?;
                if let Some(destructor) = &rc.destructor {
                    self.resources
                        .borrow_mut()
                        .push((rep.clone(), destructor.clone()));
                }
                Ok(Step::Done(Value::Resource(std::rc::Rc::new(
                    ResourceInstance {
                        name: rc.name.clone(),
                        rep,
                        consumed: std::cell::Cell::new(false),
                        identity: Rc::new(()),
                        borrowed: false,
                    },
                ))))
            }

            Value::ResourceMethod(rm) => {
                let arg = unwrap_receiver(&rm.name, arg)?;
                self.apply_step(&Value::Closure(rm.method.clone()), arg, env)
            }
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
            "defmacro-MACRO" => {
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
                let [_ty, expr] = args2(args, "The")?;
                Step::Jump(arena.clone(), expr, env.clone())
            }

            "deftype-MACRO" => {
                let [name_id, decl] = args2(args, "DefType")?;
                let Node::Sym(_) = arena.node(name_id) else {
                    return err("DefType expects a name");
                };
                if let Node::Lst(cases) = arena.node(decl) {
                    for &c in cases {
                        match arena.node(c) {
                            Node::Sym(case) => {
                                env.define(case.clone(), Value::Variant(case.clone(), None));
                            }
                            Node::Tup(case_items) => {
                                let Some(&h) = case_items.first() else {
                                    return err("bad variant case in DefType");
                                };
                                let Node::Sym(case) = arena.node(h) else {
                                    return err("bad variant case in DefType");
                                };
                                if case_items.len() > 2 {
                                    return err(
                                        "a variant case takes at most one payload type; wrap several types in tuple(...)",
                                    );
                                }
                                env.define(case.clone(), Value::CaseCtor(case.clone()));
                            }
                            _ => return err("bad variant case in DefType"),
                        }
                    }
                }
                Step::Done(unit())
            }

            "defresource-MACRO" => {
                let [name_id, members_id] = args2(args, "DefResource")?;
                let Node::Sym(rname) = arena.node(name_id) else {
                    return err("DefResource expects a name");
                };
                let Node::Rec(members) = arena.node(members_id) else {
                    return err("DefResource expects member braces `{ … }`");
                };
                let rname = rname.clone();
                let destructor =
                    if let Some((_, node)) = members.iter().find(|(key, _)| key == "Drop") {
                        let Value::Closure(closure) = self.eval(arena, *node, env)? else {
                            return err("DefResource `Drop` must be an `Fn`");
                        };
                        Some(closure)
                    } else {
                        None
                    };
                for (key, val) in members {
                    match key.as_str() {
                        "New" => {
                            let Value::Closure(c) = self.eval(arena, *val, env)? else {
                                return err("DefResource `New` must be an `Fn`");
                            };
                            env.define(
                                rname.clone(),
                                Value::ResourceCtor(std::rc::Rc::new(ResourceCtor {
                                    name: rname.clone(),
                                    ctor: c,
                                    destructor: destructor.clone(),
                                })),
                            );
                        }

                        "Drop" => {}
                        other => {
                            let (is_static, fn_id) = match arena.node(*val) {
                                Node::Tup(items)
                                    if items.len() == 2
                                        && matches!(
                                            arena.node(items[0]),
                                            Node::Sym(s) if s == "static-MACRO"
                                        ) =>
                                {
                                    (true, items[1])
                                }
                                _ => (false, *val),
                            };
                            let Value::Closure(c) = self.eval(arena, fn_id, env)? else {
                                return err(format!("resource member `{other}` must be an `Fn`"));
                            };
                            let value = if is_static {
                                Value::Closure(c)
                            } else {
                                Value::ResourceMethod(std::rc::Rc::new(ResourceMethod {
                                    name: rname.clone(),
                                    method: c,
                                }))
                            };
                            env.define(format!("{rname}-{other}"), value);
                        }
                    }
                }
                Step::Done(unit())
            }
            "package-MACRO" | "import-MACRO" | "instantiate-MACRO" | "export-MACRO" => {
                return err(format!(
                    "`{}` is only allowed at the top level of a file",
                    name.trim_end_matches("-MACRO")
                ));
            }
            _ => return Ok(None),
        };
        Ok(Some(step))
    }

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

    fn quasi(&self, arena: &Rc<Arena>, id: NodeId, env: &Env, depth: u32) -> R<Value> {
        match arena.node(id) {
            Node::Tup(items) => {
                if items.len() == 2
                    && let Node::Sym(name) = arena.node(items[0])
                {
                    let arg = items[1];
                    match name.as_str() {
                        "unquote-MACRO" if depth == 1 => return self.eval(arena, arg, env),
                        "splice-MACRO" if depth == 1 => {
                            return err("Splice must appear inside a sequence");
                        }
                        "unquote-MACRO" | "splice-MACRO" if depth > 1 => {
                            let inner = self.quasi(arena, arg, env, depth - 1)?;
                            return Ok(Value::Tup(vec![Value::Variant(name.clone(), None), inner]));
                        }
                        "quasi-MACRO" => {
                            let inner = self.quasi(arena, arg, env, depth + 1)?;
                            return Ok(Value::Tup(vec![Value::Variant(name.clone(), None), inner]));
                        }
                        _ => {}
                    }
                }
                Ok(Value::Tup(self.quasi_seq(arena, items, env, depth)?))
            }
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
            if depth == 1
                && let Node::Tup(tup) = arena.node(item)
                && tup.len() == 2
                && let Node::Sym(s) = arena.node(tup[0])
                && s == "splice-MACRO"
            {
                match self.eval(arena, tup[1], env)? {
                    Value::Lst(vs) => out.extend(vs),
                    v => {
                        return err(format!(
                            "Splice expects a list, got {}",
                            crate::value::print_value(&v)
                        ));
                    }
                }
                continue;
            }
            out.push(self.quasi(arena, item, env, depth)?);
        }
        Ok(out)
    }
}

fn unwrap_receiver(res_name: &str, arg: Value) -> R<Value> {
    fn unwrap_one(res_name: &str, v: Value) -> R<Value> {
        match v {
            Value::Resource(inst) if inst.name == res_name => {
                if inst.consumed.get() {
                    return err(format!(
                        "use of a consumed `{res_name}` handle (its ownership was transferred)"
                    ));
                }
                Ok(inst.rep.clone())
            }

            other => Ok(other),
        }
    }
    match arg {
        Value::Tup(mut items) if !items.is_empty() => {
            let first = std::mem::replace(&mut items[0], Value::Tup(vec![]));
            items[0] = unwrap_one(res_name, first)?;
            Ok(Value::Tup(items))
        }
        single => unwrap_one(res_name, single),
    }
}

fn bind_params(c: &Closure, arg: Value) -> R<Env> {
    let env = c.env.child();
    let n = c.params.len();
    match (n, &arg) {
        (0, Value::Tup(v)) if v.is_empty() => return Ok(env),
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
        Value::Tup(vs) if vs.len() == n => {
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
    let value = if let (Some((name, borrow)), Value::Resource(instance)) = (&p.resource_type, &v) {
        if *name != instance.name {
            return err(format!(
                "expected resource `{name}`, got `{}`",
                instance.name
            ));
        }
        if instance.consumed.get() {
            return err(format!(
                "use of a consumed `{name}` handle (its ownership was transferred)"
            ));
        }
        if !borrow && instance.borrowed {
            return err(format!(
                "cannot transfer ownership of a borrowed `{name}` handle"
            ));
        }
        if !borrow {
            instance.consumed.set(true);
        }
        Value::Resource(Rc::new(ResourceInstance {
            name: instance.name.clone(),
            rep: instance.rep.clone(),
            consumed: Cell::new(false),
            identity: instance.identity.clone(),
            borrowed: *borrow,
        }))
    } else {
        v
    };
    env.define(p.name.clone(), value);
    Ok(())
}

fn parse_params(arena: &Arena, id: NodeId) -> R<Vec<Param>> {
    match arena.node(id) {
        Node::Flg(names) => Ok(names
            .iter()
            .map(|n| Param {
                name: n.clone(),
                resource_type: None,
            })
            .collect()),
        Node::Rec(fields) => Ok(fields
            .iter()
            .map(|(name, ty)| {
                let resource_type = match arena.node(*ty) {
                    Node::Sym(name) => Some((name.clone(), false)),
                    Node::Tup(items) if items.len() == 2 => {
                        match (arena.node(items[0]), arena.node(items[1])) {
                            (Node::Sym(mode), Node::Sym(name))
                                if mode == "borrow" || mode == "own" =>
                            {
                                Some((name.clone(), mode == "borrow"))
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };
                Param {
                    name: name.clone(),
                    resource_type,
                }
            })
            .collect()),
        _ => err("Fn expects parameter braces"),
    }
}

fn match_pattern(arena: &Rc<Arena>, pat: NodeId, v: &Value, binds: &Env, scope: &Env) -> R<bool> {
    match arena.node(pat) {
        Node::Bool(b) => Ok(matches!(v, Value::Bool(x) if x == b)),
        Node::Int(n) => Ok(matches!(v, Value::Int(x) if x == n)),

        Node::Dec(_) => err("a float literal cannot be a Match pattern"),
        Node::Char(c) => Ok(matches!(v, Value::Char(x) if x == c)),
        Node::Str(s) => Ok(matches!(v, Value::Str(x) if x == s)),
        Node::Flg(names) => Ok(matches!(v, Value::Flg(x) if x == names)),
        Node::Sym(name) => {
            if let Some(Value::Variant(case, None)) = scope.lookup(name)
                && case == *name
            {
                return Ok(matches!(v, Value::Variant(c, None) if *c == case));
            }
            binds.define(name.clone(), v.clone());
            Ok(true)
        }

        Node::Tup(pats) => match v {
            Value::Variant(cval, payload)
                if !pats.is_empty() && matches!(arena.node(pats[0]), Node::Sym(c) if c == cval) =>
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
            Value::Tup(vs) if vs.len() == pats.len() => match_all(arena, pats, vs, binds, scope),
            _ => Ok(false),
        },
        Node::Lst(pats) => match v {
            Value::Lst(vs) if vs.len() == pats.len() => match_all(arena, pats, vs, binds, scope),
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
