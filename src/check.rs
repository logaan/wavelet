//! The static type checker (Phase A of the monomorphic type system).
//!
//! `dd-type-system.typ` defines two rules: every function's signature is a WIT
//! function type, and every expression has a WIT type. This module is the start
//! of the *total* checker that enforces them. It runs over the form arena
//! (`Node`/`NodeId`) BEFORE evaluation in [`crate::eval_snippet`], so an
//! ill-typed program is a compile error even when the bad code is never reached
//! at runtime.
//!
//! The checker is **gradual, bidirectional, and monomorphic**. It models only
//! as much of the language as it needs to reject genuine, provable type
//! conflicts; everything it does not model yields [`Type::Unknown`] (a gradual
//! top that unifies with anything and is never an error). This is what keeps the
//! existing example suite green: the checker must never preempt an existing
//! runtime error with a different message.
//!
//! Later phases (WIT synthesis from inference, overload resolution, derivers,
//! functors) build on the [`Type`] lattice and the per-form checking here.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::form::{Arena, Node, NodeId};

/// A WIT type, plus the gradual/inference extensions the checker needs.
///
/// `Unknown` is the gradual top — it unifies with anything and is never the
/// cause of an error. `IntLit`/`FloatLit` are unresolved numeric literals that
/// are compatible with a range of concrete numeric types (see [`Type::numeric`]
/// and [`unify`]); they default to `S64`/`F64` when nothing constrains them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Bool,
    U8,
    U16,
    U32,
    U64,
    S8,
    S16,
    S32,
    S64,
    F32,
    F64,
    Char,
    String,
    List(Box<Type>),
    /// A `DefType` record/variant, named nominally.
    Named(String),
    /// The unit type (`{}`), e.g. the result of a `Def`.
    Unit,
    /// An unconstrained integer literal: compatible with any int or float type.
    IntLit,
    /// An unconstrained float literal: compatible with `f32`/`f64` only.
    FloatLit,
    /// Gradual top: unifies with anything, never an error. The result of
    /// anything the checker does not (yet) model.
    Unknown,
}

impl Type {
    /// Whether this type is a concrete integer type.
    fn is_int(&self) -> bool {
        matches!(
            self,
            Type::U8
                | Type::U16
                | Type::U32
                | Type::U64
                | Type::S8
                | Type::S16
                | Type::S32
                | Type::S64
        )
    }

    /// Whether this type is a concrete float type.
    fn is_float(&self) -> bool {
        matches!(self, Type::F32 | Type::F64)
    }

    /// Whether this type is numeric in the operand sense: a concrete int/float,
    /// an unresolved numeric literal, or gradual `Unknown`.
    fn numeric(&self) -> bool {
        self.is_int()
            || self.is_float()
            || matches!(self, Type::IntLit | Type::FloatLit | Type::Unknown)
    }

    /// Parse a WIT type form (a `Sym` like `u8`/`s32`, or a constructor tuple
    /// like `list(s32)`). Returns `Unknown` for anything not modelled, so an
    /// unrecognized annotation never causes a false positive.
    fn from_form(arena: &Arena, id: NodeId) -> Type {
        match arena.node(id) {
            Node::Sym(s) => Type::from_name(s),
            Node::Tup(items) => {
                let Some((&head, args)) = items.split_first() else {
                    return Type::Unknown;
                };
                let Node::Sym(ctor) = arena.node(head) else {
                    return Type::Unknown;
                };
                match (ctor.as_str(), args) {
                    ("list", [elem]) => Type::List(Box::new(Type::from_form(arena, *elem))),
                    // option/result/tuple are not modelled in Phase A; treat as
                    // gradual so nothing downstream is falsely rejected.
                    _ => Type::Unknown,
                }
            }
            _ => Type::Unknown,
        }
    }

    /// Parse a primitive WIT type name. Unknown names (including user `DefType`
    /// names we cannot resolve here) become `Named`/`Unknown` so they stay
    /// gradual.
    fn from_name(s: &str) -> Type {
        match s {
            "bool" => Type::Bool,
            "u8" => Type::U8,
            "u16" => Type::U16,
            "u32" => Type::U32,
            "u64" => Type::U64,
            "s8" => Type::S8,
            "s16" => Type::S16,
            "s32" => Type::S32,
            "s64" => Type::S64,
            "f32" => Type::F32,
            "f64" => Type::F64,
            "char" => Type::Char,
            "string" => Type::String,
            // A bare identifier we don't recognize is a nominal name (a
            // `DefType` record/variant). It unifies only with itself.
            other => Type::Named(other.to_string()),
        }
    }
}

