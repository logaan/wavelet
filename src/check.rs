//! The static type checker (Phases A and C of the monomorphic type system).
//!
//! `dd-type-system.typ` defines two rules: every function's signature is a WIT
//! function type, and every expression has a WIT type. This module enforces
//! them as far as the phases below reach. It runs over the form arena
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
//! Where the phases stand:
//! - **Phase A** (per-form gradual checking) is this module's core:
//!   [`check_program`] and the `Checker::check`/`infer` walk.
//! - **Phase B** (WIT synthesis from inference) is implemented in
//!   [`crate::wit`] (`infer_sig`, `infer_param_from_use`), which also
//!   implements compile-time functors (Steps 10–11).
//! - **Phase C** (overload resolution, Steps 6–8) is implemented here too:
//!   same-named `Fn` defs form overload sets, call sites resolve by static
//!   argument types then return-type-directed inference, and
//!   [`resolve_overloads`] rewrites the program so the interpreter needs no
//!   overload awareness. Boundary name-mangling for exported overloads
//!   (Step 8) lives in [`crate::wit`].
//! - **Phase D** (derivers; overloaded names as first-class values) remains
//!   future work, building on the [`Type`] lattice here.

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
    /// `option<T>`.
    Option(Box<Type>),
    /// `result<O, E>`.
    Result(Box<Type>, Box<Type>),
    /// `tuple<…>`.
    Tuple(Vec<Type>),
    /// A structural (anonymous) record literal's type: its fields and their
    /// types, in literal order. Unifies with a nominal `Named` record whose
    /// declared fields match (see [`unify`]).
    Record(Vec<(String, Type)>),
    /// A `DefType` record/variant, named nominally.
    Named(String),
    /// A form — the meta layer's WIT interchange type (the `wavelet:meta`
    /// `tree` record). `Quote`/`Quasi` produce it; macro templates map it to
    /// itself; the form accessors consume it (3.7).
    Tree,
    /// The unit type (`{}`), e.g. the result of a `Def`.
    Unit,
    /// An unconstrained integer literal: compatible with any int or float type
    /// (an int type only when the carried literal value, if known, fits its
    /// range — 3.4). `None` for int-typed results whose value is not a literal
    /// (e.g. `len`).
    IntLit(Option<i64>),
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
            || matches!(self, Type::IntLit(_) | Type::FloatLit | Type::Unknown)
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
                    ("option", [elem]) => Type::Option(Box::new(Type::from_form(arena, *elem))),
                    ("result", [ok]) => Type::Result(
                        Box::new(Type::from_form(arena, *ok)),
                        Box::new(Type::Unknown),
                    ),
                    ("result", [ok, err]) => Type::Result(
                        Box::new(Type::from_form(arena, *ok)),
                        Box::new(Type::from_form(arena, *err)),
                    ),
                    ("tuple", elems) => Type::Tuple(
                        elems.iter().map(|&e| Type::from_form(arena, e)).collect(),
                    ),
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
            "tree" => Type::Tree,
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

/// A module's `DefType` declarations: how a nominal [`Type::Named`] resolves to
/// structure. Shared by unification (nominal-vs-structural records), variant
/// constructor typing, and pattern binding.
#[derive(Debug, Clone)]
pub enum TypeDef {
    /// `DefType name {x: t …}` — a nominal record.
    Record(Vec<(String, Type)>),
    /// `DefType name [case case(t) …]` — a nominal variant: each case's name
    /// and payload types (empty = nullary case).
    Variant(Vec<(String, Vec<Type>)>),
    /// `DefType name {a b c}` — flags.
    Flags(Vec<String>),
}

/// The nominal type table, keyed by `DefType` name.
type TypeTable = HashMap<String, TypeDef>;

/// Unify two types, gradually. `Unknown` absorbs anything. Numeric literals
/// unify with compatible concrete numeric types (and default toward the
/// concrete one). A structural record unifies with a nominal record whose
/// declared fields match (resolved through `tbl`). Two known, incompatible
/// concrete types fail.
fn unify(tbl: &TypeTable, a: &Type, b: &Type) -> Option<Type> {
    use Type::*;
    match (a, b) {
        (Unknown, t) | (t, Unknown) => Some(t.clone()),
        (x, y) if x == y => Some(x.clone()),

        // An integer literal unifies with any concrete int type whose range
        // admits its (known) value, resolving to that concrete type (3.4), and
        // with any float type.
        (IntLit(v), t) | (t, IntLit(v)) if t.is_int() => {
            if v.is_none_or(|n| int_in_range(n, t)) {
                Some(t.clone())
            } else {
                None
            }
        }
        (IntLit(_), t) | (t, IntLit(_)) if t.is_float() => Some(t.clone()),
        (IntLit(a), IntLit(b)) => Some(IntLit(if a == b { *a } else { None })),
        // An int literal and a float literal together are still a float literal.
        (IntLit(_), FloatLit) | (FloatLit, IntLit(_)) => Some(FloatLit),

        // A float literal unifies only with a concrete float type.
        (FloatLit, t) | (t, FloatLit) if t.is_float() => Some(t.clone()),
        (FloatLit, FloatLit) => Some(FloatLit),

        // Containers unify element-wise.
        (List(x), List(y)) => Some(List(Box::new(unify(tbl, x, y)?))),
        (Option(x), Option(y)) => Some(Option(Box::new(unify(tbl, x, y)?))),
        (Result(xo, xe), Result(yo, ye)) => Some(Result(
            Box::new(unify(tbl, xo, yo)?),
            Box::new(unify(tbl, xe, ye)?),
        )),
        (Tuple(xs), Tuple(ys)) if xs.len() == ys.len() => {
            let elems: std::option::Option<Vec<Type>> = xs
                .iter()
                .zip(ys)
                .map(|(x, y)| unify(tbl, x, y))
                .collect();
            Some(Tuple(elems?))
        }

        // A structural record literal against a nominal record type: resolve
        // the name and unify field-wise; the nominal name wins. An unresolved
        // nominal name (e.g. a type declared in another component) stays
        // gradual rather than falsely rejecting.
        (Named(n), Record(fs)) | (Record(fs), Named(n)) => match tbl.get(n) {
            Some(TypeDef::Record(dfs)) => {
                unify_fields(tbl, dfs, fs)?;
                Some(Named(n.clone()))
            }
            Some(_) => None,
            None => Some(Named(n.clone())),
        },
        // Two structural records unify field-wise over the same field-name set.
        (Record(xs), Record(ys)) => {
            let fields = unify_fields(tbl, xs, ys)?;
            Some(Record(fields))
        }

        _ => None,
    }
}

