mod builtins;
mod local_ownership;
mod parameter_inference;
mod resource_state;

use std::cell::RefCell;
use std::collections::HashMap;

use crate::form::{Arena, Node, NodeId};

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

    Option(Box<Type>),

    Result(Box<Type>, Box<Type>),

    Tuple(Vec<Type>),

    Record(Vec<(String, Type)>),

    Named(String),

    Tree,

    Flags(Vec<String>),

    Unit,

    Fn(Vec<Type>, Box<Type>),
    Cell(Box<Type>),

    IntLit(Option<i64>),

    FloatLit,

    Unknown,
}

impl Type {
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

    fn is_float(&self) -> bool {
        matches!(self, Type::F32 | Type::F64)
    }

    fn numeric(&self) -> bool {
        self.is_int()
            || self.is_float()
            || matches!(self, Type::IntLit(_) | Type::FloatLit | Type::Unknown)
    }

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

                    ("result", [ok]) => {
                        Type::Result(Box::new(Self::result_arm(arena, *ok)), Box::new(Type::Unit))
                    }
                    ("result", [ok, err]) => Type::Result(
                        Box::new(Self::result_arm(arena, *ok)),
                        Box::new(Self::result_arm(arena, *err)),
                    ),
                    ("tuple", elems) => {
                        Type::Tuple(elems.iter().map(|&e| Type::from_form(arena, e)).collect())
                    }

                    ("own", [inner]) | ("borrow", [inner]) => Type::from_form(arena, *inner),
                    _ => Type::Unknown,
                }
            }
            _ => Type::Unknown,
        }
    }

    fn result_arm(arena: &Arena, id: NodeId) -> Type {
        if matches!(arena.node(id), Node::Sym(s) if s == "_") {
            return Type::Unit;
        }
        Type::from_form(arena, id)
    }

    fn from_name(s: &str) -> Type {
        match s {
            "bool" => Type::Bool,

            "result" => Type::Result(Box::new(Type::Unit), Box::new(Type::Unit)),
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

            other => Type::Named(other.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub enum TypeDef {
    Resource,
    Record(Vec<(String, Type)>),

    Variant(Vec<(String, Vec<Type>)>),

    Flags(Vec<String>),

    Alias(Type),
}

type TypeTable = HashMap<String, TypeDef>;

fn unify(tbl: &TypeTable, a: &Type, b: &Type) -> Option<Type> {
    use Type::*;
    match (a, b) {
        (Unknown, t) | (t, Unknown) => Some(t.clone()),
        (x, y) if x == y => Some(x.clone()),

        (IntLit(v), t) | (t, IntLit(v)) if t.is_int() => {
            if v.is_none_or(|n| int_in_range(n, t)) {
                Some(t.clone())
            } else {
                None
            }
        }
        (IntLit(_), t) | (t, IntLit(_)) if t.is_float() => Some(t.clone()),
        (IntLit(a), IntLit(b)) => Some(IntLit(if a == b { *a } else { None })),

        (IntLit(_), FloatLit) | (FloatLit, IntLit(_)) => Some(FloatLit),

        (FloatLit, t) | (t, FloatLit) if t.is_float() => Some(t.clone()),
        (FloatLit, FloatLit) => Some(FloatLit),

        (Tree, t @ Tuple(_)) | (t @ Tuple(_), Tree) => Some(t.clone()),
        (Tree, t @ List(_)) | (t @ List(_), Tree) => Some(t.clone()),

        (List(x), List(y)) => Some(List(Box::new(unify(tbl, x, y)?))),
        (Cell(x), Cell(y)) => Some(Cell(Box::new(unify(tbl, x, y)?))),
        (Option(x), Option(y)) => Some(Option(Box::new(unify(tbl, x, y)?))),
        (Result(xo, xe), Result(yo, ye)) => Some(Result(
            Box::new(unify(tbl, xo, yo)?),
            Box::new(unify(tbl, xe, ye)?),
        )),
        (Fn(xs, xr), Fn(ys, yr)) if xs.len() == ys.len() => {
            let params: std::option::Option<Vec<Type>> =
                xs.iter().zip(ys).map(|(x, y)| unify(tbl, x, y)).collect();
            Some(Fn(params?, Box::new(unify(tbl, xr, yr)?)))
        }
        (Tuple(xs), Tuple(ys)) if xs.len() == ys.len() => {
            let elems: std::option::Option<Vec<Type>> =
                xs.iter().zip(ys).map(|(x, y)| unify(tbl, x, y)).collect();
            Some(Tuple(elems?))
        }

        (Named(n), t) if matches!(tbl.get(n), Some(TypeDef::Alias(_))) => {
            let Some(TypeDef::Alias(target)) = tbl.get(n) else {
                unreachable!("guard matched an alias")
            };
            unify(tbl, target, t)?;
            Some(Named(n.clone()))
        }
        (t, Named(n)) if matches!(tbl.get(n), Some(TypeDef::Alias(_))) => {
            let Some(TypeDef::Alias(target)) = tbl.get(n) else {
                unreachable!("guard matched an alias")
            };
            unify(tbl, t, target)?;
            Some(Named(n.clone()))
        }

        (Named(left), Named(right)) if left.starts_with("wit-x") || right.starts_with("wit-x") => {
            unify_dependency_types(tbl, left, right)
        }

        (Named(name), other) | (other, Named(name)) => unify_named_type(tbl, name, other),

        (Record(xs), Record(ys)) => {
            let fields = unify_fields(tbl, xs, ys)?;
            Some(Record(fields))
        }

        _ => None,
    }
}

fn unify_named_type(tbl: &TypeTable, name: &str, other: &Type) -> Option<Type> {
    match (tbl.get(name), other) {
        (None, _) => Some(Type::Named(name.to_owned())),
        (Some(TypeDef::Flags(declared)), Type::Flags(members)) => members
            .iter()
            .all(|member| declared.contains(member))
            .then(|| Type::Named(name.to_owned())),
        (Some(TypeDef::Record(declared)), Type::Record(fields)) => {
            unify_fields(tbl, declared, fields)?;
            Some(Type::Named(name.to_owned()))
        }
        _ => None,
    }
}

fn unify_dependency_types(tbl: &TypeTable, left: &str, right: &str) -> Option<Type> {
    match (tbl.get(left)?, tbl.get(right)?) {
        (TypeDef::Record(left), TypeDef::Record(right)) => {
            unify_fields(tbl, left, right)?;
        }
        (TypeDef::Variant(left), TypeDef::Variant(right)) if left.len() == right.len() => {
            for ((left_case, left_payload), (right_case, right_payload)) in left.iter().zip(right) {
                if left_case != right_case || left_payload.len() != right_payload.len() {
                    return None;
                }
                for (left, right) in left_payload.iter().zip(right_payload) {
                    unify(tbl, left, right)?;
                }
            }
        }
        (TypeDef::Flags(left), TypeDef::Flags(right)) if left == right => {}
        _ => return None,
    }
    Some(Type::Named(left.to_owned()))
}

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

fn compatible(tbl: &TypeTable, expected: &Type, actual: &Type) -> bool {
    unify(tbl, expected, actual).is_some()
}

fn compatible_widening(tbl: &TypeTable, expected: &Type, actual: &Type) -> bool {
    if int_widens(actual, expected) {
        return true;
    }
    if let (Type::Result(eo, ee), Type::Result(ao, ae)) = (expected, actual) {
        let arm_ok = |e: &Type, a: &Type| matches!(e, Type::Unit) || compatible(tbl, e, a);
        if arm_ok(eo, ao) && arm_ok(ee, ae) {
            return true;
        }
    }
    compatible(tbl, expected, actual)
}

fn int_class(t: &Type) -> Option<(bool, u8)> {
    match t {
        Type::U8 => Some((false, 0)),
        Type::U16 => Some((false, 1)),
        Type::U32 => Some((false, 2)),
        Type::U64 => Some((false, 3)),
        Type::S8 => Some((true, 0)),
        Type::S16 => Some((true, 1)),
        Type::S32 => Some((true, 2)),
        Type::S64 => Some((true, 3)),
        _ => None,
    }
}

fn int_widens(from: &Type, to: &Type) -> bool {
    match (int_class(from), int_class(to)) {
        (Some((fs, fr)), Some((ts, tr))) => fs == ts && fr <= tr,
        _ => false,
    }
}

fn args_match(tbl: &TypeTable, params: &[(String, Type)], arg_tys: &[Type]) -> bool {
    if params.len() != arg_tys.len() {
        return false;
    }
    params
        .iter()
        .zip(arg_tys)
        .all(|((_n, pt), at)| compatible(tbl, pt, at))
}

fn mutable_type(ty: &Type) -> Type {
    match ty {
        Type::IntLit(_) => Type::S64,
        Type::FloatLit => Type::F64,
        Type::List(inner) => Type::List(Box::new(mutable_type(inner))),
        Type::Option(inner) => Type::Option(Box::new(mutable_type(inner))),
        Type::Tuple(items) => Type::Tuple(items.iter().map(mutable_type).collect()),
        Type::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), mutable_type(ty)))
                .collect(),
        ),
        Type::Result(ok, err) => {
            Type::Result(Box::new(mutable_type(ok)), Box::new(mutable_type(err)))
        }
        other => other.clone(),
    }
}