/// Unify two types, gradually. `Unknown` absorbs anything. Numeric literals
/// unify with compatible concrete numeric types (and default toward the
/// concrete one). Two known, incompatible concrete types fail.
fn unify(a: &Type, b: &Type) -> Option<Type> {
    use Type::*;
    match (a, b) {
        (Unknown, t) | (t, Unknown) => Some(t.clone()),
        (x, y) if x == y => Some(x.clone()),

        // An integer literal unifies with any concrete int or float, resolving
        // to that concrete type.
        (IntLit, t) | (t, IntLit) if t.is_int() || t.is_float() => Some(t.clone()),
        (IntLit, IntLit) => Some(IntLit),
        // An int literal and a float literal together are still a float literal.
        (IntLit, FloatLit) | (FloatLit, IntLit) => Some(FloatLit),

        // A float literal unifies only with a concrete float type.
        (FloatLit, t) | (t, FloatLit) if t.is_float() => Some(t.clone()),
        (FloatLit, FloatLit) => Some(FloatLit),

        // Lists unify element-wise.
        (List(x), List(y)) => Some(List(Box::new(unify(x, y)?))),

        _ => None,
    }
}

/// Whether a value of type `actual` is acceptable where `expected` is required.
/// Gradual: `Unknown` on either side always passes; numeric literals are
/// class-compatible; otherwise it is unifiability.
fn compatible(expected: &Type, actual: &Type) -> bool {
    unify(expected, actual).is_some()
}

/// Whether an overload candidate with parameters `params` is applicable to a
/// positional call whose static argument types are `arg_tys`: the arity must
/// match and each parameter type must be compatible with its argument. (The
/// single-bundled-payload form `f(x)` to a one-parameter `f` also matches.)
fn args_match(params: &[(String, Type)], arg_tys: &[Type]) -> bool {
    if params.len() != arg_tys.len() {
        return false;
    }
    params
        .iter()
        .zip(arg_tys)
        .all(|((_n, pt), at)| compatible(pt, at))
}

/// The static signature of a module-level `Def name Fn {params} body`.
struct Sig {
    /// Parameters in order: their name and declared type (`Unknown` if untyped).
    params: Vec<(String, Type)>,
    /// The Fn body, for return-type-directed overload resolution. `None` for a
    /// definition whose body we did not capture (it never happens for Fn defs).
    body: Option<NodeId>,
}

/// A lexical scope mapping bound names to their static types. It is a flat
/// stack so nested scopes can be unwound by truncation; inner bindings shadow
/// outer ones because lookup walks from the top.
type Scope = Vec<(String, Type)>;

struct Checker<'a> {
    arena: &'a Arena,
    /// Module-level `Def name Fn {…} …` signatures, by name. A name with more
    /// than one signature is an *overload set* (Phase C): calls to it resolve
    /// per call site by static argument and expected types.
    sigs: HashMap<String, Vec<Sig>>,
    /// Module-level `Def` names (functions and values both bind a name).
    defs: std::collections::HashSet<String>,
    /// For each *overloaded* call site (keyed by the call `Tup`'s `NodeId`), the
    /// index of the chosen candidate within its overload set. Filled in while
    /// checking; read back by [`resolve_overloads`] to rewrite the program.
    resolved: RefCell<HashMap<NodeId, usize>>,
}

/// Check a whole program (the top-level roots). Returns `Err(msg)` on the first
/// type error, where `msg` is already in the `eval error: …` surface form so it
/// can be returned directly as [`crate::EvalOutcome::error`].
pub fn check_program(arena: &Arena, roots: &[NodeId]) -> Result<(), String> {
    let checker = Checker::collect(arena, roots);
    checker.check_roots(roots)
}

impl<'a> Checker<'a> {
    /// First pass: collect every module-level Def name and Fn signature so
    /// forward and mutual references resolve. Same-named Fn defs accumulate into
    /// an overload set (a `Vec<Sig>`), in file order.
    fn collect(arena: &'a Arena, roots: &[NodeId]) -> Self {
        let mut sigs: HashMap<String, Vec<Sig>> = HashMap::new();
        let mut defs = std::collections::HashSet::new();
        for &root in roots {
            if let Some((name, expr)) = as_def(arena, root) {
                defs.insert(name.to_string());
                if let Some(params) = fn_params(arena, expr) {
                    let body = fn_body(arena, expr);
                    sigs.entry(name.to_string())
                        .or_default()
                        .push(Sig { params, body });
                }
            }
        }
        Checker { arena, sigs, defs, resolved: RefCell::new(HashMap::new()) }
    }

    /// Second pass: check every top-level form's body.
    fn check_roots(&self, roots: &[NodeId]) -> Result<(), String> {
        let arena = self.arena;
        for &root in roots {
            if let Some((_name, expr)) = as_def(arena, root) {
                // Check the bound expression. For an `Fn`, check its body with
                // the parameters in scope; otherwise check the value expression.
                if let Some(params) = fn_params(arena, expr) {
                    let mut scope: Scope = params.clone();
                    let body = fn_body(arena, expr).expect("fn with params has a body");
                    self.check(body, None, &mut scope)?;
                } else {
                    let mut scope: Scope = Vec::new();
                    self.check(expr, None, &mut scope)?;
                }
            } else {
                // A bare top-level expression (the playground evaluates these).
                let mut scope: Scope = Vec::new();
                self.check(root, None, &mut scope)?;
            }
        }
        Ok(())
    }

