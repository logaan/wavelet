use std::collections::HashMap;

use wasm_encoder::{BlockType, Function, Instruction as I, MemArg, ValType};

use crate::form::{Arena, Node, NodeId};
use crate::wit::{FileInfo, FuncSig, type_decl};

mod boxes;
mod builtins;
mod call;
mod canonical;
mod component;
mod core_module;
mod deps;
mod helpers;
mod macro_component;
mod mem;
mod pattern;
mod reachability;
mod resources;
mod scalar_numeric;
mod scan;
mod serialization;
mod type_scope;
mod wit_synth;
mod witty;

pub use component::emit_macro_component_with_optimizer;
pub(crate) use deps::import_signatures;
pub use deps::{dep_non_record_types, dep_record_types};
pub use macro_component::emit_macro_component;
pub use resources::ResourceFns;
pub(crate) use type_scope::scope_dependency_types;
pub use wit_synth::dep_package_wit;
pub use witty::TypeDef;

use builtins::*;
use core_module::*;
use deps::*;
use helpers::*;
use resources::*;
use scan::*;
use serialization::serialize_module;
use wit_synth::*;
use witty::*;

pub struct Dep {
    pub package: String,
    pub funcs: Vec<FuncSig>,

    pub package_wit: String,

    pub types: Vec<(String, Vec<(String, String)>)>,

    pub type_defs: Vec<(String, TypeDef)>,

    pub aliases: Vec<(String, String)>,

    pub type_ifaces: Vec<(String, String, String)>,
}

const SCRATCH: i32 = 0;
const DATA_BASE: u32 = 16;
const TAG_BOOL: i32 = 0;
const TAG_INT: i32 = 1;
const TAG_STR: i32 = 2;
const TAG_LIST: i32 = 3;
const TAG_DEC: i32 = 4;
const TAG_FN: i32 = 5;
const TAG_REC: i32 = 6;
const TAG_VAR: i32 = 7;
const TAG_TUP: i32 = 8;
const TAG_FLG: i32 = 9;
const TAG_CHAR: i32 = 10;
const TAG_CELL: i32 = 11;

const PERSIST_RESERVE: u32 = 1 << 20;

fn ma(offset: u64, align: u32) -> MemArg {
    MemArg {
        offset,
        align,
        memory_index: 0,
    }
}

fn push_zero(fx: &mut FnCtx, vt: ValType) {
    match vt {
        ValType::I64 => fx.op(I::I64Const(0)),
        ValType::F64 => fx.op(I::F64Const(0.0.into())),
        _ => fx.op(I::I32Const(0)),
    }
}

struct FnCtx {
    instrs: Vec<I<'static>>,
    n_params: u32,
    extra_locals: Vec<ValType>,
    scopes: Vec<HashMap<String, Binding>>,
}

impl FnCtx {
    fn new(n_params: u32) -> Self {
        FnCtx {
            instrs: Vec::new(),
            n_params,
            extra_locals: Vec::new(),
            scopes: vec![],
        }
    }
    fn local(&mut self, ty: ValType) -> u32 {
        let idx = self.n_params + self.extra_locals.len() as u32;
        self.extra_locals.push(ty);
        idx
    }
    fn op(&mut self, i: I<'static>) {
        self.instrs.push(i);
    }
    fn lookup(&self, name: &str) -> Option<Binding> {
        for scope in self.scopes.iter().rev() {
            if let Some(&b) = scope.get(name) {
                return Some(b);
            }
        }
        None
    }
    fn finish(self) -> Function {
        let mut f = Function::new_with_locals_types(self.extra_locals);
        for i in &self.instrs {
            f.instruction(i);
        }
        f.instruction(&I::End);
        f
    }
}

#[derive(Default)]
struct Helpers {
    alloc: u32,
    realloc: u32,
    box_int: u32,
    box_bool: u32,
    box_dec: u32,
    box_str: u32,
    truthy: u32,
    unbox_int: u32,
    unbox_char: u32,
    unbox_dec: u32,
    eq_raw: u32,
    len_raw: u32,
    head_h: u32,
    tail_h: u32,
    strcat2: u32,
    to_str: u32,
    rec_get: u32,
    as_f64: u32,
    arith_raw: u32,
    cmp_raw: u32,
    neg_raw: u32,