fn fully_concrete(t: &Type) -> bool {
    match t {
        Type::Unknown => false,
        Type::List(e) | Type::Option(e) | Type::Cell(e) => fully_concrete(e),
        Type::Result(o, e) => fully_concrete(o) && fully_concrete(e),
        Type::Tuple(ts) => ts.iter().all(fully_concrete),
        Type::Record(fs) => fs.iter().all(|(_, t)| fully_concrete(t)),
        Type::Fn(ps, r) => ps.iter().all(fully_concrete) && fully_concrete(r),
        _ => true,
    }
}

pub(crate) fn contains_fn(t: &Type) -> bool {
    match t {
        Type::Fn(..) => true,
        Type::List(e) | Type::Option(e) | Type::Cell(e) => contains_fn(e),
        Type::Result(o, e) => contains_fn(o) || contains_fn(e),
        Type::Tuple(ts) => ts.iter().any(contains_fn),
        Type::Record(fs) => fs.iter().any(|(_, t)| contains_fn(t)),
        _ => false,
    }
}

#[derive(Clone)]
struct Sig {
    params: Vec<(String, Type)>,

    body: Option<NodeId>,
}

type Scope = Vec<(String, Type)>;

struct Checker<'a> {
    arena: &'a Arena,

    sigs: HashMap<String, Vec<Sig>>,

    defs: std::collections::HashSet<String>,

    value_defs: HashMap<String, NodeId>,

    value_def_types: RefCell<HashMap<String, Type>>,
    value_defs_in_progress: RefCell<std::collections::HashSet<String>>,

    types: TypeTable,

    variant_cases: HashMap<String, (String, Vec<Type>)>,

    import_sigs: ImportSigs,
    resource_sigs: ImportSigs,
    resource_types: std::collections::HashSet<String>,
    imports_resolved: bool,
    import_aliases: std::collections::HashSet<String>,

    resolved: RefCell<HashMap<NodeId, usize>>,

    sig_result_cache: RefCell<HashMap<NodeId, Type>>,

    sig_in_progress: RefCell<std::collections::HashSet<NodeId>>,
    node_types: RefCell<NodeTypes>,
    speculative_node_types: RefCell<Vec<NodeTypes>>,

    export_results: HashMap<String, Type>,
}

pub type NodeTypes = HashMap<NodeId, Type>;

thread_local! {
    static STRICT: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

pub(crate) fn set_strict(on: bool) {
    STRICT.with(|state| state.set(on));
}

pub fn strict() -> bool {
    STRICT.with(std::cell::Cell::get)
}

pub type ImportSig = (Vec<(String, Type)>, Type);

pub type ImportSigs = HashMap<String, ImportSig>;

pub fn check_program(arena: &Arena, roots: &[NodeId]) -> Result<(), String> {
    let checker = Checker::collect(arena, roots);
    checker.check_roots(roots)
}

pub fn check_program_with_imports(
    arena: &Arena,
    roots: &[NodeId],
    imports: &ImportSigs,
) -> Result<(), String> {
    let mut checker = Checker::collect(arena, roots);
    checker.import_sigs = imports.clone();
    checker.imports_resolved = true;
    checker.check_roots(roots)
}

pub fn type_from_wit(text: &str) -> Type {
    type_from_wit_text(text)
}

pub fn node_types_with_imports(
    arena: &Arena,
    roots: &[NodeId],
    imports: &ImportSigs,
) -> Result<NodeTypes, String> {
    let mut checker = Checker::collect(arena, roots);
    checker.import_sigs = imports.clone();
    checker.imports_resolved = true;
    checker.check_roots(roots)?;
    Ok(checker.node_types.into_inner())
}

pub fn check_program_with_dependencies(
    arena: &Arena,
    roots: &[NodeId],
    imports: &ImportSigs,
    deps: &HashMap<String, crate::emit::Dep>,
    source_imports: &[crate::wit::ImportInfo],
) -> Result<(), String> {
    let mut checker = Checker::collect(arena, roots);
    checker.import_sigs = imports.clone();
    checker.imports_resolved = true;
    checker.add_dependency_types(deps, source_imports);
    checker.check_roots(roots)
}

pub fn node_types_with_dependencies(
    arena: &Arena,
    roots: &[NodeId],
    imports: &ImportSigs,
    deps: &HashMap<String, crate::emit::Dep>,
    source_imports: &[crate::wit::ImportInfo],
) -> Result<NodeTypes, String> {
    let mut checker = Checker::collect(arena, roots);
    checker.import_sigs = imports.clone();
    checker.imports_resolved = true;
    checker.add_dependency_types(deps, source_imports);
    checker.check_roots(roots)?;
    Ok(checker.node_types.into_inner())
}

impl Checker<'_> {
    fn add_dependency_types(
        &mut self,
        deps: &HashMap<String, crate::emit::Dep>,
        source_imports: &[crate::wit::ImportInfo],
    ) {
        for (package, dep) in deps {
            for (name, fields) in &dep.types {
                self.types.entry(name.clone()).or_insert_with(|| {
                    TypeDef::Record(
                        fields
                            .iter()
                            .map(|(name, ty)| (name.clone(), type_from_wit(ty)))
                            .collect(),
                    )
                });
            }
            for (name, ty) in &dep.aliases {
                if name == ty {
                    continue;
                }
                self.types
                    .entry(name.clone())
                    .or_insert_with(|| TypeDef::Alias(type_from_wit(ty)));
            }
            for (name, ty) in &dep.type_defs {
                let ty = match ty {
                    crate::emit::TypeDef::Resource => {
                        self.resource_types.insert(name.clone());
                        TypeDef::Resource
                    }
                    crate::emit::TypeDef::Flags(members) => TypeDef::Flags(members.clone()),
                    crate::emit::TypeDef::Enum(cases) => TypeDef::Variant(
                        cases
                            .iter()
                            .map(|name| (name.clone(), Vec::new()))
                            .collect(),
                    ),
                    crate::emit::TypeDef::Variant(cases) => TypeDef::Variant(
                        cases
                            .iter()
                            .map(|(name, payload)| {
                                (
                                    name.clone(),
                                    payload.iter().map(|ty| type_from_wit(ty)).collect(),
                                )
                            })
                            .collect(),
                    ),
                };
                if let TypeDef::Variant(cases) = &ty {
                    for import in source_imports.iter().filter(|import| {
                        import.package == *package
                            && dep.type_ifaces.iter().any(|(internal, iface, _)| {
                                internal == name
                                    && import.path.split_once('/').is_some_and(|(_, imported)| {
                                        imported.split('@').next() == Some(iface.as_str())
                                    })
                            })
                    }) {
                        for (case, payload) in cases {
                            self.variant_cases.insert(
                                format!("{}-{case}", import.alias),
                                (name.clone(), payload.clone()),
                            );
                        }
                    }
                }
                self.types.entry(name.clone()).or_insert(ty);
            }
        }
    }
}

pub enum InferredWit {
    Known(String),
    Unit,
    Unknown,

    Func,
}

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
        other if contains_fn(&other) => InferredWit::Func,
        other => match type_to_wit(&other) {
            Some(text) => InferredWit::Known(text),
            None => InferredWit::Unknown,
        },
    }
}