    /// The names that form an overload set: module-level Fn names with ≥2 defs.
    fn overload_names(&self) -> std::collections::HashSet<String> {
        self.sigs
            .iter()
            .filter(|(_n, v)| v.len() > 1)
            .map(|(n, _v)| n.clone())
            .collect()
    }
}

/// Type-check a program and resolve its overload sets, returning a possibly
/// rewritten `(Arena, roots)` the interpreter can evaluate with **no** overload
/// awareness of its own.
///
/// This is the run-path overload mechanism (Phase C, Steps 6–7). It runs after
/// reading and is the single place static argument-directed and return-type-
/// directed resolution happens:
///
/// 1. Build the checker (collecting overload sets) and check every body — an
///    ill-typed program is an `Err` exactly as [`check_program`] reports.
/// 2. While checking, each overloaded call site records which member it
///    resolves to (or the check fails with an ambiguity/no-match error).
/// 3. If the program has **no** overload set, return the input arena unchanged —
///    the pass is an exact identity, so non-overloaded programs are untouched.
/// 4. Otherwise rewrite into a fresh arena: give the k-th `Def name …` of an
///    overloaded `name` the unique internal symbol `name$k`, and re-point every
///    resolved call head to its chosen member. The result has no overloaded
///    names left, so the interpreter's ordinary by-name dispatch is correct.
///
/// Only overloaded names *in call position* are re-pointed; using an overloaded
/// name as a first-class value (passing the unapplied function) has no single
/// meaning under overloading and is out of scope here — it would survive
/// unrenamed and fail at runtime as an unbound name. No current program does
/// this; Phase D should revisit it if derived/functor ops are ever passed by
/// value.
pub fn resolve_overloads(
    arena: Arena,
    roots: &[NodeId],
) -> Result<(Arena, Vec<NodeId>), String> {
    let checker = Checker::collect(&arena, roots);
    checker.check_roots(roots)?;

    let overloads = checker.overload_names();
    if overloads.is_empty() {
        // Identity: nothing to rewrite, hand the program back as-is.
        return Ok((arena, roots.to_vec()));
    }

    let resolved = checker.resolved.into_inner();
    let mut rw = Rewriter {
        arena: &arena,
        overloads: &overloads,
        resolved: &resolved,
        out: Arena::new(),
        def_counts: HashMap::new(),
    };
    let new_roots: Vec<NodeId> = roots.iter().map(|&r| rw.rewrite_root(r)).collect();
    Ok((rw.out, new_roots))
}

/// The unique internal name for the k-th `Def name …` of an overloaded `name`.
fn mangled_def_name(name: &str, k: usize) -> String {
    format!("{name}${k}")
}

/// Rewrites a program so each overload-set member has a unique name and every
/// resolved call head points at its chosen member. Mirrors the copy/descend
/// style of [`crate::expand`].
struct Rewriter<'a> {
    arena: &'a Arena,
    overloads: &'a std::collections::HashSet<String>,
    resolved: &'a HashMap<NodeId, usize>,
    out: Arena,
    /// Running count of `Def`s seen per overloaded name, to assign `name$k`.
    def_counts: HashMap<String, usize>,
}

impl<'a> Rewriter<'a> {
    /// Rewrite a top-level form. A `Def name Fn …` whose `name` is overloaded is
    /// renamed to `name$k` (k counting in file order); everything else descends.
    fn rewrite_root(&mut self, id: NodeId) -> NodeId {
        if let Some((name, _expr)) = as_def(self.arena, id)
            && self.overloads.contains(name)
        {
            let name = name.to_string();
            let Node::Tup(items) = self.arena.node(id) else {
                unreachable!("as_def matched a Tup")
            };
            let items = items.clone();
            let k = self.def_counts.entry(name.clone()).or_insert(0);
            let unique = mangled_def_name(&name, *k);
            *k += 1;
            let span = self.arena.span(id);
            // items = [def-MACRO, name_sym, expr]; replace name_sym, rewrite expr.
            let head = self.rewrite(items[0]);
            let new_name = self.out.add(Node::Sym(unique), self.arena.span(items[1]));
            let expr = self.rewrite(items[2]);
            return self.out.add(Node::Tup(vec![head, new_name, expr]), span);
        }
        self.rewrite(id)
    }