/// Unify two field lists by name: the field-name *sets* must be equal, and each
/// same-named pair must unify. Result fields keep `a`'s order.
fn unify_fields(
    tbl: &TypeTable,
    a: &[(String, Type)],
    b: &[(String, Type)],
) -> Option<Vec<(String, Type)>> {
    if a.len() != b.len() {
        return None;
    }
    let mut out = Vec::with_capacity(a.len());
    for (name, at) in a {
        let (_n, bt) = b.iter().find(|(n, _)| n == name)?;
        out.push((name.clone(), unify(tbl, at, bt)?));
    }
    Some(out)
}

/// Whether a value of type `actual` is acceptable where `expected` is required.
/// Gradual: `Unknown` on either side always passes; numeric literals are
/// class-compatible; otherwise it is unifiability.
fn compatible(tbl: &TypeTable, expected: &Type, actual: &Type) -> bool {
    unify(tbl, expected, actual).is_some()
}

/// Whether an overload candidate with parameters `params` is applicable to a
/// positional call whose static argument types are `arg_tys`: the arity must
/// match and each parameter type must be compatible with its argument. (The
/// single-bundled-payload form `f(x)` to a one-parameter `f` also matches.)
fn args_match(tbl: &TypeTable, params: &[(String, Type)], arg_tys: &[Type]) -> bool {
    if params.len() != arg_tys.len() {
        return false;
    }
    params
        .iter()
        .zip(arg_tys)
        .all(|((_n, pt), at)| compatible(tbl, pt, at))
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
    /// `DefType` declarations: nominal name -> structure (3.3).
    types: TypeTable,
    /// Variant-case index: case name -> (owning `DefType` name, payload types).
    /// Lets a constructor call like `days(30)` (or a bare nullary case) type as
    /// its nominal variant.
    variant_cases: HashMap<String, (String, Vec<Type>)>,
    /// For each *overloaded* call site (keyed by the call `Tup`'s `NodeId`), the
    /// index of the chosen candidate within its overload set. Filled in while
    /// checking; read back by [`resolve_overloads`] to rewrite the program.
    resolved: RefCell<HashMap<NodeId, usize>>,
    /// Memoised result types from [`Self::infer_sig_result`], keyed by the Fn
    /// body's `NodeId`. The inferred result depends only on the body (inference
    /// builds its own scope from the sig's params and ignores the caller's), and
    /// its side effects on `resolved` are throwaway, so re-running it for the
    /// same body is pure redundant work during return-type-directed resolution.
    sig_result_cache: RefCell<HashMap<NodeId, Type>>,
    /// Bodies currently being inferred by [`Self::infer_sig_result`]: the
    /// recursion guard. A (mutually) recursive def re-entering its own result
    /// inference gets `Unknown` for the recursive call, exactly like the
    /// `visiting` guard in [`crate::wit`]'s inference.
    sig_in_progress: RefCell<std::collections::HashSet<NodeId>>,
}

/// Check a whole program (the top-level roots). Returns `Err(msg)` on the first
/// type error, where `msg` is already in the `eval error: …` surface form so it
/// can be returned directly as [`crate::EvalOutcome::error`].
pub fn check_program(arena: &Arena, roots: &[NodeId]) -> Result<(), String> {
    let checker = Checker::collect(arena, roots);
    checker.check_roots(roots)
}

/// A WIT-rendered inference outcome, for [`infer_wit_result`]. Mirrors the
/// shape `wit::infer` reports: a concrete WIT type text, unit, or unknown.
pub enum InferredWit {
    Known(String),
    Unit,
    Unknown,
}

/// Phase B bridge (3.8): infer a def's *result* WIT type using the full Phase
/// A/C checker (which models lists, options, results, tuples, records, and
/// nominal `DefType`s), rendering the inferred [`Type`] as WIT text.
/// `params` are the def's parameters as `(name, wit-type-text)` pairs.
pub fn infer_wit_result(
    arena: &Arena,
    roots: &[NodeId],
    params: &[(String, String)],
    body: NodeId,
) -> InferredWit {
    let checker = Checker::collect(arena, roots);
    let mut scope: Scope = params
        .iter()
        .map(|(n, t)| (n.clone(), type_from_wit_text(t)))
        .collect();
    let Ok(ty) = checker.infer(body, None, &mut scope) else {
        return InferredWit::Unknown;
    };
    match ty {
        Type::Unit => InferredWit::Unit,
        other => match type_to_wit(&other) {
            Some(text) => InferredWit::Known(text),
            None => InferredWit::Unknown,
        },
    }
}