fn type_from_wit_text(text: &str) -> Type {
    let text = text.trim();
    if let Some(inner) = text
        .strip_prefix("own<")
        .or_else(|| text.strip_prefix("borrow<"))
        .and_then(|text| text.strip_suffix('>'))
    {
        return Type::Named(inner.to_owned());
    }
    if let Some(inner) = text.strip_prefix("list<").and_then(|t| t.strip_suffix('>')) {
        return Type::List(Box::new(type_from_wit_text(inner)));
    }
    if let Some(inner) = text
        .strip_prefix("option<")
        .and_then(|t| t.strip_suffix('>'))
    {
        return Type::Option(Box::new(type_from_wit_text(inner)));
    }
    if let Some(inner) = text
        .strip_prefix("result<")
        .and_then(|t| t.strip_suffix('>'))
    {
        let arm = |s: &str| {
            if s.trim() == "_" {
                Type::Unit
            } else {
                type_from_wit_text(s)
            }
        };
        let parts = split_wit_args(inner);
        return match parts.as_slice() {
            [ok] => Type::Result(Box::new(arm(ok)), Box::new(Type::Unit)),
            [ok, err] => Type::Result(Box::new(arm(ok)), Box::new(arm(err))),
            _ => Type::Unknown,
        };
    }
    if let Some(inner) = text
        .strip_prefix("tuple<")
        .and_then(|t| t.strip_suffix('>'))
    {
        return Type::Tuple(
            split_wit_args(inner)
                .iter()
                .map(|t| type_from_wit_text(t))
                .collect(),
        );
    }
    if text.contains('<') || text.contains('>') {
        return Type::Unknown;
    }
    Type::from_name(text)
}

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

        Type::Result(o, e) => Some(match (&**o, &**e) {
            (Type::Unit, Type::Unit) => "result".to_string(),
            (Type::Unit, _) => format!("result<_, {}>", type_to_wit(e)?),
            (_, Type::Unit) => format!("result<{}>", type_to_wit(o)?),
            _ => format!("result<{}, {}>", type_to_wit(o)?, type_to_wit(e)?),
        }),
        Type::Tuple(ts) => {
            let parts: Option<Vec<String>> = ts.iter().map(type_to_wit).collect();
            Some(format!("tuple<{}>", parts?.join(", ")))
        }
        Type::Named(n) => Some(n.clone()),
        Type::Tree => None,
        Type::Flags(_)
        | Type::Record(_)
        | Type::Unit
        | Type::Unknown
        | Type::Fn(..)
        | Type::Cell(_) => None,
    }
}

impl<'a> Checker<'a> {
    fn apply_export_params(arena: &Arena, roots: &[NodeId], sigs: &mut HashMap<String, Vec<Sig>>) {
        for &root in roots {
            let Node::Tup(items) = arena.node(root) else {
                continue;
            };
            if items.len() != 2
                || !matches!(arena.node(items[0]), Node::Sym(head) if head == "export-MACRO")
            {
                continue;
            }
            let Node::Rec(fields) = arena.node(items[1]) else {
                continue;
            };
            let Some(name) = fields.iter().find_map(|(key, value)| {
                if key == "name"
                    && let Node::Sym(name) = arena.node(*value)
                {
                    return Some(name);
                }
                None
            }) else {
                continue;
            };
            let Some(params) = fields
                .iter()
                .find_map(|(key, value)| (key == "params").then(|| parse_params(arena, *value)))
            else {
                continue;
            };
            if let Some(signatures) = sigs.get_mut(name) {
                for signature in signatures {
                    if signature.params.len() != params.len() {
                        continue;
                    }
                    for ((_, ty), (_, declared)) in signature.params.iter_mut().zip(&params) {
                        if *ty == Type::Unknown {
                            *ty = declared.clone()
                        }
                    }
                }
            }
        }
    }

    fn collect(arena: &'a Arena, roots: &[NodeId]) -> Self {
        let mut sigs: HashMap<String, Vec<Sig>> = HashMap::new();
        let mut defs = std::collections::HashSet::new();
        let mut types: TypeTable = HashMap::new();
        let mut variant_cases: HashMap<String, (String, Vec<Type>)> = HashMap::new();
        let mut value_defs: HashMap<String, NodeId> = HashMap::new();
        let mut export_results: HashMap<String, Type> = HashMap::new();
        let mut resource_sigs = ImportSigs::new();
        for &root in roots {
            if let Some((name, expr)) = as_def(arena, root) {
                defs.insert(name.to_string());
                if let Some(params) = fn_params(arena, expr) {
                    let body = fn_body(arena, expr);
                    sigs.entry(name.to_string())
                        .or_default()
                        .push(Sig { params, body });
                } else {
                    value_defs.insert(name.to_string(), expr);
                }
            }
            if let Some((name, def)) = as_deftype(arena, root) {
                if let TypeDef::Variant(cases) = &def {
                    for (case, payload) in cases {
                        variant_cases.insert(case.clone(), (name.to_string(), payload.clone()));
                    }
                }
                types.insert(name.to_string(), def);
            }
            if let Some((name, result)) = as_export_result(arena, root)
                && !matches!(result, Type::Unknown)
            {
                export_results.insert(name, result);
            }
        }
        Self::apply_export_params(arena, roots, &mut sigs);
        for &root in roots {
            if let Some((resource, members)) = as_defresource(arena, root) {
                for (name, node) in members {
                    if name == "Drop" {
                        continue;
                    }
                    let (_, function) = strip_static(arena, *node);
                    let Some(params) = fn_params(arena, function) else {
                        continue;
                    };
                    let result = if name == "New" {
                        Type::Named(resource.to_owned())
                    } else if let Some(body) = fn_body(arena, function) {
                        match arena.node(body) {
                            Node::Tup(items)
                                if items.len() == 3
                                    && matches!(arena.node(items[0]), Node::Sym(head) if head == "the-MACRO") =>
                            {
                                Type::from_form(arena, items[1])
                            }
                            _ => Type::Unknown,
                        }
                    } else {
                        Type::Unknown
                    };
                    let exposed = if name == "New" {
                        resource.to_owned()
                    } else {
                        format!("{resource}-{name}")
                    };
                    resource_sigs.insert(exposed, (params, result));
                }
            }
        }
        parameter_inference::infer(arena, &mut sigs);
        let mut import_aliases = std::collections::HashSet::new();
        for &root in roots {
            let Node::Tup(items) = arena.node(root) else {
                continue;
            };
            if items.len() < 2 {
                continue;
            }
            if matches!(arena.node(items[0]), Node::Sym(head) if head == "import-MACRO" || head == "instantiate-MACRO")
            {
                match arena.node(items[1]) {
                    Node::Rec(fields) => {
                        let alias = fields.iter().find_map(|(key, value)| {
                            match (key.as_str(), arena.node(*value)) {
                                ("as", Node::Sym(name)) => Some(name.clone()),
                                _ => None,
                            }
                        });
                        let package = fields.iter().find_map(|(key, value)| {
                            match (key.as_str(), arena.node(*value)) {
                                ("pkg", Node::Str(name)) => Some(name.clone()),
                                _ => None,
                            }
                        });
                        if let Some(alias) = alias.or_else(|| {
                            package.map(|p| {
                                p.split('@')
                                    .next()
                                    .unwrap()
                                    .rsplit('/')
                                    .next()
                                    .unwrap()
                                    .to_owned()
                            })
                        }) {
                            import_aliases.insert(alias);
                        }
                    }
                    Node::Str(package) => {
                        import_aliases.insert(
                            package
                                .split('@')
                                .next()
                                .unwrap()
                                .rsplit('/')
                                .next()
                                .unwrap()
                                .to_owned(),
                        );
                    }
                    _ => {}
                }
            }
        }
        let mut checker = Checker {
            arena,
            sigs,
            defs,
            types,
            variant_cases,
            value_defs,
            value_def_types: RefCell::new(HashMap::new()),
            value_defs_in_progress: RefCell::new(std::collections::HashSet::new()),
            import_sigs: HashMap::new(),
            resource_sigs,
            resource_types: resource_names(arena, roots),
            imports_resolved: import_aliases.is_empty(),
            import_aliases,
            resolved: RefCell::new(HashMap::new()),
            sig_result_cache: RefCell::new(HashMap::new()),
            sig_in_progress: RefCell::new(std::collections::HashSet::new()),
            node_types: RefCell::new(HashMap::new()),
            speculative_node_types: RefCell::new(Vec::new()),
            export_results,
        };
        for &root in roots {
            if let Some((resource, members)) = as_defresource(arena, root) {
                let state = members
                    .iter()
                    .find(|(name, _)| name == "New")
                    .and_then(|(_, function)| {
                        let mut scope = fn_params(arena, *function)?;
                        checker
                            .infer_speculative(fn_body(arena, *function)?, &mut scope)
                            .ok()
                    })
                    .unwrap_or(Type::Unknown);
                for (name, function) in members {
                    if name == "New" || name == "Drop" {
                        continue;
                    }
                    let (is_static, function) = strip_static(arena, *function);
                    let exposed = format!("{resource}-{name}");
                    if checker
                        .resource_sigs
                        .get(&exposed)
                        .is_some_and(|(_, result)| *result != Type::Unknown)
                    {
                        continue;
                    }
                    let Some(mut scope) = fn_params(arena, function) else {
                        continue;
                    };
                    if !is_static && let Some((_, receiver)) = scope.first_mut() {
                        *receiver = state.clone()
                    }
                    let result = checker
                        .infer_speculative(fn_body(arena, function).unwrap(), &mut scope)
                        .unwrap_or(Type::Unknown);
                    if let Some((_, output)) = checker.resource_sigs.get_mut(&exposed) {
                        *output = result
                    }
                }
            }
        }
        checker
    }