    /// Copy `id` into the output arena, re-pointing a resolved overloaded call
    /// head to its chosen member.
    fn rewrite(&mut self, id: NodeId) -> NodeId {
        let span = self.arena.span(id);
        match self.arena.node(id).clone() {
            Node::Tup(items) => {
                // A call whose head is an overloaded name resolved at this site:
                // rewrite the head symbol to the chosen `name$k`.
                if let Some(&chosen) = self.resolved.get(&id)
                    && let Some(&head) = items.first()
                    && let Node::Sym(name) = self.arena.node(head)
                {
                    let unique = mangled_def_name(name, chosen);
                    let new_head = self.out.add(Node::Sym(unique), self.arena.span(head));
                    let mut kids = Vec::with_capacity(items.len());
                    kids.push(new_head);
                    for &x in &items[1..] {
                        kids.push(self.rewrite(x));
                    }
                    return self.out.add(Node::Tup(kids), span);
                }
                let kids: Vec<NodeId> = items.iter().map(|&x| self.rewrite(x)).collect();
                self.out.add(Node::Tup(kids), span)
            }
            Node::Lst(items) => {
                let kids: Vec<NodeId> = items.iter().map(|&x| self.rewrite(x)).collect();
                self.out.add(Node::Lst(kids), span)
            }
            Node::Rec(fields) => {
                let nf: Vec<(String, NodeId)> = fields
                    .iter()
                    .map(|(k, v)| (k.clone(), self.rewrite(*v)))
                    .collect();
                self.out.add(Node::Rec(nf), span)
            }
            leaf => self.out.add(leaf, span),
        }
    }
}

/// If `id` is `Def name expr`, return `(name, expr)`.
fn as_def(arena: &Arena, id: NodeId) -> Option<(&str, NodeId)> {
    let Node::Tup(items) = arena.node(id) else {
        return None;
    };
    let [head, name_id, expr] = items.as_slice() else {
        return None;
    };
    let Node::Sym(h) = arena.node(*head) else {
        return None;
    };
    if h != "def-MACRO" {
        return None;
    }
    let Node::Sym(name) = arena.node(*name_id) else {
        return None;
    };
    Some((name, *expr))
}

/// If `id` is `Fn {params} body`, return the parsed parameter list.
fn fn_params(arena: &Arena, id: NodeId) -> Option<Vec<(String, Type)>> {
    let (params_id, _body) = as_fn(arena, id)?;
    Some(parse_params(arena, params_id))
}

/// If `id` is `Fn {params} body`, return its body form.
fn fn_body(arena: &Arena, id: NodeId) -> Option<NodeId> {
    as_fn(arena, id).map(|(_p, body)| body)
}

/// If `id` is `Fn {params} body`, return `(params_form, body_form)`.
fn as_fn(arena: &Arena, id: NodeId) -> Option<(NodeId, NodeId)> {
    let Node::Tup(items) = arena.node(id) else {
        return None;
    };
    let [head, params, body] = items.as_slice() else {
        return None;
    };
    let Node::Sym(h) = arena.node(*head) else {
        return None;
    };
    if h != "fn-MACRO" {
        return None;
    }
    Some((*params, *body))
}

/// Parse a `Fn` parameter form (`{a: t b …}` record, or `{}` flags). Untyped
/// parameters get `Unknown`.
fn parse_params(arena: &Arena, id: NodeId) -> Vec<(String, Type)> {
    match arena.node(id) {
        Node::Rec(fields) => fields
            .iter()
            .map(|(k, v)| (k.clone(), Type::from_form(arena, *v)))
            .collect(),
        Node::Flg(names) => names.iter().map(|n| (n.clone(), Type::Unknown)).collect(),
        _ => Vec::new(),
    }
}

impl<'a> Checker<'a> {
    /// Infer (and optionally check against `expected`) the type of expression
    /// `id` in `scope`. On a provable conflict, returns `Err(eval-error-msg)`.
    fn check(
        &self,
        id: NodeId,
        expected: Option<&Type>,
        scope: &mut Scope,
    ) -> Result<Type, String> {
        let ty = self.infer(id, expected, scope)?;
        if let Some(exp) = expected
            && !compatible(exp, &ty)
        {
            return Err(self.type_error(id, exp, &ty));
        }
        Ok(ty)
    }

    fn type_error(&self, _id: NodeId, expected: &Type, actual: &Type) -> String {
        format!("eval error: type mismatch: expected {expected:?}, got {actual:?}")
    }