/// Parse WIT type *text* (`list<s32>`, `option<string>`, `result<a, b>`,
/// `tuple<a, b>`, or a bare name) into a checker [`Type`]. Anything
/// unrecognized is gradual `Unknown`-free `Named`, exactly like
/// [`Type::from_name`].
fn type_from_wit_text(text: &str) -> Type {
    let text = text.trim();
    if let Some(inner) = text.strip_prefix("list<").and_then(|t| t.strip_suffix('>')) {
        return Type::List(Box::new(type_from_wit_text(inner)));
    }
    if let Some(inner) = text.strip_prefix("option<").and_then(|t| t.strip_suffix('>')) {
        return Type::Option(Box::new(type_from_wit_text(inner)));
    }
    if let Some(inner) = text.strip_prefix("result<").and_then(|t| t.strip_suffix('>')) {
        let parts = split_wit_args(inner);
        return match parts.as_slice() {
            [ok] => Type::Result(Box::new(type_from_wit_text(ok)), Box::new(Type::Unknown)),
            [ok, err] => Type::Result(
                Box::new(type_from_wit_text(ok)),
                Box::new(type_from_wit_text(err)),
            ),
            _ => Type::Unknown,
        };
    }
    if let Some(inner) = text.strip_prefix("tuple<").and_then(|t| t.strip_suffix('>')) {
        return Type::Tuple(split_wit_args(inner).iter().map(|t| type_from_wit_text(t)).collect());
    }
    if text.contains('<') || text.contains('>') {
        return Type::Unknown;
    }
    Type::from_name(text)
}