    fn check_roots(&self, roots: &[NodeId]) -> Result<(), String> {
        let arena = self.arena;

        validate_variant_payloads(arena, roots)?;
        validate_defresources(arena, roots)?;
        resource_state::validate(arena, roots)?;
        local_ownership::validate(self, roots)?;

        let exported = crate::wit::export_names(arena, roots);
        for &root in roots {
            if let Some((name, expr)) = as_def(arena, root) {
                let expected = self.export_results.get(name).cloned();
                let ret = if let Some(params) = fn_params(arena, expr) {
                    let body = fn_body(arena, expr).expect("fn with params has a body");
                    let mut scope: Scope = self
                        .sigs
                        .get(name)
                        .and_then(|sigs| sigs.iter().find(|sig| sig.body == Some(body)))
                        .map(|sig| sig.params.clone())
                        .unwrap_or(params);
                    self.check(body, expected.as_ref(), &mut scope)?
                } else {
                    let mut scope: Scope = Vec::new();
                    self.check(expr, expected.as_ref(), &mut scope)?
                };

                if contains_fn(&ret)
                    && (exported.contains(name)
                        || (strict() && !matches!(arena.node(expr), Node::Sym(_))))
                {
                    return Err(format!(
                        "eval error: export `{name}`: function types cannot cross \
                         component boundaries yet; import the combinator as a macro \
                         so it expands into this component instead"
                    ));
                }
            } else {
                let mut scope: Scope = Vec::new();
                self.check(root, None, &mut scope)?;
            }
        }
        Ok(())
    }

    fn overload_names(&self) -> std::collections::HashSet<String> {
        self.sigs
            .iter()
            .filter(|(_n, v)| v.len() > 1)
            .map(|(n, _v)| n.clone())
            .collect()
    }
}

pub fn resolve_overloads(arena: Arena, roots: &[NodeId]) -> Result<(Arena, Vec<NodeId>), String> {
    resolve_overloads_excluding(arena, roots, &std::collections::HashSet::new())
}

pub fn resolve_overloads_excluding(
    arena: Arena,
    roots: &[NodeId],
    keep: &std::collections::HashSet<String>,
) -> Result<(Arena, Vec<NodeId>), String> {
    let checker = Checker::collect(&arena, roots);
    checker.check_roots(roots)?;

    let mut overloads = checker.overload_names();
    overloads.retain(|n| !keep.contains(n));
    if overloads.is_empty() {
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

fn mangled_def_name(name: &str, k: usize) -> String {
    format!("{name}${k}")
}

struct Rewriter<'a> {
    arena: &'a Arena,
    overloads: &'a std::collections::HashSet<String>,
    resolved: &'a HashMap<NodeId, usize>,
    out: Arena,

    def_counts: HashMap<String, usize>,
}

impl<'a> Rewriter<'a> {
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

            let head = self.rewrite(items[0]);
            let new_name = self.out.add(Node::Sym(unique), self.arena.span(items[1]));
            let expr = self.rewrite(items[2]);
            return self.out.add(Node::Tup(vec![head, new_name, expr]), span);
        }
        self.rewrite(id)
    }

    fn rewrite(&mut self, id: NodeId) -> NodeId {
        let span = self.arena.span(id);
        match self.arena.node(id).clone() {
            Node::Tup(items) => {
                if let Some(&chosen) = self.resolved.get(&id)
                    && let Some(&head) = items.first()
                    && let Node::Sym(name) = self.arena.node(head)
                    && self.overloads.contains(name)
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

fn as_export_result(arena: &Arena, id: NodeId) -> Option<(String, Type)> {
    let Node::Tup(items) = arena.node(id) else {
        return None;
    };
    let [head, rec] = items.as_slice() else {
        return None;
    };
    let Node::Sym(h) = arena.node(*head) else {
        return None;
    };
    if h != "export-MACRO" {
        return None;
    }
    let Node::Rec(fields) = arena.node(*rec) else {
        return None;
    };
    let mut name: Option<String> = None;
    let mut result: Option<Type> = None;
    for (k, v) in fields {
        match k.as_str() {
            "name" => {
                if let Node::Sym(n) = arena.node(*v) {
                    name = Some(n.clone());
                }
            }
            "result" => result = Some(Type::from_form(arena, *v)),
            _ => {}
        }
    }
    Some((name?, result?))
}

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
                        let tys = payload.iter().map(|&t| Type::from_form(arena, t)).collect();
                        out.push((case.clone(), tys));
                    }
                    _ => return None,
                }
            }
            TypeDef::Variant(out)
        }
        Node::Flg(names) => TypeDef::Flags(names.clone()),

        _ => TypeDef::Alias(Type::from_form(arena, *decl)),
    };
    Some((name, def))
}

fn as_defresource(arena: &Arena, id: NodeId) -> Option<(&str, &[(String, NodeId)])> {
    let Node::Tup(items) = arena.node(id) else {
        return None;
    };
    let [head, name_id, members] = items.as_slice() else {
        return None;
    };
    let Node::Sym(h) = arena.node(*head) else {
        return None;
    };
    if h != "defresource-MACRO" {
        return None;
    }
    let Node::Sym(name) = arena.node(*name_id) else {
        return None;
    };
    let Node::Rec(fields) = arena.node(*members) else {
        return None;
    };
    Some((name, fields.as_slice()))
}

fn strip_static(arena: &Arena, val: NodeId) -> (bool, NodeId) {
    if let Node::Tup(items) = arena.node(val)
        && items.len() == 2
        && matches!(arena.node(items[0]), Node::Sym(s) if s == "static-MACRO")
    {
        return (true, items[1]);
    }
    (false, val)
}

fn resource_names(arena: &Arena, roots: &[NodeId]) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for &root in roots {
        if let Some((name, _)) = as_defresource(arena, root) {
            out.insert(name.to_string());
        }
    }
    out
}

fn first_param_is_self(params: &[(String, Type)], rname: &str) -> bool {
    matches!(params.first(), Some((n, t)) if n == "self" && *t == Type::Named(rname.to_string()))
}

fn declares_self(params: &[(String, Type)]) -> bool {
    matches!(params.first(), Some((n, _)) if n == "self")
}