    fn infer(
        &self,
        id: NodeId,
        expected: Option<&Type>,
        scope: &mut Scope,
    ) -> Result<Type, String> {
        match self.arena.node(id) {
            Node::Bool(_) => Ok(Type::Bool),
            Node::Int(_) => Ok(Type::IntLit),
            Node::Dec(_) => Ok(Type::FloatLit),
            Node::Char(_) => Ok(Type::Char),
            Node::Str(_) => Ok(Type::String),
            Node::Sym(name) => self.infer_name(name, scope),
            // A qualified name (`alias/fn`) reaches into an imported component we
            // do not model here.
            Node::Qsym(..) => Ok(Type::Unknown),
            Node::Lst(items) => self.infer_list(items, expected, scope),
            // A record literal in value position: we don't model record types
            // structurally in Phase A. Check its fields, yield Unknown.
            Node::Rec(fields) => {
                for (_k, v) in fields {
                    self.check(*v, None, scope)?;
                }
                Ok(Type::Unknown)
            }
            Node::Flg(_) => Ok(Type::Unknown),
            Node::Tup(items) => self.infer_tup(id, items, expected, scope),
        }
    }

    /// Resolve a bare name to its type. A name is bound if it is a parameter, a
    /// `Let`/`Match` binding (in `scope`), a module-level `Def`, or a builtin.
    /// Anything else is an unbound-name compile error.
    fn infer_name(&self, name: &str, scope: &Scope) -> Result<Type, String> {
        if let Some((_, t)) = scope.iter().rev().find(|(n, _)| n == name) {
            return Ok(t.clone());
        }
        if self.defs.contains(name) {
            // A reference to a module-level def. As a value its type is the
            // function/value itself, which we don't model — gradual.
            return Ok(Type::Unknown);
        }
        if is_builtin(name) {
            return Ok(Type::Unknown);
        }
        Err(format!("eval error: unbound name `{name}`"))
    }

    fn infer_list(
        &self,
        items: &[NodeId],
        expected: Option<&Type>,
        scope: &mut Scope,
    ) -> Result<Type, String> {
        let elem_expected = match expected {
            Some(Type::List(e)) => Some((**e).clone()),
            _ => None,
        };
        let mut elem = Type::Unknown;
        let mut seeded = false;
        for &it in items {
            let t = self.check(it, elem_expected.as_ref(), scope)?;
            if !seeded {
                elem = t;
                seeded = true;
            } else if let Some(u) = unify(&elem, &t) {
                elem = u;
            } else {
                // Heterogeneous list elements: not modelled as an error in
                // Phase A (lists of mixed shape appear in quoted data); stay
                // gradual.
                elem = Type::Unknown;
            }
        }
        Ok(Type::List(Box::new(elem)))
    }

    /// A `Tup` in evaluation position is either a core special form (head is a
    /// `*-MACRO` symbol) or a call `head(args…)`.
    fn infer_tup(
        &self,
        id: NodeId,
        items: &[NodeId],
        expected: Option<&Type>,
        scope: &mut Scope,
    ) -> Result<Type, String> {
        let Some((&head, args)) = items.split_first() else {
            return Ok(Type::Unit);
        };
        if let Node::Sym(h) = self.arena.node(head) {
            if h.ends_with("-MACRO") {
                return self.infer_special(h, args, expected, scope);
            }
            // A call to a known builtin or module-level def.
            return self.infer_call(id, h, args, expected, scope);
        }
        // Head is not a plain symbol (e.g. a Qsym, or a computed head): check
        // the arguments and yield Unknown.
        for &a in args {
            self.check(a, None, scope)?;
        }
        Ok(Type::Unknown)
    }