/// Split `a, b, c` at top-level commas (respecting `<…>` nesting).
fn split_wit_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut cur = String::new();
    for ch in s.chars() {
        match ch {
            '<' => {
                depth += 1;
                cur.push(ch);
            }
            '>' => {
                depth = depth.saturating_sub(1);
                cur.push(ch);
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur = String::new();
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Render a checker [`Type`] as WIT type text, if it names a concrete WIT
/// type. Numeric literals default to their WIT widths (`s64`/`f64` — the
/// defaulting rule). `None` for anything without a boundary spelling
/// (gradual `Unknown`, structural records, half-known results).
pub(crate) fn type_to_wit(t: &Type) -> Option<String> {
    match t {
        Type::Bool => Some("bool".into()),
        Type::U8 => Some("u8".into()),
        Type::U16 => Some("u16".into()),
        Type::U32 => Some("u32".into()),
        Type::U64 => Some("u64".into()),
        Type::S8 => Some("s8".into()),
        Type::S16 => Some("s16".into()),
        Type::S32 => Some("s32".into()),
        Type::S64 => Some("s64".into()),
        Type::F32 => Some("f32".into()),
        Type::F64 => Some("f64".into()),
        Type::Char => Some("char".into()),
        Type::String => Some("string".into()),
        Type::IntLit(_) => Some("s64".into()),
        Type::FloatLit => Some("f64".into()),
        Type::List(e) => Some(format!("list<{}>", type_to_wit(e)?)),
        Type::Option(e) => Some(format!("option<{}>", type_to_wit(e)?)),
        Type::Result(o, e) => Some(format!("result<{}, {}>", type_to_wit(o)?, type_to_wit(e)?)),
        Type::Tuple(ts) => {
            let parts: Option<Vec<String>> = ts.iter().map(type_to_wit).collect();
            Some(format!("tuple<{}>", parts?.join(", ")))
        }
        Type::Named(n) => Some(n.clone()),
        Type::Tree => None,
        Type::Record(_) | Type::Unit | Type::Unknown => None,
    }
}

impl<'a> Checker<'a> {
    /// First pass: collect every module-level Def name and Fn signature so
    /// forward and mutual references resolve. Same-named Fn defs accumulate into
    /// an overload set (a `Vec<Sig>`), in file order.
    fn collect(arena: &'a Arena, roots: &[NodeId]) -> Self {
        let mut sigs: HashMap<String, Vec<Sig>> = HashMap::new();
        let mut defs = std::collections::HashSet::new();
        let mut types: TypeTable = HashMap::new();
        let mut variant_cases: HashMap<String, (String, Vec<Type>)> = HashMap::new();
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
            if let Some((name, def)) = as_deftype(arena, root) {
                if let TypeDef::Variant(cases) = &def {
                    for (case, payload) in cases {
                        variant_cases
                            .insert(case.clone(), (name.to_string(), payload.clone()));
                    }
                }
                types.insert(name.to_string(), def);
            }
        }
        Checker {
            arena,
            sigs,
            defs,
            types,
            variant_cases,
            resolved: RefCell::new(HashMap::new()),
            sig_result_cache: RefCell::new(HashMap::new()),
            sig_in_progress: RefCell::new(std::collections::HashSet::new()),
        }
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
pub fn resolve_overloads(arena: Arena, roots: &[NodeId]) -> Result<(Arena, Vec<NodeId>), String> {
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

/// If `id` is `DefType name decl`, parse the declaration into a [`TypeDef`].
fn as_deftype(arena: &Arena, id: NodeId) -> Option<(&str, TypeDef)> {
    let Node::Tup(items) = arena.node(id) else {
        return None;
    };
    let [head, name_id, decl] = items.as_slice() else {
        return None;
    };
    let Node::Sym(h) = arena.node(*head) else {
        return None;
    };
    if h != "deftype-MACRO" {
        return None;
    }
    let Node::Sym(name) = arena.node(*name_id) else {
        return None;
    };
    let def = match arena.node(*decl) {
        Node::Rec(fields) => TypeDef::Record(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), Type::from_form(arena, *v)))
                .collect(),
        ),
        Node::Lst(cases) => {
            let mut out = Vec::with_capacity(cases.len());
            for &c in cases {
                match arena.node(c) {
                    Node::Sym(case) => out.push((case.clone(), Vec::new())),
                    Node::Tup(case_items) => {
                        let (&h, payload) = case_items.split_first()?;
                        let Node::Sym(case) = arena.node(h) else {
                            return None;
                        };
                        let tys = payload
                            .iter()
                            .map(|&t| Type::from_form(arena, t))
                            .collect();
                        out.push((case.clone(), tys));
                    }
                    _ => return None,
                }
            }
            TypeDef::Variant(out)
        }
        Node::Flg(names) => TypeDef::Flags(names.clone()),
        _ => return None,
    };
    Some((name, def))
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
            && !compatible(&self.types, exp, &ty)
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
            Node::Int(n) => Ok(Type::IntLit(Some(*n))),
            Node::Dec(_) => Ok(Type::FloatLit),
            Node::Char(_) => Ok(Type::Char),
            Node::Str(_) => Ok(Type::String),
            Node::Sym(name) => self.infer_name(name, scope),
            // A qualified name (`alias/fn`) reaches into an imported component we
            // do not model here.
            Node::Qsym(..) => Ok(Type::Unknown),
            Node::Lst(items) => self.infer_list(items, expected, scope),
            // A record literal in value position types structurally (3.3): its
            // fields' types, checked against the expected record's declared
            // field types when the context supplies one (a nominal `Named`
            // record resolves through the `DefType` table).
            Node::Rec(fields) => {
                let exp_fields: Option<Vec<(String, Type)>> = match expected {
                    Some(Type::Record(fs)) => Some(fs.clone()),
                    Some(Type::Named(n)) => match self.types.get(n) {
                        Some(TypeDef::Record(fs)) => Some(fs.clone()),
                        _ => None,
                    },
                    _ => None,
                };
                let mut out = Vec::with_capacity(fields.len());
                for (k, v) in fields {
                    let exp = exp_fields.as_ref().and_then(|fs| {
                        fs.iter().find(|(n, _)| n == k).map(|(_, t)| t.clone())
                    });
                    let t = self.check(*v, exp.as_ref(), scope)?;
                    out.push((k.clone(), t));
                }
                Ok(Type::Record(out))
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
        if name == "none" {
            return Ok(Type::Option(Box::new(Type::Unknown)));
        }
        if name == "pi" {
            return Ok(Type::F64);
        }
        if is_builtin(name) {
            return Ok(Type::Unknown);
        }
        // A nullary case of a `DefType` variant used as a value: its nominal
        // variant type (3.3).
        if let Some((tyname, payload)) = self.variant_cases.get(name)
            && payload.is_empty()
        {
            return Ok(Type::Named(tyname.clone()));
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
            } else if let Some(u) = unify(&self.types, &elem, &t) {
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
                match unify(&self.types, &tt, &et) {
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
                let scrut_ty = self.check(scrut, None, scope)?;
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
                    // Bind the pattern's variables at the types the scrutinee
                    // implies (3.3): a variant-case pattern binds its payload at
                    // the declared payload types, a record pattern binds fields
                    // at their field types, and so on. Anything the scrutinee
                    // type doesn't determine binds as Unknown.
                    let mark = scope.len();
                    self.bind_pattern(pair[0], &scrut_ty, scope);
                    let rt = self.check(pair[1], expected, scope)?;
                    scope.truncate(mark);
                    result = Some(match result {
                        None => rt,
                        Some(prev) => unify(&self.types, &prev, &rt).ok_or_else(|| {
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
            // `Quote` produces a form: the `tree` interchange type (3.7). Its
            // contents are data, never value-checked.
            "quote-MACRO" => Ok(Type::Tree),
            // `Quasi` also produces a `tree`, but its `Unquote`/`Splice` holes
            // contain ordinary expressions — check them (3.7).
            "quasi-MACRO" => {
                if let [form] = args {
                    self.check_quasi(*form, scope, 1)?;
                }
                Ok(Type::Tree)
            }
            // A `DefMacro` template is tree -> tree (3.7): its parameters are
            // bound forms, and its body is ordinary code evaluated at expansion
            // time — check it with the parameters in scope at `tree`.
            "defmacro-MACRO" => {
                if let [_name_id, params_id, body] = args {
                    let params = parse_params(self.arena, *params_id);
                    let mark = scope.len();
                    for (n, _t) in &params {
                        scope.push((n.clone(), Type::Tree));
                    }
                    self.check(*body, None, scope)?;
                    scope.truncate(mark);
                }
                Ok(Type::Unit)
            }
            // Top-level file forms (`Package`/`Import`/`Export`/`DefType`)
            // carry annotations, not value expressions: opaque to the value
            // checker.
            "package-MACRO" | "import-MACRO" | "export-MACRO" | "deftype-MACRO" => {
                Ok(Type::Unknown)
            }
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

    /// Walk a `Quasi` template's form. Everything is data except the
    /// `Unquote`/`Splice` holes at depth 1, whose contents are ordinary
    /// expressions checked in the enclosing scope; nesting mirrors the
    /// interpreter's depth rules (`interp::quasi`).
    fn check_quasi(&self, id: NodeId, scope: &mut Scope, depth: u32) -> Result<(), String> {
        match self.arena.node(id) {
            Node::Tup(items) => {
                if items.len() == 2
                    && let Node::Sym(name) = self.arena.node(items[0])
                {
                    let arg = items[1];
                    match name.as_str() {
                        "unquote-MACRO" if depth == 1 => {
                            self.check(arg, None, scope)?;
                            return Ok(());
                        }
                        "splice-MACRO" if depth == 1 => {
                            // A splice's expression must be a list (the
                            // interpreter enforces this at runtime); a `tree`
                            // can itself be a list form.
                            let t = self.check(arg, None, scope)?;
                            if !matches!(t, Type::List(_) | Type::Tree | Type::Unknown) {
                                return Err(format!(
                                    "eval error: Splice expects a list, got {t:?}"
                                ));
                            }
                            return Ok(());
                        }
                        "unquote-MACRO" | "splice-MACRO" if depth > 1 => {
                            return self.check_quasi(arg, scope, depth - 1);
                        }
                        "quasi-MACRO" => {
                            return self.check_quasi(arg, scope, depth + 1);
                        }
                        _ => {}
                    }
                }
                for &it in items {
                    self.check_quasi(it, scope, depth)?;
                }
                Ok(())
            }
            Node::Lst(items) => {
                for &it in items {
                    self.check_quasi(it, scope, depth)?;
                }
                Ok(())
            }
            Node::Rec(fields) => {
                for (_k, v) in fields {
                    self.check_quasi(*v, scope, depth)?;
                }
                Ok(())
            }
            _ => Ok(()),
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
                // The interpreter only conformance-checks a *bare `Sym`*
                // annotation against the runtime value (interp `the-MACRO`); a
                // constructor annotation like `list(s32)` is not checked at all.
                // Mirror that: propagate the expected type (so return-type-
                // directed overload resolution still fires) only for a `Sym`
                // annotation, so we never reject a program the interpreter runs,
                // e.g. `The list(s32) ["a"]`.
                let prop = matches!(self.arena.node(ty_form), Node::Sym(_)).then_some(ty);
                self.check(expr, prop, scope)?;
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
        // A constructor call of a `DefType` variant case: `days(30)` types as
        // its nominal variant (3.3).
        if let Some((tyname, payload)) = self.variant_cases.get(name).cloned() {
            return self.check_variant_ctor(name, &tyname, &payload, args, scope);
        }
        // A builtin we model, or one we don't (Unknown).
        self.check_builtin_call(name, args, scope)
    }

    /// Check a variant-case constructor call `case(args…)` against the case's
    /// declared payload types; the call's type is the owning nominal variant.
    fn check_variant_ctor(
        &self,
        case: &str,
        tyname: &str,
        payload: &[Type],
        args: &[NodeId],
        scope: &mut Scope,
    ) -> Result<Type, String> {
        if args.len() != payload.len() {
            return Err(format!(
                "eval error: variant case `{case}` of `{tyname}` takes {} argument(s), got {}",
                payload.len(),
                args.len()
            ));
        }
        for (&a, pt) in args.iter().zip(payload) {
            self.check(a, Some(pt), scope)?;
        }
        Ok(Type::Named(tyname.to_string()))
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
            .filter(|&i| args_match(&self.types, &sigs[i].params, &arg_tys))
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
                .filter(|&i| compatible(&self.types, exp, &self.infer_sig_result(&sigs[i])))
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
        // The result depends only on the body, so memoise it: a body can recur
        // through return-type-directed resolution and is also inferred again for
        // the chosen winner, all yielding the same type.
        if let Some(cached) = self.sig_result_cache.borrow().get(&body) {
            return cached.clone();
        }
        // Recursion guard: a recursive (or mutually recursive) def re-enters
        // its own result inference through the call in its body. The recursive
        // occurrence contributes no constraint (`Unknown`); the non-recursive
        // branches still determine the result.
        if !self.sig_in_progress.borrow_mut().insert(body) {
            return Type::Unknown;
        }
        let mut scope: Scope = sig.params.clone();
        // Inference errors inside a candidate body don't disqualify it here (the
        // body is checked properly when its own Def is checked); treat them as
        // an unconstrained result so resolution stays gradual.
        let result = self.infer(body, None, &mut scope).unwrap_or(Type::Unknown);
        self.sig_in_progress.borrow_mut().remove(&body);
        self.sig_result_cache
            .borrow_mut()
            .insert(body, result.clone());
        result
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

        // The call's result type: inferred from the def's body exactly as
        // return-type-directed overload resolution does (3.2 — calling a def no
        // longer erases type information).
        let result = self.infer_sig_result(sig);

        // The single-record-arg-by-name form: `f({a: … b: …})` binds by field
        // name when the field names are exactly the parameter names — check
        // each field's type against its parameter (3.3). A record whose fields
        // do NOT match the parameter names is an ordinary single value.
        if args.len() == 1 {
            if let Type::Record(fs) = &arg_tys[0] {
                let mut fnames: Vec<&str> = fs.iter().map(|(n, _)| n.as_str()).collect();
                let mut pnames: Vec<&str> = sig.params.iter().map(|(n, _)| n.as_str()).collect();
                fnames.sort_unstable();
                pnames.sort_unstable();
                if fnames == pnames {
                    for (fname, ft) in fs {
                        let (_pn, pt) = sig
                            .params
                            .iter()
                            .find(|(pn, _)| pn == fname)
                            .expect("field names equal param names");
                        if !compatible(&self.types, pt, ft) {
                            return Err(self.param_type_error(fname, pt, ft, name));
                        }
                    }
                    return Ok(result);
                }
                if nparams != 1 {
                    // Field names don't bind the parameters and the record is
                    // not a single-parameter payload: the interpreter rejects
                    // this bind at runtime; stay gradual here.
                    return Ok(result);
                }
            }
            // A single argument to a single parameter taking the whole payload.
            if nparams == 1 {
                if !compatible(&self.types, &sig.params[0].1, &arg_tys[0]) {
                    return Err(self.param_type_error(
                        &sig.params[0].0,
                        &sig.params[0].1,
                        &arg_tys[0],
                        name,
                    ));
                }
                return Ok(result);
            }
        }

        // Positional call: arity must match.
        if args.len() != nparams {
            return Err(format!(
                "eval error: `{name}` expects {nparams} arguments, got {}",
                args.len()
            ));
        }
        for ((pn, pt), at) in sig.params.iter().zip(&arg_tys) {
            if !compatible(&self.types, pt, at) {
                return Err(self.param_type_error(pn, pt, at, name));
            }
        }
        Ok(result)
    }

    /// The error for an argument that does not fit its parameter. An integer
    /// literal that misses a concrete int parameter's range produces the SAME
    /// message the interpreter's runtime bind check produces (`bind_one`), so
    /// moving the check to compile time does not change the reported error.
    fn param_type_error(&self, pname: &str, pt: &Type, at: &Type, fname: &str) -> String {
        if let Type::IntLit(Some(n)) = at
            && pt.is_int()
        {
            return format!(
                "eval error: parameter `{pname}`: {n} does not conform to type `{}`",
                wit_name(pt)
            );
        }
        format!("eval error: argument `{pname}` to `{fname}` has the wrong type")
    }

    /// Check a builtin call. Every builtin has a typed signature here (3.5):
    /// its result type is modelled, and operands are constrained wherever the
    /// runtime is strict about them. Argument subexpressions are always checked.
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
            "add" | "sub" | "mul" | "div" | "rem" | "neg" | "abs" => {
                let mut result = Type::Unknown;
                let mut any_unknown = false;
                let mut seeded = false;
                for t in &arg_tys {
                    if !t.numeric() {
                        return Err(format!("eval error: `{name}` requires numeric operands"));
                    }
                    if matches!(t, Type::Unknown) {
                        any_unknown = true;
                    }
                    if !seeded {
                        result = t.clone();
                        seeded = true;
                    } else if let Some(u) = unify(&self.types, &result, t) {
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
            // `min`/`max` return one of their operands and — via the
            // interpreter's `compare` — are defined over numbers, strings, and
            // chars. The result is the unified operand type, gradual when they
            // disagree.
            "min" | "max" => {
                let mut result = Type::Unknown;
                for t in &arg_tys {
                    if !comparable(t) {
                        return Err(format!("eval error: `{name}` requires comparable operands"));
                    }
                    result = unify(&self.types, &result, t).unwrap_or(Type::Unknown);
                }
                Ok(result)
            }
            // str-cat: every arg must be string/char/unknown; result string.
            "str-cat" => {
                for t in &arg_tys {
                    if !matches!(t, Type::String | Type::Char | Type::Unknown) {
                        return Err(format!("eval error: `{name}` requires string operands"));
                    }
                }
                Ok(Type::String)
            }
            "upper" | "lower" => {
                for t in &arg_tys {
                    if !matches!(t, Type::String | Type::Unknown) {
                        return Err(format!("eval error: `{name}` requires a string operand"));
                    }
                }
                Ok(Type::String)
            }
            // `eq` is total structural equality over every type; result bool.
            "eq" => Ok(Type::Bool),
            // Ordering comparisons are defined over numbers, strings, and chars.
            "lt" | "le" | "gt" | "ge" => {
                for t in &arg_tys {
                    if !comparable(t) {
                        return Err(format!("eval error: `{name}` requires comparable operands"));
                    }
                }
                Ok(Type::Bool)
            }
            "not" => {
                for t in &arg_tys {
                    if !matches!(t, Type::Bool | Type::Unknown) {
                        return Err("eval error: `not` requires a bool operand".to_string());
                    }
                }
                Ok(Type::Bool)
            }
            // `empty`/`contains` report a property; result bool.
            "empty" | "contains" => Ok(Type::Bool),
            // `len` returns a plain Int that range-checks against any int type
            // at runtime (and promotes to float), so model it as an
            // unconstrained int literal — not concrete `s64`, which would
            // falsely reject e.g. `The u8 len(xs)` that the interpreter accepts.
            "len" => Ok(Type::IntLit(None)),
            // Option/result constructors (3.3/3.8): the payload's type flows in;
            // the other side stays unconstrained until unified against context.
            // Multiple args bundle to a tuple payload (`some(1 2)`).
            "some" => Ok(Type::Option(Box::new(ctor_payload(&arg_tys)))),
            "ok" => Ok(Type::Result(
                Box::new(ctor_payload(&arg_tys)),
                Box::new(Type::Unknown),
            )),
            "err" => Ok(Type::Result(
                Box::new(Type::Unknown),
                Box::new(ctor_payload(&arg_tys)),
            )),
            // Sequence ops (3.5/3.8): element types flow through.
            "get" | "head" => Ok(elem_type(arg_tys.first())),
            "tail" | "reverse" => Ok(arg_tys.first().cloned().unwrap_or(Type::Unknown)),
            "put" if arg_tys.len() == 3 => {
                let elem = unify(&self.types, &elem_type(arg_tys.first()), &arg_tys[2])
                    .unwrap_or(Type::Unknown);
                Ok(Type::List(Box::new(elem)))
            }
            "push" if arg_tys.len() == 2 => {
                let elem = unify(&self.types, &elem_type(arg_tys.first()), &arg_tys[1])
                    .unwrap_or(Type::Unknown);
                Ok(Type::List(Box::new(elem)))
            }
            "concat" if arg_tys.len() == 2 => {
                Ok(unify(&self.types, &arg_tys[0], &arg_tys[1])
                    .unwrap_or(Type::List(Box::new(Type::Unknown))))
            }
            "range" => Ok(Type::List(Box::new(Type::S64))),
            "zip" if arg_tys.len() == 2 => Ok(Type::List(Box::new(Type::Tuple(vec![
                elem_type(arg_tys.first()),
                elem_type(arg_tys.get(1)),
            ])))),
            // The mapper's result type is not modelled (closures are untyped in
            // Phase A), so `map` yields list<unknown>; `filter` preserves its
            // sequence's type; `fold`'s accumulator is unconstrained.
            "map" => Ok(Type::List(Box::new(Type::Unknown))),
            "filter" => Ok(arg_tys.get(1).cloned().unwrap_or(Type::Unknown)),
            "fold" => Ok(Type::Unknown),
            "split" => Ok(Type::List(Box::new(Type::String))),
            "join" => Ok(Type::String),
            "to-string" | "form-kind" => Ok(Type::String),
            // `read` produces a form or an error string (3.7).
            "read" => Ok(Type::Result(Box::new(Type::Tree), Box::new(Type::String))),
            // Meta-layer builtins are ordinary typed functions over `tree`
            // (3.7): `gensym` mints a fresh symbol form, `expand` maps a form
            // to a form, `rec-key` projects a record form's first key as a
            // symbol form. (`rec-val` yields the field's *value*, which on a
            // runtime record can be anything — it stays gradual.)
            "gensym" | "expand" | "rec-key" => Ok(Type::Tree),
            // Numeric conversions: the result is the named concrete type. A
            // literal argument is range-checked at compile time with the SAME
            // message the runtime conversion produces (3.4).
            "to-u8" | "to-u16" | "to-u32" | "to-u64" | "to-s8" | "to-s16" | "to-s32"
            | "to-s64" => {
                let ty = Type::from_name(&name[3..]);
                if let [arg] = args
                    && let Node::Int(n) = self.arena.node(*arg)
                    && !int_in_range(*n, &ty)
                {
                    return Err(format!("eval error: `{name}`: {n} out of range"));
                }
                Ok(ty)
            }
            "to-f32" => Ok(Type::F32),
            "to-f64" => Ok(Type::F64),
            "to-char" => {
                if let [arg] = args
                    && let Node::Int(n) = self.arena.node(*arg)
                    && u32::try_from(*n).ok().and_then(char::from_u32).is_none()
                {
                    return Err(format!(
                        "eval error: `to-char`: {n} is not a Unicode scalar value"
                    ));
                }
                Ok(Type::Char)
            }
            // Unit-returning effects.
            "cell-set" | "drop" => Ok(Type::Unit),
            // Everything else (apply/gensym/expand/rec-key/rec-val/cell-new/
            // cell-get, form accessors): result Unknown, args unconstrained
            // (already checked above). 3.7 types the form accessors as tree.
            _ => Ok(Type::Unknown),
        }
    }

    /// Bind every variable name appearing in a Match pattern, at the type the
    /// scrutinee's type `ty` implies for its position (3.3); `Unknown` wherever
    /// the scrutinee type does not determine it.
    fn bind_pattern(&self, pat: NodeId, ty: &Type, scope: &mut Scope) {
        match self.arena.node(pat) {
            Node::Sym(name) => {
                // A bare symbol that names a nullary case of the scrutinee's
                // type matches by equality and binds nothing; any other symbol
                // binds the whole scrutinee.
                if self.case_payload(name, ty).is_some_and(|p| p.is_empty()) {
                    return;
                }
                scope.push((name.clone(), ty.clone()));
            }
            Node::Tup(items) => {
                // A variant-case pattern `(case p…)` against a scrutinee whose
                // type declares that case: bind the sub-patterns at the payload
                // types.
                if let Some((&h, rest)) = items.split_first()
                    && let Node::Sym(case) = self.arena.node(h)
                    && let Some(payload) = self.case_payload(case, ty)
                {
                    if rest.len() == payload.len() {
                        for (&p, t) in rest.iter().zip(&payload) {
                            self.bind_pattern(p, t, scope);
                        }
                    } else if rest.len() == 1 && payload.len() > 1 {
                        // One pattern against a bundled tuple payload.
                        self.bind_pattern(rest[0], &Type::Tuple(payload), scope);
                    } else {
                        for &p in rest {
                            self.bind_pattern(p, &Type::Unknown, scope);
                        }
                    }
                    return;
                }
                // An element-wise tuple pattern against a tuple type.
                if let Type::Tuple(ts) = ty
                    && ts.len() == items.len()
                {
                    for (&p, t) in items.iter().zip(ts) {
                        self.bind_pattern(p, t, scope);
                    }
                    return;
                }
                for &it in items {
                    self.bind_pattern(it, &Type::Unknown, scope);
                }
            }
            Node::Lst(items) => {
                let elem = match ty {
                    Type::List(e) => (**e).clone(),
                    _ => Type::Unknown,
                };
                for &it in items {
                    self.bind_pattern(it, &elem, scope);
                }
            }
            Node::Rec(fields) => {
                let rec_fields: Option<Vec<(String, Type)>> = match ty {
                    Type::Record(fs) => Some(fs.clone()),
                    Type::Named(n) => match self.types.get(n) {
                        Some(TypeDef::Record(fs)) => Some(fs.clone()),
                        _ => None,
                    },
                    _ => None,
                };
                for (k, v) in fields {
                    let ft = rec_fields
                        .as_ref()
                        .and_then(|fs| fs.iter().find(|(n, _)| n == k).map(|(_, t)| t.clone()))
                        .unwrap_or(Type::Unknown);
                    self.bind_pattern(*v, &ft, scope);
                }
            }
            // Literals in patterns bind nothing.
            _ => {}
        }
    }

    /// The payload types of variant case `case` under scrutinee type `ty`:
    /// `Some(payloads)` when `ty` declares that case (a `DefType` variant, an
    /// `option`, or a `result`), `None` otherwise. A nullary case yields an
    /// empty payload list.
    fn case_payload(&self, case: &str, ty: &Type) -> Option<Vec<Type>> {
        match ty {
            Type::Named(n) => match self.types.get(n)? {
                TypeDef::Variant(cases) => cases
                    .iter()
                    .find(|(c, _)| c == case)
                    .map(|(_, p)| p.clone()),
                _ => None,
            },
            Type::Option(t) => match case {
                "some" => Some(vec![(**t).clone()]),
                "none" => Some(Vec::new()),
                _ => None,
            },
            Type::Result(o, e) => match case {
                "ok" => Some(vec![(**o).clone()]),
                "err" => Some(vec![(**e).clone()]),
                _ => None,
            },
            _ => None,
        }
    }
}

/// The element type of a sequence operand: `list<t>` yields `t`; anything else
/// (including a tuple, whose per-index type a dynamic `get` cannot pin) is
/// gradual `Unknown`.
fn elem_type(t: Option<&Type>) -> Type {
    match t {
        Some(Type::List(e)) => (**e).clone(),
        _ => Type::Unknown,
    }
}

/// The payload type of an option/result constructor call from its bundled
/// argument types: no argument is unit-ish `Unknown`, one argument is that
/// value, several bundle into a tuple.
fn ctor_payload(arg_tys: &[Type]) -> Type {
    match arg_tys {
        [] => Type::Unknown,
        [one] => one.clone(),
        many => Type::Tuple(many.to_vec()),
    }
}

/// Whether a type is admissible to the ordering builtins (`lt`/`min`/…):
/// numbers, strings, and chars, plus gradual `Unknown`.
fn comparable(t: &Type) -> bool {
    t.numeric() || matches!(t, Type::String | Type::Char)
}

/// The WIT name of a concrete primitive type, for error messages.
fn wit_name(t: &Type) -> &'static str {
    match t {
        Type::Bool => "bool",
        Type::U8 => "u8",
        Type::U16 => "u16",
        Type::U32 => "u32",
        Type::U64 => "u64",
        Type::S8 => "s8",
        Type::S16 => "s16",
        Type::S32 => "s32",
        Type::S64 => "s64",
        Type::F32 => "f32",
        Type::F64 => "f64",
        Type::Char => "char",
        Type::String => "string",
        _ => "?",
    }
}

/// Whether `n` fits the integer type `ty`. Delegates to [`crate::value::int_fits`],
/// the single source of truth shared with the runtime `The` check
/// (`interp::check_type`), so the compile-time and runtime bounds cannot drift.
fn int_in_range(n: i64, ty: &Type) -> bool {
    let name = match ty {
        Type::U8 => "u8",
        Type::U16 => "u16",
        Type::U32 => "u32",
        Type::U64 => "u64",
        Type::S8 => "s8",
        Type::S16 => "s16",
        Type::S32 => "s32",
        Type::S64 => "s64",
        // A float type accepts any integer literal (int promotes to float). Any
        // non-numeric ascription target also leaves the literal unconstrained
        // here; gradual elsewhere.
        _ => return true,
    };
    // `int_fits` returns `Some` for all eight names matched above.
    crate::value::int_fits(name, n).unwrap_or(true)
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