    arith_int: u32,

    cmp_f64: u32,

    persist_alloc: u32,

    persist: u32,
}

impl Helpers {
    fn reserve(take: &mut impl FnMut() -> u32) -> Self {
        Self {
            alloc: take(),
            realloc: take(),
            box_int: take(),
            box_bool: take(),
            box_dec: take(),
            box_str: take(),
            truthy: take(),
            unbox_int: take(),
            unbox_char: take(),
            unbox_dec: take(),
            eq_raw: take(),
            len_raw: take(),
            head_h: take(),
            tail_h: take(),
            strcat2: take(),
            to_str: take(),
            rec_get: take(),
            as_f64: take(),
            arith_raw: take(),
            cmp_raw: take(),
            neg_raw: take(),
            arith_int: take(),
            cmp_f64: take(),
            persist_alloc: take(),
            persist: take(),
        }
    }
}

pub trait CoreOptimizer {
    fn optimize(&mut self, module: Vec<u8>) -> Result<Vec<u8>, String>;
}

impl<F> CoreOptimizer for F
where
    F: FnMut(Vec<u8>) -> Result<Vec<u8>, String>,
{
    fn optimize(&mut self, module: Vec<u8>) -> Result<Vec<u8>, String> {
        self(module)
    }
}

pub fn emit_component(
    arena: &Arena,
    roots: &[NodeId],
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> Result<Vec<u8>, String> {
    emit_component_with_optimizer(arena, roots, info, deps, &mut Ok)
}

pub fn emit_component_with_optimizer(
    arena: &Arena,
    roots: &[NodeId],
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
    optimizer: &mut dyn CoreOptimizer,
) -> Result<Vec<u8>, String> {
    validate_dependency_versions(info, deps)?;
    let scoped = scope_dependency_types(info, deps);
    let deps = &scoped;
    let module = emit_core_module(arena, roots, info, deps)?;
    let wit = synthesize_world_wit(arena, info, deps)?;
    component::encode(
        module,
        &wit,
        &crate::wit::names::identifier(&info.world),
        optimizer,
    )
}

#[derive(Clone, Debug)]
struct FnSig {
    params: Vec<Repr>,
    result: Repr,
}

type MemTy = u32;

enum MemLookup<'a> {
    Fx(&'a FnCtx),
    Sim(&'a Vec<HashMap<String, WitTy>>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Repr {
    Boxed,
    Scalar(Scalar),
    Mem(MemTy),
}

impl Repr {
    fn of_scalar(k: Option<Scalar>) -> Repr {
        match k {
            Some(k) => Repr::Scalar(k),
            None => Repr::Boxed,
        }
    }
}

fn repr_vt(repr: Repr) -> ValType {
    match repr {
        Repr::Scalar(Scalar::Int) | Repr::Scalar(Scalar::Char) => ValType::I64,
        Repr::Scalar(Scalar::Float) => ValType::F64,
        Repr::Scalar(Scalar::Bool) | Repr::Boxed | Repr::Mem(_) => ValType::I32,
    }
}

impl FnSig {
    fn boxed(n: usize) -> FnSig {
        FnSig {
            params: vec![Repr::Boxed; n],
            result: Repr::Boxed,
        }
    }

    fn param_vts(&self) -> Vec<ValType> {
        self.params.iter().map(|&p| repr_vt(p)).collect()
    }
}

fn wit_scalar(ty: &WitTy) -> Option<Scalar> {
    match ty {
        WitTy::Bool => Some(Scalar::Bool),
        WitTy::IntS(_) | WitTy::IntU(_) | WitTy::S64 => Some(Scalar::Int),
        WitTy::F32 | WitTy::F64 => Some(Scalar::Float),
        WitTy::Char => Some(Scalar::Char),
        _ => None,
    }
}

fn wit_int_range(ty: &WitTy) -> Option<(i64, i64)> {
    Some(match ty {
        WitTy::IntU(1) => (0, u8::MAX as i64),
        WitTy::IntU(2) => (0, u16::MAX as i64),
        WitTy::IntU(4) => (0, u32::MAX as i64),
        WitTy::IntS(1) => (i8::MIN as i64, i8::MAX as i64),
        WitTy::IntS(2) => (i16::MIN as i64, i16::MAX as i64),
        WitTy::IntS(4) => (i32::MIN as i64, i32::MAX as i64),
        WitTy::S64 => (i64::MIN, i64::MAX),
        _ => return None,
    })
}

fn check_int_range(t: &crate::check::Type) -> Option<(i64, i64)> {
    use crate::check::Type as T;
    Some(match t {
        T::U8 => (0, u8::MAX as i64),
        T::U16 => (0, u16::MAX as i64),
        T::U32 => (0, u32::MAX as i64),
        T::S8 => (i8::MIN as i64, i8::MAX as i64),
        T::S16 => (i16::MIN as i64, i16::MAX as i64),
        T::S32 => (i32::MIN as i64, i32::MAX as i64),

        T::S64 | T::U64 => (i64::MIN, i64::MAX),
        T::IntLit(Some(v)) => (*v, *v),
        _ => return None,
    })
}

#[derive(Clone, Copy)]
struct Binding {
    local: u32,
    repr: Repr,

    known_fn: Option<u32>,

    known_lambda: Option<u32>,
}

impl Binding {
    fn new(local: u32, repr: Repr) -> Binding {
        Binding {
            local,
            repr,
            known_fn: None,
            known_lambda: None,
        }
    }

    fn boxed(local: u32) -> Binding {
        Binding::new(local, Repr::Boxed)
    }
}

#[derive(Clone)]
struct KnownLambda {
    reserved_idx: u32,

    captures: Vec<(Repr, u32)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scalar {
    Int,
    Float,
    Bool,
    Char,
}

impl Scalar {
    fn of(ty: &crate::check::Type) -> Option<Scalar> {
        use crate::check::Type as T;
        Some(match ty {
            T::Bool => Scalar::Bool,
            T::U8 | T::U16 | T::U32 | T::U64 | T::S8 | T::S16 | T::S32 | T::S64 | T::IntLit(_) => {
                Scalar::Int
            }
            T::F32 | T::F64 | T::FloatLit => Scalar::Float,
            T::Char => Scalar::Char,
            _ => return None,
        })
    }
}

struct Emitter<'a> {
    arena: &'a Arena,
    info: &'a FileInfo,
    deps: &'a HashMap<String, Dep>,
    type_env: TypeEnv,

    local_cases: HashMap<String, bool>,
    data: Vec<u8>,
    str_cache: HashMap<String, u32>,
    types: Vec<(Vec<ValType>, Vec<ValType>)>,
    imports: Vec<(String, String, u32)>,
    import_fn: HashMap<(String, String), u32>,
    h: Helpers,
    live_helpers: Option<std::collections::HashSet<u32>>,
    funcs: HashMap<String, (u32, Vec<String>, FnSig)>,
    value_globals: HashMap<String, u32>,
    compiling_values: Vec<String>,
    bodies: Vec<(u32, Function)>,

    closure_bodies: Vec<(u32, Function)>,

    known_fn_names: Vec<String>,

    known_lambdas: Vec<KnownLambda>,
    lambda_reserved: HashMap<NodeId, u32>,
    lambda_order: Vec<NodeId>,
    lambda_stash: HashMap<NodeId, (u32, Function)>,
    fn_wrappers: HashMap<String, u32>,
    fn_box_cache: HashMap<String, u32>,
    var_box_cache: HashMap<String, u32>,
    false_addr: u32,
    true_addr: u32,

    macro_expand_idx: Option<u32>,

    functor_fns: HashMap<String, ResourceFns>,

    user_res: HashMap<String, UserRes>,

    node_types: crate::check::NodeTypes,

    mem_tys: Vec<WitTy>,
}

impl<'a> Emitter<'a> {
    fn new(arena: &'a Arena, info: &'a FileInfo, deps: &'a HashMap<String, Dep>) -> Self {
        let mut em = Self {
            arena,
            info,
            deps,
            type_env: TypeEnv::default(),
            local_cases: HashMap::new(),
            data: Vec::new(),
            str_cache: HashMap::new(),
            types: Vec::new(),
            imports: Vec::new(),
            import_fn: HashMap::new(),
            h: Helpers::default(),
            live_helpers: None,
            funcs: HashMap::new(),
            value_globals: HashMap::new(),
            compiling_values: Vec::new(),
            bodies: Vec::new(),
            closure_bodies: Vec::new(),
            known_fn_names: Vec::new(),
            known_lambdas: Vec::new(),
            lambda_reserved: HashMap::new(),
            lambda_order: Vec::new(),
            lambda_stash: HashMap::new(),
            fn_wrappers: HashMap::new(),
            fn_box_cache: HashMap::new(),
            var_box_cache: HashMap::new(),
            false_addr: 0,
            true_addr: 0,
            macro_expand_idx: None,
            functor_fns: HashMap::new(),
            user_res: HashMap::new(),
            node_types: Default::default(),
            mem_tys: Vec::new(),
        };

        em.false_addr = DATA_BASE;
        em.put_i32(TAG_BOOL);
        em.put_i32(0);
        em.true_addr = DATA_BASE + 8;
        em.put_i32(TAG_BOOL);
        em.put_i32(1);

        em
    }

    fn unit_addr(&mut self) -> u32 {
        self.intern_unit_rec()
    }

    fn arena_floor_global(&self) -> u32 {
        1 + self.info.value_defs.len() as u32 + u32::from(self.macro_expand_idx.is_some())
    }

    fn has_persist(&self) -> bool {
        !self.info.functors.is_empty() || !self.info.resources.is_empty()
    }

    fn ty_idx(&mut self, params: Vec<ValType>, results: Vec<ValType>) -> u32 {
        if let Some(i) = self
            .types
            .iter()
            .position(|t| t.0 == params && t.1 == results)
        {
            return i as u32;
        }
        self.types.push((params, results));
        (self.types.len() - 1) as u32
    }

    fn align8(&mut self) {
        while !(DATA_BASE as usize + self.data.len()).is_multiple_of(8) {
            self.data.push(0);
        }
    }

    fn put_i32(&mut self, v: i32) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }

    fn intern_str(&mut self, s: &str) -> u32 {
        if let Some(&a) = self.str_cache.get(s) {
            return a;
        }
        self.align8();
        let addr = DATA_BASE + self.data.len() as u32;
        self.put_i32(TAG_STR);
        self.put_i32(s.len() as i32);
        self.data.extend_from_slice(s.as_bytes());
        self.str_cache.insert(s.to_string(), addr);
        addr
    }

    fn intern_dec(&mut self, v: f64) -> u32 {
        self.align8();
        let addr = DATA_BASE + self.data.len() as u32;
        self.put_i32(TAG_DEC);
        self.put_i32(0);
        self.data.extend_from_slice(&v.to_le_bytes());
        addr
    }

    fn intern_unit_rec(&mut self) -> u32 {
        if let Some(&a) = self.str_cache.get("\u{1}unit-rec") {
            return a;
        }
        self.align8();
        let addr = DATA_BASE + self.data.len() as u32;
        self.put_i32(TAG_REC);
        self.put_i32(0);
        self.str_cache.insert("\u{1}unit-rec".to_string(), addr);
        addr
    }

    fn import_idx(&self, module: &str, field: &str) -> u32 {
        self.import_fn[&(module.to_string(), field.to_string())]
    }

    fn expr(&mut self, fx: &mut FnCtx, id: NodeId, tail: bool) -> Result<(), String> {
        match self.arena.node(id).clone() {
            Node::Int(n) => {
                fx.op(I::I64Const(n));
                fx.op(I::Call(self.h.box_int));
            }
            Node::Dec(d) => {
                fx.op(I::F64Const(d.into()));
                fx.op(I::Call(self.h.box_dec));
            }
            Node::Bool(b) => {
                let a = if b { self.true_addr } else { self.false_addr };
                fx.op(I::I32Const(a as i32));
            }
            Node::Str(s) => {
                let a = self.intern_str(&s);
                fx.op(I::I32Const(a as i32));
            }
            Node::Char(c) => {
                fx.op(I::I64Const(c as u32 as i64));
                self.box_char(fx);
            }
            Node::Sym(name) => match fx.lookup(&name) {
                Some(b) => match b.repr {
                    Repr::Boxed => fx.op(I::LocalGet(b.local)),
                    Repr::Scalar(kind) => {
                        fx.op(I::LocalGet(b.local));
                        self.box_scalar(fx, kind);
                    }
                    Repr::Mem(t) => {
                        let ty = self.mem_tys[t as usize].clone();
                        self.load_from_mem(fx, &ty, b.local, 0)?;
                    }
                },
                None => return self.value_def_ref(fx, &name),
            },

            Node::Tup(items) => {
                if items.is_empty() {
                    return Err("cannot evaluate empty form ()".into());
                }
                return self.call(fx, items[0], &items[1..], tail);
            }
            Node::Lst(items) => return self.list_box(fx, &items),
            Node::Rec(fields) => return self.rec_box(fx, &fields),
            Node::Flg(names) => {
                self.flg_box(fx, &names);
            }
        }
        Ok(())
    }

    fn value_def_ref(&mut self, fx: &mut FnCtx, name: &str) -> Result<(), String> {
        if name == "pi" {
            let addr = self.intern_dec(std::f64::consts::PI);
            fx.op(I::I32Const(addr as i32));
            return Ok(());
        }
        if name == "none" {
            let addr = self.none_like_box("none");
            fx.op(I::I32Const(addr as i32));
            return Ok(());
        }

        match self.local_cases.get(name) {
            Some(false) => {
                let addr = self.none_like_box(name);
                fx.op(I::I32Const(addr as i32));
                return Ok(());
            }
            Some(true) => {
                let address = self.case_value_box(name, name);
                fx.op(I::I32Const(address as i32));
                return Ok(());
            }
            None => {}
        }
        if self.funcs.contains_key(name) {
            let addr = self.fn_value_box(name)?;
            fx.op(I::I32Const(addr as i32));
            return Ok(());
        }
        if let Some((alias, member)) = crate::wit::qualified_name(self.info, name) {
            if let Ok(dep) = self.dep_for_alias(&alias)
                && let Some(payload) = dep_case(dep, &member)
            {
                let address = if payload {
                    self.case_value_box(name, &member)
                } else {
                    self.none_like_box(&member)
                };
                fx.op(I::I32Const(address as i32));
                return Ok(());
            }
            return Err(format!(
                "`{name}` used as a value (only calls are supported)"
            ));
        }
        let Some(&g) = self.value_globals.get(name) else {
            return Err(format!(
                "`{name}` is not a local binding or module-level definition \
                 (wasm backend)"
            ));
        };
        if self.compiling_values.iter().any(|v| v == name) {
            return Err(format!(
                "module-level value `{name}` is defined in terms of itself"
            ));
        }
        let init = self
            .info
            .value_defs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, e)| *e)
            .expect("value_globals entries come from value_defs");
        fx.op(I::GlobalGet(g));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        self.compiling_values.push(name.to_string());
        let r = self.expr(fx, init, false);
        self.compiling_values.pop();
        r?;
        fx.op(I::GlobalSet(g));
        fx.op(I::End);
        fx.op(I::GlobalGet(g));
        Ok(())
    }
}

fn param_names(arena: &Arena, params_id: NodeId) -> Result<Vec<String>, String> {
    match arena.node(params_id) {
        Node::Flg(names) => Ok(names.clone()),
        Node::Rec(fields) => Ok(fields.iter().map(|(k, _)| k.clone()).collect()),
        _ => Err("malformed Fn parameters".into()),
    }
}