    fn infer_special(
        &self,
        head: &str,
        args: &[NodeId],
        expected: Option<&Type>,
        scope: &mut Scope,
    ) -> Result<Type, String> {
        match head {
            "fn-MACRO" => {
                // A nested anonymous Fn: check its body with parameters in
                // scope, but its value type (a callback) is gradual.
                if let [params_id, body] = args {
                    let params = parse_params(self.arena, *params_id);
                    let mark = scope.len();
                    for (n, t) in &params {
                        scope.push((n.clone(), t.clone()));
                    }
                    self.check(*body, None, scope)?;
                    scope.truncate(mark);
                }
                Ok(Type::Unknown)
            }
            "if-MACRO" => {
                let [c, t, e] = expect3(args)?;
                // Do NOT statically check the condition's bool-ness (a runtime
                // example relies on a non-bool condition failing at runtime).
                self.check(c, None, scope)?;
                let tt = self.check(t, expected, scope)?;
                let et = self.check(e, expected, scope)?;
                match unify(&tt, &et) {
                    Some(u) => Ok(u),
                    None => Err("eval error: If branches have incompatible types".to_string()),
                }
            }
            "let-MACRO" => {
                let [bindings, body] = expect2(args)?;
                let mark = scope.len();
                if let Node::Rec(fields) = self.arena.node(bindings) {
                    for (k, v) in fields {
                        let t = self.check(*v, None, scope)?;
                        scope.push((k.clone(), t));
                    }
                }
                let r = self.check(body, expected, scope);
                scope.truncate(mark);
                r
            }
            "do-MACRO" => {
                let [list] = args else {
                    return Ok(Type::Unknown);
                };
                let Node::Lst(stmts) = self.arena.node(*list) else {
                    return Ok(Type::Unknown);
                };
                let mut last = Type::Unit;
                for (i, &s) in stmts.iter().enumerate() {
                    let exp = if i + 1 == stmts.len() { expected } else { None };
                    last = self.check(s, exp, scope)?;
                }
                Ok(last)
            }
            "match-MACRO" => {
                let [scrut, clauses] = expect2(args)?;
                self.check(scrut, None, scope)?;
                let Node::Lst(items) = self.arena.node(clauses) else {
                    return Ok(Type::Unknown);
                };
                let mut result: Option<Type> = None;
                for &clause in items {
                    let Node::Tup(pair) = self.arena.node(clause) else {
                        continue;
                    };
                    if pair.len() != 2 {
                        continue;
                    }
                    // Bind every variable mentioned in the pattern as Unknown so
                    // the clause body doesn't see false "unbound" errors.
                    let mark = scope.len();
                    self.bind_pattern(pair[0], scope);
                    let rt = self.check(pair[1], expected, scope)?;
                    scope.truncate(mark);
                    result = Some(match result {
                        None => rt,
                        Some(prev) => unify(&prev, &rt).ok_or_else(|| {
                            "eval error: Match clauses have incompatible result types".to_string()
                        })?,
                    });
                }
                Ok(result.unwrap_or(Type::Unknown))
            }
            "the-MACRO" => {
                let [ty_form, expr] = expect2(args)?;
                let ty = Type::from_form(self.arena, ty_form);
                self.check_the(ty_form, &ty, expr, scope)
            }
            // Quote/Quasi produce data (the `tree` arena type); their contents
            // are not value-checked. A `DefMacro` defines a compile-time macro
            // whose body is a template (data), not a value expression, so we do
            // not check it. Top-level file forms (`Package`/`Import`/`Export`/
            // `DefType`) carry annotations, not value expressions. All of these
            // are opaque to the value checker.
            "quote-MACRO" | "quasi-MACRO" | "defmacro-MACRO" | "package-MACRO"
            | "import-MACRO" | "export-MACRO" | "deftype-MACRO" => Ok(Type::Unknown),
            // Any other `-MACRO` head is a user (or foreign) macro call that
            // `eval_snippet` expands at runtime. We cannot statically see
            // through it, so check nothing and stay gradual. Crucially we do NOT
            // value-check its arguments: a macro receives its arguments as
            // *forms* (data), so a bare name there is not an unbound use.
            _ => {
                let _ = args;
                Ok(Type::Unknown)
            }
        }
    }

    /// Check a `The ty expr` ascription. Numeric literals are range-checked at
    /// compile time, producing the SAME message the interpreter's runtime check
    /// produces so a locked example keeps matching.
    fn check_the(
        &self,
        ty_form: NodeId,
        ty: &Type,
        expr: NodeId,
        scope: &mut Scope,
    ) -> Result<Type, String> {
        let ty_text = type_name(self.arena, ty_form);
        match self.arena.node(expr) {
            Node::Int(n) => {
                if !int_in_range(*n, ty) {
                    return Err(format!(
                        "eval error: The: {n} does not conform to type `{ty_text}`"
                    ));
                }
                Ok(ty.clone())
            }
            Node::Dec(_) => {
                if ty.is_int() {
                    return Err(format!(
                        "eval error: The: {} does not conform to type `{ty_text}`",
                        print_dec(self.arena, expr)
                    ));
                }
                Ok(ty.clone())
            }
            _ => {
                let actual = self.check(expr, Some(ty), scope)?;
                let _ = actual;
                Ok(ty.clone())
            }
        }
    }

    /// Check a call `name(args…)` to a builtin or a module-level def.
    fn infer_call(
        &self,
        id: NodeId,
        name: &str,
        args: &[NodeId],
        expected: Option<&Type>,
        scope: &mut Scope,
    ) -> Result<Type, String> {
        if let Some(sigs) = self.sigs.get(name) {
            // An overload set (≥2 same-named Fn defs): resolve per call site by
            // static argument types, then by the expected (return) type.
            if sigs.len() > 1 {
                return self.resolve_overload(id, name, sigs, args, expected, scope);
            }
            // A single module-level def with a known signature: check arity and
            // argument types against the parameters (Phase A behaviour).
            return self.check_def_call(name, &sigs[0], args, scope);
        }
        // A builtin we model, or one we don't (Unknown).
        self.check_builtin_call(name, args, scope)
    }