fn validate_variant_payloads(arena: &Arena, roots: &[NodeId]) -> Result<(), String> {
    for &root in roots {
        if let Some((name, TypeDef::Variant(cases))) = as_deftype(arena, root) {
            for (case, payload) in cases {
                if payload.len() > 1 {
                    return Err(format!(
                        "variant case `{name}/{case}` takes at most one payload type; wrap several types in tuple(...)"
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_defresources(arena: &Arena, roots: &[NodeId]) -> Result<(), String> {
    let resources = resource_names(arena, roots);
    for &root in roots {
        if let Some((rname, members)) = as_defresource(arena, root) {
            for (key, val) in members {
                let (is_static, fn_id) = strip_static(arena, *val);
                match key.as_str() {
                    "New" => {
                        if is_static {
                            return Err(format!(
                                "DefResource `{rname}`: the constructor `New` cannot be `Static`"
                            ));
                        }
                        if fn_params(arena, fn_id).is_none() {
                            return Err(format!("DefResource `{rname}`: `New` must be an `Fn`"));
                        }
                    }
                    "Drop" => {
                        if is_static {
                            return Err(format!(
                                "DefResource `{rname}`: the destructor `Drop` cannot be `Static`"
                            ));
                        }
                        let params = fn_params(arena, fn_id).ok_or(format!(
                            "DefResource `{rname}`: `Drop` must be an `Fn {{self: {rname}}}`"
                        ))?;
                        if !first_param_is_self(&params, rname) {
                            return Err(format!(
                                "DefResource `{rname}`: `Drop` must take `self: {rname}`                                  as its only parameter"
                            ));
                        }
                    }
                    other => {
                        let params = fn_params(arena, fn_id).ok_or(format!(
                            "DefResource `{rname}`: member `{other}` must be an `Fn`"
                        ))?;
                        if is_static {
                            if declares_self(&params) {
                                return Err(format!(
                                    "DefResource `{rname}`: static member `{other}`                                      must not take `self`"
                                ));
                            }
                        } else if !first_param_is_self(&params, rname) {
                            return Err(format!(
                                "DefResource `{rname}`: member `{other}` must take                                  `self: {rname}` as its first parameter or be marked `Static`"
                            ));
                        }
                    }
                }
            }
        }

        check_no_borrow_in_stored(arena, root, &resources)?;
    }
    Ok(())
}

fn check_no_borrow_in_stored(
    arena: &Arena,
    root: NodeId,
    resources: &std::collections::HashSet<String>,
) -> Result<(), String> {
    let Node::Tup(items) = arena.node(root) else {
        return Ok(());
    };
    let Some(&head) = items.first() else {
        return Ok(());
    };
    let Node::Sym(h) = arena.node(head) else {
        return Ok(());
    };
    match h.as_str() {
        "export-MACRO" => {
            if let Some(&p) = items.get(1)
                && let Node::Rec(fields) = arena.node(p)
            {
                for (k, v) in fields {
                    if k == "result" && form_borrows_resource(arena, *v, resources) {
                        return Err(
                            "a `borrow(<resource>)` type cannot appear in result position                              (return `own` — the bare resource name — instead)"
                                .to_string(),
                        );
                    }
                }
            }
        }
        "deftype-MACRO" => {
            if let Some(&decl) = items.get(2)
                && form_borrows_resource(arena, decl, resources)
            {
                return Err(
                    "a `borrow(<resource>)` type cannot be stored in a `DefType`                      (borrow is a parameter-only, boundary-only type)"
                        .to_string(),
                );
            }
        }
        _ => {}
    }
    Ok(())
}

fn form_borrows_resource(
    arena: &Arena,
    id: NodeId,
    resources: &std::collections::HashSet<String>,
) -> bool {
    match arena.node(id) {
        Node::Tup(items) => {
            if let [head, arg] = items.as_slice()
                && matches!(arena.node(*head), Node::Sym(s) if s == "borrow")
                && matches!(arena.node(*arg), Node::Sym(a) if resources.contains(a))
            {
                return true;
            }
            items
                .iter()
                .any(|&c| form_borrows_resource(arena, c, resources))
        }
        Node::Lst(items) => items
            .iter()
            .any(|&c| form_borrows_resource(arena, c, resources)),
        Node::Rec(fields) => fields
            .iter()
            .any(|(_, v)| form_borrows_resource(arena, *v, resources)),
        _ => false,
    }
}

fn fn_params(arena: &Arena, id: NodeId) -> Option<Vec<(String, Type)>> {
    let (params_id, _body) = as_fn(arena, id)?;
    Some(parse_params(arena, params_id))
}

fn fn_body(arena: &Arena, id: NodeId) -> Option<NodeId> {
    as_fn(arena, id).map(|(_p, body)| body)
}

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

pub(crate) fn parse_params(arena: &Arena, id: NodeId) -> Vec<(String, Type)> {
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
    fn check(
        &self,
        id: NodeId,
        expected: Option<&Type>,
        scope: &mut Scope,
    ) -> Result<Type, String> {
        let ty = self.infer(id, expected, scope)?;
        if let Some(exp) = expected
            && !compatible_widening(&self.types, exp, &ty)
        {
            return Err(self.type_error(id, exp, &ty));
        }

        if matches!(ty, Type::Unknown)
            && strict()
            && self.imports_resolved
            && self.speculative_node_types.borrow().is_empty()
            && !self.strict_exempt(id)
        {
            return Err(format!(
                "eval error: cannot infer a static type for `{}`; annotate its parameters or result",
                crate::printer::print(self.arena, id)
            ));
        }
        if let Some(table) = self.speculative_node_types.borrow_mut().last_mut() {
            table.insert(id, ty.clone());
        } else {
            self.node_types.borrow_mut().insert(id, ty.clone());
        }
        Ok(ty)
    }

    fn infer_speculative(&self, id: NodeId, scope: &mut Scope) -> Result<Type, String> {
        struct ShadowTable<'a>(&'a RefCell<Vec<NodeTypes>>);
        impl Drop for ShadowTable<'_> {
            fn drop(&mut self) {
                self.0.borrow_mut().pop();
            }
        }
        self.speculative_node_types
            .borrow_mut()
            .push(HashMap::new());
        let _shadow = ShadowTable(&self.speculative_node_types);
        self.infer(id, None, scope)
    }

    fn value_def_type(&self, name: &str, expr: NodeId) -> Type {
        if let Some(t) = self.value_def_types.borrow().get(name) {
            return t.clone();
        }
        if !self
            .value_defs_in_progress
            .borrow_mut()
            .insert(name.to_string())
        {
            return Type::Unknown;
        }
        let mut scope: Scope = Vec::new();
        let t = self
            .infer_speculative(expr, &mut scope)
            .unwrap_or(Type::Unknown);
        self.value_defs_in_progress.borrow_mut().remove(name);
        self.value_def_types
            .borrow_mut()
            .insert(name.to_string(), t.clone());
        t
    }

    fn strict_exempt(&self, id: NodeId) -> bool {
        let Node::Tup(items) = self.arena.node(id) else {
            return false;
        };
        let Some(&head) = items.first() else {
            return false;
        };
        let Node::Sym(h) = self.arena.node(head) else {
            return false;
        };
        matches!(
            h.as_str(),
            "package-MACRO"
                | "import-MACRO"
                | "instantiate-MACRO"
                | "export-MACRO"
                | "deftype-MACRO"
        ) || (h.ends_with("-MACRO")
            && !matches!(
                h.as_str(),
                "fn-MACRO"
                    | "let-MACRO"
                    | "match-MACRO"
                    | "the-MACRO"
                    | "quote-MACRO"
                    | "quasi-MACRO"
                    | "def-MACRO"
                    | "defmacro-MACRO"
            ))
    }

    fn type_error(&self, _id: NodeId, expected: &Type, actual: &Type) -> String {
        format!("eval error: type mismatch: expected {expected:?}, got {actual:?}")
    }

    fn flags_members<'t>(&'t self, t: &'t Type) -> Option<&'t Vec<String>> {
        match t {
            Type::Flags(ms) => Some(ms),
            Type::Named(n) => match self.types.get(n) {
                Some(TypeDef::Flags(decl)) => Some(decl),
                _ => None,
            },
            _ => None,
        }
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
            Node::Sym(name) => {
                if !self.defs.contains(name)
                    && !self.variant_cases.contains_key(name)
                    && let Some(Type::Fn(params, _)) = expected
                    && crate::builtins::tuple_ctor_arity(name)
                        .is_some_and(|arity| arity == params.len())
                {
                    return Ok(Type::Fn(
                        params.clone(),
                        Box::new(Type::Tuple(params.clone())),
                    ));
                }
                self.infer_name(name, scope)
            }

            Node::Lst(items) => self.infer_list(items, expected, scope),

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
                    let exp = exp_fields
                        .as_ref()
                        .and_then(|fs| fs.iter().find(|(n, _)| n == k).map(|(_, t)| t.clone()));
                    let t = self.check(*v, exp.as_ref(), scope)?;
                    if strict() && contains_fn(&t) {
                        return Err("eval error: runtime records cannot store function values; use statically resolved functions or macros".into());
                    }
                    out.push((k.clone(), t));
                }
                Ok(Type::Record(out))
            }

            Node::Flg(names) => {
                if let Some(decl) = expected.and_then(|e| self.flags_members(e))
                    && names.iter().all(|m| decl.contains(m))
                {
                    Ok(Type::Flags(decl.clone()))
                } else {
                    Ok(Type::Flags(names.clone()))
                }
            }
            Node::Tup(items) => self.infer_tup(id, items, expected, scope),
        }
    }

    fn infer_name(&self, name: &str, scope: &Scope) -> Result<Type, String> {
        if let Some((_, t)) = scope.iter().rev().find(|(n, _)| n == name) {
            return Ok(t.clone());
        }
        if self.defs.contains(name) {
            if let Some(&expr) = self.value_defs.get(name) {
                return Ok(self.value_def_type(name, expr));
            }

            if let Some(sigs) = self.sigs.get(name)
                && sigs.len() == 1
                && let Some(arrow) = self.arrow_of_sig(&sigs[0])
            {
                return Ok(arrow);
            }
            return Ok(Type::Unknown);
        }
        if name == "none" {
            return Ok(Type::Option(Box::new(Type::Unknown)));
        }
        if name == "pi" {
            return Ok(Type::F64);
        }

        if let Some((tyname, payload)) = self.variant_cases.get(name) {
            if payload.is_empty() {
                return Ok(Type::Named(tyname.clone()));
            }
            return Ok(Type::Fn(
                payload.clone(),
                Box::new(Type::Named(tyname.clone())),
            ));
        }
        if is_builtin(name) {
            return Ok(Type::Unknown);
        }
        if let Some((params, result)) = self.import_sigs.get(name) {
            return Ok(Type::Fn(
                params.iter().map(|(_, ty)| ty.clone()).collect(),
                Box::new(result.clone()),
            ));
        }
        if self
            .import_aliases
            .iter()
            .any(|alias| name.starts_with(&format!("{alias}-")))
        {
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
            } else if let Some(u) = unify(&self.types, &elem, &t) {
                elem = u;
            } else {
                return Err(format!(
                    "eval error: heterogeneous list: elements must share one \
                     type, got {elem:?} and {t:?}"
                ));
            }
        }
        if strict() && contains_fn(&elem) {
            return Err("eval error: runtime lists cannot store function values; use statically resolved functions or macros".into());
        }
        Ok(Type::List(Box::new(elem)))
    }

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

            if let Some((_, Type::Fn(params, ret))) = scope.iter().rev().find(|(n, _)| n == h) {
                let params = params.clone();
                let ret = (**ret).clone();
                return self.check_indirect_apply(&params, ret, args, scope);
            }
            if scope.iter().any(|(name, _)| name == h) {
                for &argument in args {
                    self.check(argument, None, scope)?;
                }
                return Ok(Type::Unknown);
            }
            if let Some((params, result)) = self.import_sigs.get(h).cloned() {
                return self.check_call_shape(h, &params, result, args, scope);
            }

            return self.infer_call(id, h, args, expected, scope);
        }

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
                if let [params_id, body] = args {
                    let mut params = parse_params(self.arena, *params_id);
                    let result_expected = if let Some(Type::Fn(inputs, result)) = expected {
                        if inputs.len() != params.len() {
                            return Err(
                                "eval error: callback has the wrong number of parameters".into()
                            );
                        }
                        for ((_, ty), input) in params.iter_mut().zip(inputs) {
                            if matches!(ty, Type::Unknown) {
                                *ty = input.clone();
                            }
                        }
                        Some(result.as_ref())
                    } else {
                        None
                    };
                    let mark = scope.len();
                    for (n, t) in &params {
                        scope.push((n.clone(), t.clone()));
                    }
                    let ret = self.check(*body, result_expected, scope);
                    scope.truncate(mark);
                    let ret = ret?;
                    let ptys: Vec<Type> = params.iter().map(|(_, t)| t.clone()).collect();
                    if ptys.iter().all(fully_concrete) && fully_concrete(&ret) {
                        return Ok(Type::Fn(ptys, Box::new(ret)));
                    }
                }
                Ok(Type::Unknown)
            }
            "let-MACRO" => {
                let [bindings, body] = expect2(args)?;
                let mark = scope.len();
                if let Node::Rec(fields) = self.arena.node(bindings) {
                    for (k, v) in fields {
                        let t = self.check(*v, None, scope)?;
                        if strict()
                            && contains_fn(&t)
                            && !matches!(self.arena.node(*v), Node::Sym(_))
                            && as_fn(self.arena, *v).is_none()
                        {
                            return Err("eval error: runtime function selection is unsupported; bind a static function or choose a variant tag".into());
                        }
                        scope.push((k.clone(), t));
                    }
                }
                let r = self.check(body, expected, scope);
                scope.truncate(mark);
                r
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

                    self.reject_float_patterns(pair[0])?;
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

                let pats: Vec<NodeId> = items
                    .iter()
                    .filter_map(|&clause| match self.arena.node(clause) {
                        Node::Tup(pair) if pair.len() == 2 => Some(pair[0]),
                        _ => None,
                    })
                    .collect();
                self.check_exhaustive(&scrut_ty, &pats)?;
                Ok(result.unwrap_or(Type::Unknown))
            }
            "the-MACRO" => {
                let [ty_form, expr] = expect2(args)?;
                let ty = Type::from_form(self.arena, ty_form);
                self.check_the(ty_form, &ty, expr, scope)
            }

            "quote-MACRO" => Ok(Type::Tree),

            "quasi-MACRO" => {
                if let [form] = args {
                    self.check_quasi(*form, scope, 1)?;
                }
                Ok(Type::Tree)
            }

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

            "package-MACRO" | "import-MACRO" | "instantiate-MACRO" | "export-MACRO"
            | "deftype-MACRO" => Ok(Type::Unknown),

            _ => {
                let _ = args;
                Ok(Type::Unknown)
            }
        }
    }

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
                let rt = self.resolve_alias(ty);
                let numeric_or_gradual = rt.is_int()
                    || rt.is_float()
                    || matches!(
                        rt,
                        Type::IntLit(_)
                            | Type::FloatLit
                            | Type::Unknown
                            | Type::Named(_)
                            | Type::Tree
                    );
                if !numeric_or_gradual || !int_in_range(*n, ty) {
                    return Err(format!(
                        "eval error: The: {n} does not conform to type `{ty_text}`"
                    ));
                }
                Ok(ty.clone())
            }
            Node::Dec(_) => {
                let rt = self.resolve_alias(ty);
                let ok = rt.is_float()
                    || matches!(
                        rt,
                        Type::FloatLit | Type::Unknown | Type::Named(_) | Type::Tree
                    );
                if !ok {
                    return Err(format!(
                        "eval error: The: {} does not conform to type `{ty_text}`",
                        print_dec(self.arena, expr)
                    ));
                }
                Ok(ty.clone())
            }
            _ => {
                self.check(expr, Some(ty), scope)?;
                Ok(ty.clone())
            }
        }
    }

    fn infer_call(
        &self,
        id: NodeId,
        name: &str,
        args: &[NodeId],
        expected: Option<&Type>,
        scope: &mut Scope,
    ) -> Result<Type, String> {
        if let Some(sigs) = self.sigs.get(name) {
            if sigs.len() > 1 {
                return self.resolve_overload(id, name, sigs, args, expected, scope);
            }

            return self.check_def_call(name, &sigs[0], args, scope);
        }
        if let Some((params, result)) = self.resource_sigs.get(name) {
            return self.check_call_shape(name, params, result.clone(), args, scope);
        }
        if let Some(&expr) = self.value_defs.get(name)
            && let Type::Fn(params, result) = self.value_def_type(name, expr)
        {
            return self.check_indirect_apply(&params, *result, args, scope);
        }

        if let Some((tyname, payload)) = self.variant_cases.get(name).cloned() {
            return self.check_variant_ctor(name, &tyname, &payload, args, scope);
        }
        if !is_builtin(name)
            && !self
                .import_aliases
                .iter()
                .any(|alias| name.starts_with(&format!("{alias}-")))
        {
            return Err(format!(
                "eval error: unbound name `{name}` in call position"
            ));
        }
        self.check_builtin_call(name, args, scope)
    }

    fn check_variant_ctor(
        &self,
        case: &str,
        tyname: &str,
        payload: &[Type],
        args: &[NodeId],
        scope: &mut Scope,
    ) -> Result<Type, String> {
        if payload.is_empty() {
            return Err(format!(
                "eval error: variant case `{case}` of `{tyname}` takes no payload \
                 and is not callable; write the bare case name"
            ));
        }
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

    fn resolve_overload(
        &self,
        id: NodeId,
        name: &str,
        sigs: &[Sig],
        args: &[NodeId],
        expected: Option<&Type>,
        scope: &mut Scope,
    ) -> Result<Type, String> {
        let arg_tys: Vec<Type> = args
            .iter()
            .map(|&a| self.check(a, None, scope))
            .collect::<Result<_, _>>()?;

        let mut candidates: Vec<usize> = (0..sigs.len())
            .filter(|&i| args_match(&self.types, &sigs[i].params, &arg_tys))
            .collect();

        if candidates.len() > 1
            && let Some(exp) = expected
        {
            let by_result: Vec<usize> = candidates
                .iter()
                .copied()
                .filter(|&i| {
                    compatible_widening(&self.types, exp, &self.infer_sig_result(&sigs[i]))
                })
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

    fn arrow_of_sig(&self, sig: &Sig) -> Option<Type> {
        let ptys: Vec<Type> = sig.params.iter().map(|(_, t)| t.clone()).collect();
        if !ptys.iter().all(fully_concrete) {
            return None;
        }
        let ret = self.infer_sig_result(sig);
        if !fully_concrete(&ret) {
            return None;
        }
        Some(Type::Fn(ptys, Box::new(ret)))
    }

    fn infer_sig_result(&self, sig: &Sig) -> Type {
        let Some(body) = sig.body else {
            return Type::Unknown;
        };

        if let Some(cached) = self.sig_result_cache.borrow().get(&body) {
            return cached.clone();
        }

        if !self.sig_in_progress.borrow_mut().insert(body) {
            return Type::Unknown;
        }
        let mut scope: Scope = sig.params.clone();

        let result = self
            .infer_speculative(body, &mut scope)
            .unwrap_or(Type::Unknown);
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
        let result = self.infer_sig_result(sig);
        self.check_call_shape(name, &sig.params, result, args, scope)
    }

    fn check_indirect_apply(
        &self,
        params: &[Type],
        ret: Type,
        args: &[NodeId],
        scope: &mut Scope,
    ) -> Result<Type, String> {
        let arg_tys: Vec<Type> = args
            .iter()
            .map(|&a| self.check(a, None, scope))
            .collect::<Result<_, _>>()?;
        if arg_tys.len() != params.len() {
            return Err(format!(
                "eval error: this function takes {} argument(s), got {}",
                params.len(),
                arg_tys.len()
            ));
        }
        for (pt, at) in params.iter().zip(&arg_tys) {
            if !compatible_widening(&self.types, pt, at) {
                return Err(format!(
                    "eval error: type mismatch: expected {pt:?}, got {at:?}"
                ));
            }
        }
        Ok(ret)
    }

    fn check_call_shape(
        &self,
        name: &str,
        sig_params: &[(String, Type)],
        result: Type,
        args: &[NodeId],
        scope: &mut Scope,
    ) -> Result<Type, String> {
        let arg_tys: Vec<Type> = args
            .iter()
            .map(|&a| self.check(a, None, scope))
            .collect::<Result<_, _>>()?;

        let nparams = sig_params.len();

        if args.len() == 1 {
            if let Type::Record(fs) = &arg_tys[0] {
                let mut fnames: Vec<&str> = fs.iter().map(|(n, _)| n.as_str()).collect();
                let mut pnames: Vec<&str> = sig_params.iter().map(|(n, _)| n.as_str()).collect();
                fnames.sort_unstable();
                pnames.sort_unstable();
                if fnames == pnames {
                    for (fname, ft) in fs {
                        let (_pn, pt) = sig_params
                            .iter()
                            .find(|(pn, _)| pn == fname)
                            .expect("field names equal param names");
                        if !self.param_compatible(pt, ft) {
                            return Err(self.param_type_error(fname, pt, ft, name));
                        }
                    }
                    return Ok(result);
                }
                if nparams != 1 {
                    return Ok(result);
                }
            }

            if nparams == 1 {
                if !self.param_compatible(&sig_params[0].1, &arg_tys[0]) {
                    return Err(self.param_type_error(
                        &sig_params[0].0,
                        &sig_params[0].1,
                        &arg_tys[0],
                        name,
                    ));
                }
                return Ok(result);
            }
        }

        if nparams == 1 && args.len() > 1 {
            let bundle = Type::Tuple(arg_tys.clone());
            if !self.param_compatible(&sig_params[0].1, &bundle) {
                return Err(self.param_type_error(
                    &sig_params[0].0,
                    &sig_params[0].1,
                    &bundle,
                    name,
                ));
            }
            return Ok(result);
        }

        if args.len() != nparams {
            return Err(format!(
                "eval error: `{name}` expects {nparams} arguments, got {}",
                args.len()
            ));
        }
        for ((pn, pt), at) in sig_params.iter().zip(&arg_tys) {
            if !self.param_compatible(pt, at) {
                return Err(self.param_type_error(pn, pt, at, name));
            }
        }
        Ok(result)
    }

    fn param_compatible(&self, pt: &Type, at: &Type) -> bool {
        if compatible(&self.types, pt, at) {
            return true;
        }
        matches!(
            (self.resolve_alias(pt), self.resolve_alias(at)),
            (Type::List(e), Type::String) if *e == Type::U8
        )
    }

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

    fn bind_pattern(&self, pat: NodeId, ty: &Type, scope: &mut Scope) {
        let resolved = self.resolve_alias(ty);
        let ty = &resolved;
        match self.arena.node(pat) {
            Node::Sym(name) => {
                if self.sym_matches_nullary_case(name, ty) {
                    return;
                }
                scope.push((name.clone(), ty.clone()));
            }
            Node::Tup(items) => {
                if let Some((&h, rest)) = items.split_first()
                    && let Node::Sym(case) = self.arena.node(h)
                    && let Some(payload) = self.case_payload(case, ty)
                {
                    let payload = if rest.len() > 1 && payload.len() == 1 {
                        match self.resolve_alias(&payload[0]) {
                            Type::Tuple(elements) if elements.len() == rest.len() => elements,
                            _ => payload,
                        }
                    } else {
                        payload
                    };
                    if rest.len() == payload.len() {
                        for (&p, t) in rest.iter().zip(&payload) {
                            self.bind_pattern(p, t, scope);
                        }
                    } else if rest.len() == 1 && payload.len() > 1 {
                        self.bind_pattern(rest[0], &Type::Tuple(payload), scope);
                    } else {
                        for &p in rest {
                            self.bind_pattern(p, &Type::Unknown, scope);
                        }
                    }
                    return;
                }

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

            _ => {}
        }
    }

    fn reject_float_patterns(&self, pat: NodeId) -> Result<(), String> {
        match self.arena.node(pat) {
            Node::Dec(_) => Err("eval error: a float literal cannot be a Match \
                 pattern (float equality is unreliable); use an explicit \
                 comparison such as `If eq(x 0.5) …` instead"
                .to_string()),
            Node::Tup(items) | Node::Lst(items) => {
                for &it in items {
                    self.reject_float_patterns(it)?;
                }
                Ok(())
            }
            Node::Rec(fields) => {
                for (_, v) in fields {
                    self.reject_float_patterns(*v)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn check_exhaustive(&self, ty: &Type, pats: &[NodeId]) -> Result<(), String> {
        if pats.iter().any(|&p| self.irrefutable(p, ty)) {
            return Ok(());
        }
        let ty = self.resolve_alias(ty);
        match &ty {
            Type::Bool => {
                let mut have = [false, false];
                for &p in pats {
                    if let Node::Bool(b) = self.arena.node(p) {
                        have[*b as usize] = true;
                    }
                }
                if have[0] && have[1] {
                    Ok(())
                } else {
                    Err(format!(
                        "eval error: non-exhaustive Match: missing case `{}`",
                        if have[1] { "false" } else { "true" }
                    ))
                }
            }
            Type::Option(t) => self.check_cases_covered(
                &[
                    ("some".to_string(), vec![(**t).clone()]),
                    ("none".to_string(), vec![]),
                ],
                pats,
            ),
            Type::Result(o, e) => {
                let arm = |t: &Type| match t {
                    Type::Unit => Vec::new(),
                    t => vec![t.clone()],
                };
                self.check_cases_covered(
                    &[("ok".to_string(), arm(o)), ("err".to_string(), arm(e))],
                    pats,
                )
            }
            Type::Named(n) => match self.types.get(n) {
                Some(TypeDef::Variant(cases)) => {
                    let cases = cases.clone();
                    self.check_cases_covered(&cases, pats)
                }

                Some(TypeDef::Record(fields)) if self.record_patterns_cover(fields, pats) => Ok(()),
                Some(TypeDef::Record(_)) => Err(non_exhaustive_catch_all()),

                _ => Ok(()),
            },

            Type::Record(fields) if self.record_patterns_cover(fields, pats) => Ok(()),
            Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::S8
            | Type::S16
            | Type::S32
            | Type::S64
            | Type::F32
            | Type::F64
            | Type::IntLit(_)
            | Type::FloatLit
            | Type::Char
            | Type::String
            | Type::List(_)
            | Type::Tuple(_)
            | Type::Record(_)
            | Type::Flags(_)
            | Type::Fn(..)
            | Type::Cell(_)
            | Type::Tree => Err(non_exhaustive_catch_all()),

            Type::Unit | Type::Unknown => Ok(()),
        }
    }

    fn record_patterns_cover(&self, fields: &[(String, Type)], patterns: &[NodeId]) -> bool {
        for (field, ty) in fields {
            let mut choices = Vec::new();
            for &pattern in patterns {
                let Node::Rec(parts) = self.arena.node(pattern) else {
                    continue;
                };
                if !parts.iter().all(|(name, pattern)| {
                    fields
                        .iter()
                        .find(|(declared, _)| declared == name)
                        .is_some_and(|(_, ty)| name == field || self.irrefutable(*pattern, ty))
                }) {
                    continue;
                }
                if let Some((_, pattern)) = parts.iter().find(|(name, _)| name == field) {
                    choices.push(*pattern);
                } else {
                    return true;
                }
            }
            if !choices.is_empty() && self.check_exhaustive(ty, &choices).is_ok() {
                return true;
            }
        }
        false
    }

    fn check_cases_covered(
        &self,
        cases: &[(String, Vec<Type>)],
        pats: &[NodeId],
    ) -> Result<(), String> {
        let mut missing = Vec::new();
        for (case, payload) in cases {
            let covered = pats.iter().any(|&p| self.covers_case(p, case, payload));
            if !covered {
                missing.push(format!("`{case}`"));
            }
        }
        if missing.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "eval error: non-exhaustive Match: missing case{} {}",
                if missing.len() == 1 { "" } else { "s" },
                missing.join(", ")
            ))
        }
    }

    fn covers_case(&self, p: NodeId, case: &str, payload: &[Type]) -> bool {
        match self.arena.node(p) {
            Node::Sym(s) => s == case && payload.is_empty(),
            Node::Tup(items) => {
                let Some((&h, rest)) = items.split_first() else {
                    return false;
                };
                if !matches!(self.arena.node(h), Node::Sym(s) if s == case) {
                    return false;
                }
                let expanded;
                let payload = if rest.len() > 1 && payload.len() == 1 {
                    expanded = match self.resolve_alias(&payload[0]) {
                        Type::Tuple(elements) if elements.len() == rest.len() => elements,
                        _ => payload.to_vec(),
                    };
                    expanded.as_slice()
                } else {
                    payload
                };
                if rest.len() == payload.len() {
                    rest.iter()
                        .zip(payload)
                        .all(|(&sub, t)| self.irrefutable(sub, t))
                } else if rest.is_empty()
                    && payload.len() == 1
                    && matches!(payload[0], Type::Unknown)
                {
                    true
                } else if rest.len() == 1 && payload.len() > 1 {
                    self.irrefutable(rest[0], &Type::Tuple(payload.to_vec()))
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn irrefutable(&self, p: NodeId, ty: &Type) -> bool {
        match self.arena.node(p) {
            Node::Sym(name) => !self.sym_matches_nullary_case(name, ty),
            Node::Rec(fields) => {
                let ty = self.resolve_alias(ty);
                let rec_fields: Option<Vec<(String, Type)>> = match &ty {
                    Type::Record(fs) => Some(fs.clone()),
                    Type::Named(n) => match self.types.get(n) {
                        Some(TypeDef::Record(fs)) => Some(fs.clone()),
                        _ => None,
                    },
                    _ => None,
                };
                let Some(rec_fields) = rec_fields else {
                    return false;
                };
                fields.iter().all(|(k, v)| {
                    rec_fields
                        .iter()
                        .find(|(n, _)| n == k)
                        .is_some_and(|(_, t)| self.irrefutable(*v, t))
                })
            }
            Node::Tup(items) => {
                let ty = self.resolve_alias(ty);
                match &ty {
                    Type::Tuple(ts) if ts.len() == items.len() => items
                        .iter()
                        .zip(ts)
                        .all(|(&sub, t)| self.irrefutable(sub, t)),
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn resolve_alias(&self, ty: &Type) -> Type {
        let mut ty = ty.clone();
        for _ in 0..8 {
            let Type::Named(n) = &ty else { break };
            let Some(TypeDef::Alias(target)) = self.types.get(n) else {
                break;
            };
            ty = target.clone();
        }
        ty
    }

    fn case_payload(&self, case: &str, ty: &Type) -> Option<Vec<Type>> {
        let resolved = self.resolve_alias(ty);
        match &resolved {
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
                "ok" => Some(match &**o {
                    Type::Unit => Vec::new(),
                    t => vec![t.clone()],
                }),
                "err" => Some(match &**e {
                    Type::Unit => Vec::new(),
                    t => vec![t.clone()],
                }),
                _ => None,
            },
            _ => None,
        }
    }

    fn sym_matches_nullary_case(&self, name: &str, ty: &Type) -> bool {
        match self.resolve_alias(ty) {
            Type::Option(_) => name == "none",
            Type::Named(n) => matches!(
                self.types.get(&n),
                Some(TypeDef::Variant(cases))
                    if cases.iter().any(|(c, p)| c == name && p.is_empty())
            ),
            _ => false,
        }
    }
}

fn elem_type(t: Option<&Type>) -> Type {
    match t {
        Some(Type::List(e)) => (**e).clone(),
        _ => Type::Unknown,
    }
}

fn ctor_payload(arg_tys: &[Type]) -> Type {
    match arg_tys {
        [] => Type::Unknown,
        [one] => one.clone(),
        many => Type::Tuple(many.to_vec()),
    }
}

fn result_ctor_payload(arg_tys: &[Type]) -> Type {
    match arg_tys {
        [] => Type::Unit,
        _ => ctor_payload(arg_tys),
    }
}

fn non_exhaustive_catch_all() -> String {
    "eval error: non-exhaustive Match: no clause matches every possible value \
     (add a catch-all clause)"
        .to_string()
}

fn comparable(t: &Type) -> bool {
    t.numeric() || matches!(t, Type::String | Type::Char)
}

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

        _ => return true,
    };

    crate::value::int_fits(name, n).unwrap_or(true)
}

fn type_name(arena: &Arena, id: NodeId) -> String {
    match arena.node(id) {
        Node::Sym(s) => s.clone(),
        _ => crate::printer::print(arena, id),
    }
}

fn print_dec(arena: &Arena, id: NodeId) -> String {
    if let Node::Dec(f) = arena.node(id) {
        crate::value::print_value(&crate::value::Value::Dec(*f))
    } else {
        crate::printer::print(arena, id)
    }
}

fn is_builtin(name: &str) -> bool {
    name == "none" || name == "pi" || crate::builtins::NAMES.contains(&name)
}

fn expect2(args: &[NodeId]) -> Result<[NodeId; 2], String> {
    match args {
        [a, b] => Ok([*a, *b]),
        _ => Err("eval error: malformed form".to_string()),
    }
}

#[cfg(test)]
mod speculative_type_tests {
    use super::*;

    #[test]
    fn candidate_inference_never_publishes_backend_types() {
        let (arena, roots) = crate::reader::read_file(
            "Def first Fn {n: s64} add(n 1)\nDef second Fn {n: s64} first(n)",
        )
        .unwrap();
        let checker = Checker::collect(&arena, &roots);
        assert_eq!(
            checker.infer_sig_result(&checker.sigs["second"][0]),
            Type::S64
        );
        assert!(checker.node_types.borrow().is_empty());
        assert!(checker.speculative_node_types.borrow().is_empty());
        checker.check_roots(&roots).unwrap();
        assert!(!checker.node_types.borrow().is_empty());
    }
}