    /// Resolve an overloaded call `name(args…)` to exactly one member of its
    /// overload set, recording the chosen index for [`resolve_overloads`].
    ///
    /// Step 1: keep every candidate whose arity matches and whose parameter
    /// types are each compatible with the corresponding static argument type.
    /// Step 2: if more than one survives, filter by the expected result type
    /// from context (an enclosing `The`, or any propagated expected type). A
    /// unique survivor resolves; zero or several is an ambiguity/no-match error.
    fn resolve_overload(
        &self,
        id: NodeId,
        name: &str,
        sigs: &[Sig],
        args: &[NodeId],
        expected: Option<&Type>,
        scope: &mut Scope,
    ) -> Result<Type, String> {
        // Infer the argument types once (also checks their subexpressions).
        let arg_tys: Vec<Type> = args
            .iter()
            .map(|&a| self.check(a, None, scope))
            .collect::<Result<_, _>>()?;

        // Step 1 — argument-directed filtering.
        let mut candidates: Vec<usize> = (0..sigs.len())
            .filter(|&i| args_match(&sigs[i].params, &arg_tys))
            .collect();

        // Step 2 — return-type-directed filtering, only when arguments leave
        // more than one candidate and the context supplies an expected type.
        // Keep the narrowed set whenever it is non-empty; if nothing matches the
        // expected type, fall back to the argument-filtered set so the error
        // below reports the (still-ambiguous) call rather than a spurious
        // no-match.
        if candidates.len() > 1
            && let Some(exp) = expected
        {
            let by_result: Vec<usize> = candidates
                .iter()
                .copied()
                .filter(|&i| compatible(exp, &self.infer_sig_result(&sigs[i])))
                .collect();
            if !by_result.is_empty() {
                candidates = by_result;
            }
        }

        match candidates.as_slice() {
            [chosen] => {
                self.resolved.borrow_mut().insert(id, *chosen);
                Ok(self.infer_sig_result(&sigs[*chosen]))
            }
            [] => Err(format!(
                "eval error: no overload of `{name}` matches the call"
            )),
            _ => Err(format!(
                "eval error: ambiguous call to overloaded `{name}`; \
                 qualify it to choose an overload"
            )),
        }
    }

    /// Infer the result type of an overload candidate by checking its Fn body
    /// with its parameters in scope. Used for return-type-directed resolution.
    fn infer_sig_result(&self, sig: &Sig) -> Type {
        let Some(body) = sig.body else {
            return Type::Unknown;
        };
        let mut scope: Scope = sig.params.clone();
        // Inference errors inside a candidate body don't disqualify it here (the
        // body is checked properly when its own Def is checked); treat them as
        // an unconstrained result so resolution stays gradual.
        self.infer(body, None, &mut scope).unwrap_or(Type::Unknown)
    }

    fn check_def_call(
        &self,
        name: &str,
        sig: &Sig,
        args: &[NodeId],
        scope: &mut Scope,
    ) -> Result<Type, String> {
        // First, infer the argument types (also checks their subexpressions).
        let arg_tys: Vec<Type> = args
            .iter()
            .map(|&a| self.check(a, None, scope))
            .collect::<Result<_, _>>()?;

        let nparams = sig.params.len();

        // The single-record-arg-by-name form: `f({a: … b: …})` binds by field
        // name. We do not check those field types in Phase A — accept.
        if args.len() == 1 {
            if let Node::Rec(_) = self.arena.node(args[0]) {
                return Ok(Type::Unknown);
            }
            // A single argument to a single parameter taking the whole payload.
            if nparams == 1 {
                if !compatible(&sig.params[0].1, &arg_tys[0]) {
                    return Err(format!(
                        "eval error: argument to `{name}` has the wrong type"
                    ));
                }
                return Ok(Type::Unknown);
            }
        }

        // Positional call: arity must match.
        if args.len() != nparams {
            return Err(format!(
                "eval error: `{name}` expects {nparams} arguments, got {}",
                args.len()
            ));
        }
        for ((_pn, pt), at) in sig.params.iter().zip(&arg_tys) {
            if !compatible(pt, at) {
                return Err(format!(
                    "eval error: argument to `{name}` has the wrong type"
                ));
            }
        }
        Ok(Type::Unknown)
    }

    /// Check a builtin call, modelling only the operand/result behaviour needed
    /// to pass the tests and to type the example set without false positives.
    fn check_builtin_call(
        &self,
        name: &str,
        args: &[NodeId],
        scope: &mut Scope,
    ) -> Result<Type, String> {
        let arg_tys: Vec<Type> = args
            .iter()
            .map(|&a| self.check(a, None, scope))
            .collect::<Result<_, _>>()?;

        match name {
            // Arithmetic: every operand must be numeric; result is the unified
            // numeric type (Unknown if any operand is Unknown).
            "add" | "sub" | "mul" | "div" | "rem" | "neg" | "min" | "max" | "abs" => {
                let mut result = Type::Unknown;
                let mut any_unknown = false;
                let mut seeded = false;
                for t in &arg_tys {
                    if !t.numeric() {
                        return Err(format!(
                            "eval error: `{name}` requires numeric operands"
                        ));
                    }
                    if matches!(t, Type::Unknown) {
                        any_unknown = true;
                    }
                    if !seeded {
                        result = t.clone();
                        seeded = true;
                    } else if let Some(u) = unify(&result, t) {
                        result = u;
                    } else {
                        result = Type::Unknown;
                    }
                }
                if any_unknown {
                    Ok(Type::Unknown)
                } else {
                    Ok(result)
                }
            }
            // str-cat: every arg must be string/char/unknown; result string.
            "str-cat" => {
                for t in &arg_tys {
                    if !matches!(t, Type::String | Type::Char | Type::Unknown) {
                        return Err(format!(
                            "eval error: `{name}` requires string operands"
                        ));
                    }
                }
                Ok(Type::String)
            }
            "upper" | "lower" => {
                for t in &arg_tys {
                    if !matches!(t, Type::String | Type::Unknown) {
                        return Err(format!(
                            "eval error: `{name}` requires a string operand"
                        ));
                    }
                }
                Ok(Type::String)
            }
            // Comparisons and `not`: do NOT constrain operands; result bool.
            "eq" | "lt" | "le" | "gt" | "ge" | "not" => Ok(Type::Bool),
            "len" => Ok(Type::S64),
            // Everything else: result Unknown, args unconstrained (already
            // checked their subexpressions above). Later phases extend this.
            _ => Ok(Type::Unknown),
        }
    }

    /// Bind every variable name appearing in a Match pattern as `Unknown`.
    fn bind_pattern(&self, pat: NodeId, scope: &mut Scope) {
        match self.arena.node(pat) {
            Node::Sym(name) => {
                // A bare symbol pattern binds the name (it may also be a nullary
                // variant case, but binding it as Unknown is harmless).
                scope.push((name.clone(), Type::Unknown));
            }
            Node::Tup(items) => {
                // `(case rest…)` or element-wise tuple: skip the head if it
                // looks like a constructor symbol, bind the rest.
                for &it in items {
                    self.bind_pattern(it, scope);
                }
            }
            Node::Lst(items) => {
                for &it in items {
                    self.bind_pattern(it, scope);
                }
            }
            Node::Rec(fields) => {
                for (_k, v) in fields {
                    self.bind_pattern(*v, scope);
                }
            }
            // Literals in patterns bind nothing.
            _ => {}
        }
    }
}

/// Whether `n` fits the integer type `ty`. Mirrors `interp::check_type`'s range
/// logic so the compile-time `The` check matches the runtime one.
fn int_in_range(n: i64, ty: &Type) -> bool {
    match ty {
        Type::U8 => (0..=u8::MAX as i64).contains(&n),
        Type::U16 => (0..=u16::MAX as i64).contains(&n),
        Type::U32 => (0..=u32::MAX as i64).contains(&n),
        Type::U64 => n >= 0,
        Type::S8 => (i8::MIN as i64..=i8::MAX as i64).contains(&n),
        Type::S16 => (i16::MIN as i64..=i16::MAX as i64).contains(&n),
        Type::S32 => (i32::MIN as i64..=i32::MAX as i64).contains(&n),
        Type::S64 => true,
        // A float type accepts any integer literal (int promotes to float).
        Type::F32 | Type::F64 => true,
        // Any non-numeric ascription target leaves the literal unconstrained
        // here; gradual elsewhere.
        _ => true,
    }
}

/// The printed name of a type form, for error messages (matches the runtime
/// `The` message, which uses the raw annotation text like `s8`).
fn type_name(arena: &Arena, id: NodeId) -> String {
    match arena.node(id) {
        Node::Sym(s) => s.clone(),
        _ => crate::printer::print(arena, id),
    }
}

/// Print a `Dec` literal as the interpreter would.
fn print_dec(arena: &Arena, id: NodeId) -> String {
    if let Node::Dec(f) = arena.node(id) {
        crate::value::print_value(&crate::value::Value::Dec(*f))
    } else {
        crate::printer::print(arena, id)
    }
}

/// Is `name` a builtin? The set is `builtins::NAMES` plus `none` and `pi`.
fn is_builtin(name: &str) -> bool {
    name == "none" || name == "pi" || crate::builtins::NAMES.contains(&name)
}

// --- small arity helpers (the checker only ever needs 2..=3) -----------------

fn expect2(args: &[NodeId]) -> Result<[NodeId; 2], String> {
    match args {
        [a, b] => Ok([*a, *b]),
        _ => Err("eval error: malformed form".to_string()),
    }
}

fn expect3(args: &[NodeId]) -> Result<[NodeId; 3], String> {
    match args {
        [a, b, c] => Ok([*a, *b, *c]),
        _ => Err("eval error: malformed form".to_string()),
    }
}
