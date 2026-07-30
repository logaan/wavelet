//! Phase 5: emit a core wasm module per file and wrap it into a component
//! (§9 read → expand → analyze → emit → componentize).
//!
//! v0 backend scope: enough of the language to compile the §1 demo and
//! similar programs. Values are boxed in linear memory (bump allocator, no
//! GC — leaks are fine for short-lived commands):
//!
//!   offset 0: tag i32     0=bool  1=int  2=str  3=list  4=dec
//!   bool: i32 value @4              int: i64 @8
//!   str:  i32 len @4, bytes @8      list: i32 len @4, i32 box ptrs @8
//!   dec:  f64 @8
//!
//! Every Wavelet value is an i32 pointer to a box. Internal functions take
//! one i32 per parameter and return an i32; tail calls use `return_call`.

use std::collections::HashMap;

use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, ElementSection, Elements, EntityType,
    ExportKind, ExportSection, Function, FunctionSection, GlobalSection, GlobalType, ImportSection,
    Instruction as I, MemArg, MemorySection, MemoryType, Module, RefType, TableSection, TableType,
    TypeSection, ValType,
};

use crate::form::{Arena, Node, NodeId};
use crate::wit::{FileInfo, FuncSig, type_decl};

mod boxes;
mod builtins;
mod call;
mod canonical;
mod core_module;
mod deps;
mod helpers;
mod macro_component;
mod mem;
mod pattern;
mod resources;
mod scan;
mod wit_synth;
mod witty;

pub use deps::{dep_non_record_types, dep_record_types};
pub use macro_component::emit_macro_component;
pub use resources::ResourceFns;
pub use wit_synth::dep_package_wit;
pub use witty::TypeDef;

use builtins::*;
use core_module::*;
use deps::*;
use helpers::*;
use resources::*;
use scan::*;
use wit_synth::*;
use witty::*;

/// What the build step knows about a dependency component in the build set.
pub struct Dep {
    /// full package id with version, e.g. `demo:shout@0.1.0`
    pub package: String,
    pub funcs: Vec<FuncSig>,
    /// nested-package WIT text: `package demo:shout@0.1.0 { interface api {…} }`
    pub package_wit: String,
    /// record types the dep defines, name → field (name, type-string), so we
    /// can lay out record params/results we pass to/receive from it
    pub types: Vec<(String, Vec<(String, String)>)>,
    /// non-record named types the dep defines (enum/variant/flags), so the
    /// generic bridge can lower/lift values of those kinds at the boundary.
    /// Defaulted empty for Wavelet deps, which only define records today.
    pub type_defs: Vec<(String, TypeDef)>,
    /// named type *aliases* the dep defines (`type points = list<point>`):
    /// name → underlying WIT type text, expanded by `wit_ty` before lowering
    /// exactly like local `DefType` aliases (4.4).
    pub aliases: Vec<(String, String)>,
    /// which interface each named type is declared in: type name → interface
    /// name. Drives `use <pkg>/<iface>.{type};` synthesis when a local export
    /// signature references a dep-defined type (4.3).
    pub type_ifaces: Vec<(String, String)>,
}

const SCRATCH: i32 = 0; // 0..16 reserved as canonical-ABI return area
const DATA_BASE: u32 = 16;
const TAG_BOOL: i32 = 0;
const TAG_INT: i32 = 1;
const TAG_STR: i32 = 2;
const TAG_LIST: i32 = 3;
const TAG_DEC: i32 = 4;
const TAG_FN: i32 = 5; // table-slot i32 @4, n-captures @8, capture boxes @12…
const TAG_REC: i32 = 6; // n-fields i32 @4, then (key str box, value box) pairs @8+8i
const TAG_VAR: i32 = 7; // case-name str box @4, payload box (0 if none) @8
const TAG_TUP: i32 = 8; // n i32 @4, then element boxes @8+4i (list layout, distinct tag)
const TAG_FLG: i32 = 9; // a flags *form* (Node::Flg): n i32 @4, name str boxes @8+4i
const TAG_CHAR: i32 = 10; // a char value/form: i64 Unicode scalar @8 (TAG_INT layout)
const TAG_CELL: i32 = 11; // a mutable cell: current value box @4 (identity = ptr)

/// 5.1 persistent region reserve (bytes). Resource/functor components carve
/// this out below the arena floor for resource state that must survive the
/// per-call arena reset; the arena starts above it and is reset each call. A
/// fixed reserve keeps the persistent bump allocator from colliding with the
/// growable arena; `persist_alloc` traps if it is exhausted. Non-resource
/// components reserve zero.
const PERSIST_RESERVE: u32 = 1 << 20; // 1 MiB

fn ma(offset: u64, align: u32) -> MemArg {
    MemArg {
        offset,
        align,
        memory_index: 0,
    }
}

/// Push a zero of the given flat type (variant payload padding).
fn push_zero(fx: &mut FnCtx, vt: ValType) {
    match vt {
        ValType::I64 => fx.op(I::I64Const(0)),
        ValType::F64 => fx.op(I::F64Const(0.0.into())),
        _ => fx.op(I::I32Const(0)),
    }
}

// ------------------------------------------------------- function building

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

// ------------------------------------------------------------- helper ids

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
    case_h: u32,
    to_str: u32,
    rec_get: u32,
    as_f64: u32,
    arith_raw: u32,
    cmp_raw: u32,
    neg_raw: u32,
    /// `arith_int(a: i64, b: i64, op: i32) -> i64` — the checked integer
    /// arithmetic core (op: 0=add 1=sub 2=mul 3=div 4=rem), shared by the
    /// boxed `arith_raw` and the goal-5 typed scalar path so their semantics
    /// cannot drift.
    arith_int: u32,
    /// `cmp_f64(x: f64, y: f64) -> i32` in {-1,0,1}; traps on NaN. The
    /// numeric tail of `cmp_raw`, shared with the typed scalar path.
    cmp_f64: u32,
    /// `persist_alloc(n: i32) -> ptr` — bump-allocate `n` bytes in the
    /// PERSISTENT region (below the arena floor). Resource/functor components
    /// hold resource state here so it survives the per-call arena reset (5.1
    /// evacuation). Traps if the fixed reserve is exhausted. A non-resource
    /// component has a zero reserve and never calls this.
    persist_alloc: u32,
    /// `persist(box) -> box` — deep-copy a boxed value graph out of the arena
    /// into the persistent region (the 5.1 "write barrier": resource-state
    /// stores route their value through this so it outlives the reset).
    /// Interned/already-persistent nodes (`box < arena_floor`) are returned
    /// unchanged; arena nodes are copied and their children persisted
    /// recursively. Traps on `TAG_FN` (closures in resource state unsupported).
    persist: u32,
}

// ---------------------------------------------------------------- emitter

pub fn emit_component(
    arena: &Arena,
    roots: &[NodeId],
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> Result<Vec<u8>, String> {
    // Functor components: the wasm backend now emits the exported `set` resource
    // for each `Set` instantiation. `emit_core_module` synthesizes the
    // ctor/add/contains/size/dtor core funcs (step 02 bodies) and exports them
    // under the canonical resource ABI names; `synthesize_world_wit` declares and
    // exports each specialized interface, so the encoder synthesizes the matching
    // `[resource-new/rep/drop]set` intrinsics. The resource MEANS what the
    // interpreter's `set-*` builtins mean (structural `eq_raw` membership) — the
    // one hard project rule.
    //
    // Runtime routing of the qualified `pts/new`/`pts/add` ops inside an export
    // body is step 04 (`dep_call` does not yet know the functor alias). Until
    // then a body that calls `pts/<op>` fails to emit with an honest
    // "unknown import alias" — the component still builds and validates whenever
    // no body calls a functor op (the resource and its WIT are exported either
    // way).
    let mut module = emit_core_module(arena, roots, info, deps)?;
    let wit = synthesize_world_wit(arena, info, deps)?;

    let mut resolve = wit_parser::Resolve::default();
    let pkg = resolve
        .push_str("wavelet-synthesized.wit", &wit)
        .map_err(|e| {
            format!("internal: synthesized WIT did not parse: {e:#}\n--- WIT ---\n{wit}")
        })?;
    let world = resolve
        .select_world(&[pkg], Some(&info.world))
        .map_err(|e| format!("internal: world selection failed: {e:#}"))?;
    wit_component::embed_component_metadata(
        &mut module,
        &resolve,
        world,
        wit_component::StringEncoding::UTF8,
    )
    .map_err(|e| format!("embedding component metadata failed: {e:#}"))?;

    wit_component::ComponentEncoder::default()
        .validate(true)
        .module(&module)
        .map_err(|e| format!("componentizing failed: {e:#}"))?
        .encode()
        .map_err(|e| format!("component encoding failed: {e:#}"))
}

/// An internal function's representation signature (goal 5, 5.2): for each
/// parameter — and the result — either `Some(kind)` (an UNBOXED scalar slot:
/// i64 for ints/chars, f64 for floats, i32 for bools) or `None` (an i32 box
/// pointer, the pre-goal-5 uniform convention). Computed from the def's
/// declared parameter types and the checker's inferred body type, so a
/// gradual def keeps the all-boxed signature unchanged.
#[derive(Clone, Debug)]
struct FnSig {
    params: Vec<Repr>,
    result: Repr,
}

/// An interned canonical-layout type: an index into `Emitter::mem_tys`.
/// Interning keeps [`Repr`] (and so [`Binding`]) `Copy`.
type MemTy = u32;

/// Where a 5.3 eligibility scan resolves names: the live emission scopes,
/// or the simulated scopes of a pre-emission def-signature prediction.
enum MemLookup<'a> {
    Fx(&'a FnCtx),
    Sim(&'a Vec<HashMap<String, WitTy>>),
}

/// The representation of one value slot (a local, parameter, or result) in
/// goal-5 typed code: a box pointer (the uniform fallback), an unboxed
/// scalar on the wasm stack (5.2), or a pointer to the value's canonical
/// ABI layout in linear memory, typed by the interned WIT type (5.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Repr {
    Boxed,
    Scalar(Scalar),
    Mem(MemTy),
}

impl Repr {
    /// Lift a checker-derived scalar kind (`None` = not a static scalar)
    /// into a representation slot.
    fn of_scalar(k: Option<Scalar>) -> Repr {
        match k {
            Some(k) => Repr::Scalar(k),
            None => Repr::Boxed,
        }
    }
}

/// The core valtype of a representation slot.
fn repr_vt(repr: Repr) -> ValType {
    match repr {
        Repr::Scalar(Scalar::Int) | Repr::Scalar(Scalar::Char) => ValType::I64,
        Repr::Scalar(Scalar::Float) => ValType::F64,
        Repr::Scalar(Scalar::Bool) | Repr::Boxed | Repr::Mem(_) => ValType::I32,
    }
}

impl FnSig {
    /// The all-boxed signature (every slot an i32 box pointer).
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

/// The scalar kind a WIT boundary type carries at the value level, if any.
/// (`Handle` is deliberately `None`: handles ride in int boxes, opaque.)
fn wit_scalar(ty: &WitTy) -> Option<Scalar> {
    match ty {
        WitTy::Bool => Some(Scalar::Bool),
        WitTy::IntS(_) | WitTy::IntU(_) | WitTy::S64 => Some(Scalar::Int),
        WitTy::F32 | WitTy::F64 => Some(Scalar::Float),
        WitTy::Char => Some(Scalar::Char),
        _ => None,
    }
}

/// The inclusive i64 range a canonical int field of `ty` holds losslessly.
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

/// The inclusive range every runtime value of checker int type `t` is known
/// to lie in (`IntLit(Some(v))` is exactly `v`). `None` = unbounded or not
/// an int — the 5.3 gate then keeps the boxed representation.
fn check_int_range(t: &crate::check::Type) -> Option<(i64, i64)> {
    use crate::check::Type as T;
    Some(match t {
        T::U8 => (0, u8::MAX as i64),
        T::U16 => (0, u16::MAX as i64),
        T::U32 => (0, u32::MAX as i64),
        T::S8 => (i8::MIN as i64, i8::MAX as i64),
        T::S16 => (i16::MIN as i64, i16::MAX as i64),
        T::S32 => (i32::MIN as i64, i32::MAX as i64),
        // u64 rides the interpreter's i64 domain (the 5.2 residue)
        T::S64 | T::U64 => (i64::MIN, i64::MAX),
        T::IntLit(Some(v)) => (*v, *v),
        _ => return None,
    })
}

/// One name in a function's lexical scope: which wasm local holds it and,
/// under goal 5, which representation that local carries ([`Repr`]).
#[derive(Clone, Copy)]
struct Binding {
    local: u32,
    repr: Repr,
    /// 5.8 devirtualization: when this binding is statically known to hold
    /// exactly one named module-level function value (a `Let`-bound bare def
    /// reference whose checked type is a concrete arrow), the def's name
    /// (interned in [`Emitter::known_fn_names`]). Apply sites through the
    /// binding compile to a direct call instead of the TAG_FN indirect path;
    /// every other use of the binding still reads the boxed closure value.
    known_fn: Option<u32>,
    /// 5.8 devirtualization: when this binding is initialised with an inline
    /// `Fn` literal (with captures) whose checked type is a concrete arrow,
    /// the lambda has been lambda-lifted to a top-level core function at a
    /// pre-reserved index; this indexes [`Emitter::known_lambdas`], which
    /// carries that index and the ordered capture slots to push at each direct
    /// apply. Non-apply uses still read the boxed closure value `fn_form` built.
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
    /// A boxed (i32 box-pointer) binding — the pre-goal-5 default.
    fn boxed(local: u32) -> Binding {
        Binding::new(local, Repr::Boxed)
    }
}

/// 5.8 Fn-literal capture devirtualization: a lambda-lifted `Fn` literal.
/// Its body is emitted once as a top-level core function taking its captures
/// (in `captures` order) as leading typed parameters followed by its boxed
/// value parameters; a direct apply pushes the capture locals and the boxed
/// arguments and `call`s `reserved_idx` instead of allocating a closure box
/// and dispatching through the funcref table.
#[derive(Clone)]
struct KnownLambda {
    reserved_idx: u32,
    /// `(repr, local)` of each capture, in the lifted function's leading-param
    /// order — pushed verbatim (each `local` is live at every apply site because
    /// locals are single-assignment and this binding is lexically scoped).
    captures: Vec<(Repr, u32)>,
}

/// The unboxed representation of a *scalar-kinded* value on the wasm stack
/// (goal 5, 5.2/5.6.1). The kind mirrors the interpreter's value variants —
/// NOT the WIT width — so typed code computes exactly what the oracle
/// computes: every integer type (and an unresolved int literal) is the
/// interpreter's `Value::Int` domain, i.e. one i64; every float type is
/// `Value::Dec` (f64; f32 is a boundary-only representation); `Value::Bool`
/// is an i32 0/1; `Value::Char` is its Unicode scalar as an i64.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scalar {
    Int,
    Float,
    Bool,
    Char,
}

impl Scalar {
    /// The static-scalar kind of a checker type, if it has one.
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
    type_env: TypeEnv, // record types in scope (local + deps), for boundary ABI
    /// this file's own `DefType` variant/enum cases: case name → whether the
    /// case carries a payload. Bare case names construct variant values (4.1).
    local_cases: HashMap<String, bool>,
    data: Vec<u8>, // segment contents, lives at DATA_BASE
    str_cache: HashMap<String, u32>,
    types: Vec<(Vec<ValType>, Vec<ValType>)>,
    imports: Vec<(String, String, u32)>, // module, field, type idx
    import_fn: HashMap<(String, String), u32>,
    h: Helpers,
    funcs: HashMap<String, (u32, Vec<String>, FnSig)>, // internal defs: (idx, param names, repr sig)
    value_globals: HashMap<String, u32>,               // module-level value defs → global idx
    compiling_values: Vec<String>,                     // cycle guard for value-def inits
    bodies: Vec<(u32, Function)>,                      // (type idx, body) for defined funcs
    /// uniform `(env, payload) -> box` functions reachable through the
    /// funcref table; slot k = function index `imports + bodies + k`
    closure_bodies: Vec<(u32, Function)>,
    /// Interned def names for [`Binding::known_fn`] (5.8 devirtualization).
    known_fn_names: Vec<String>,
    /// 5.8 Fn-literal capture devirtualization. `known_lambdas` backs
    /// [`Binding::known_lambda`]. `lambda_reserved` maps each qualifying
    /// `Fn`-literal node (an inline Let init with a concrete arrow type) to the
    /// core-function index reserved for its lifted body — reserved right after
    /// the overload block and before the export wrappers, so `lambda_order`
    /// (reservation order) drives where the stashed bodies in `lambda_stash`
    /// are pushed into `bodies`. Empty on the macro-component path (no
    /// reservations there), so `let_form` there falls back to the boxed closure.
    known_lambdas: Vec<KnownLambda>,
    lambda_reserved: HashMap<NodeId, u32>,
    lambda_order: Vec<NodeId>,
    lambda_stash: HashMap<NodeId, (u32, Function)>,
    fn_wrappers: HashMap<String, u32>, // def name → table slot of its wrapper
    fn_box_cache: HashMap<String, u32>, // def name → static closure box addr
    var_box_cache: HashMap<String, u32>, // payload-less variant case → static box addr
    false_addr: u32,
    true_addr: u32,
    /// In a macro component, the function index of the guest-internal one-step
    /// expander, so the `expand` builtin (used *inside* a macro body) can call
    /// it. `None` in an ordinary module, where `expand` is unsupported.
    macro_expand_idx: Option<u32>,
    /// Each functor instantiation's `set` resource core-func indices, keyed by
    /// the instantiation alias (`pts`). Populated up front in `emit_core_module`
    /// — before the internal/export bodies are emitted — so `dep_call` can route
    /// an `alias/op` call (`pts/new`, `pts/add`, …) to the matching resource fn
    /// while those bodies are still being lowered (step 04 routing).
    functor_fns: HashMap<String, ResourceFns>,
    /// User-declared resource types (4.5) exported by this component, keyed by
    /// the resource name (`counter`). Populated up front so `dep_call` can route
    /// a `counter/next` method / `counter/sum` static call while bodies are still
    /// being lowered. The constructor is an ordinary internal fn registered in
    /// `funcs` under the bare resource name; methods/statics under `name/op`.
    user_res: HashMap<String, UserRes>,
    /// Per-node static types from the checker (goal 5): the emitter consults
    /// this to choose type-directed (unboxed/ABI-native) representations.
    /// Empty when checking failed or was skipped — every node then keeps the
    /// boxed fallback, which is always semantics-preserving.
    node_types: crate::check::NodeTypes,
    /// Interned canonical-layout types for [`Repr::Mem`] slots (5.3), indexed
    /// by [`MemTy`].
    mem_tys: Vec<WitTy>,
}

impl<'a> Emitter<'a> {
    /// Whether this component reserves a persistent region (5.1): true iff it
    /// instantiates a functor or declares a resource, in which case resource
    /// state (cells, functor rep lists) is allocated and written through the
    /// persistent allocator / write barrier so it survives the per-call arena
    /// reset. Non-resource components have a zero reserve, so `persist_alloc`
    /// would trap — they keep using the arena.
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

    /// Intern a static string box; returns its address.
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

    /// Intern a static dec box `[TAG_DEC, _pad, f64]` (the `box_dec` layout with
    /// the f64 at offset 8); returns its address.
    fn intern_dec(&mut self, v: f64) -> u32 {
        self.align8();
        let addr = DATA_BASE + self.data.len() as u32;
        self.put_i32(TAG_DEC);
        self.put_i32(0);
        self.data.extend_from_slice(&v.to_le_bytes());
        addr
    }

    /// Intern the empty-record box `[TAG_REC, 0]` — the backend image of the
    /// interpreter's unit value (`Value::Rec(vec![])`), which prints as "{}".
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

    // -------------------------------------------------------- expressions

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
                // a goal-5 typed local boxes at the seam to boxed code; a
                // canonical-layout local (5.3) rebuilds the box it is a
                // faithful image of
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
            // Every fully-expanded tuple in evaluation position is a call.
            Node::Tup(items) => {
                if items.is_empty() {
                    return Err("cannot evaluate empty form ()".into());
                }
                return self.call(fx, items[0], &items[1..], tail);
            }
            Node::Lst(items) => return self.list_box(fx, &items),
            Node::Rec(fields) => return self.rec_box(fx, &fields),
            Node::Flg(names) => {
                // A flags literal IS the interpreter's `Value::Flg(names)`:
                // a TAG_FLG box over the set names, in source order (5.4).
                self.flg_box(fx, &names);
            }
            Node::Qsym(a, n) => {
                // A dep-declared nullary variant/enum case referenced through
                // its import alias (`t/north`) is a payload-less variant box
                // (4.1); anything else stays call-only.
                if let Ok(dep) = self.dep_for_alias(&a)
                    && dep_case(dep, &n) == Some(false)
                {
                    let addr = self.none_like_box(&n);
                    fx.op(I::I32Const(addr as i32));
                    return Ok(());
                }
                return Err(format!(
                    "`{a}/{n}` used as a value (only calls are supported)"
                ));
            }
        }
        Ok(())
    }

    /// A name that is no local binding: a module-level value `Def` (lazily
    /// initialized global; 0 = uncomputed, no box lives at 0) or a named
    /// function used as a value (static closure box over a uniform wrapper).
    fn value_def_ref(&mut self, fx: &mut FnCtx, name: &str) -> Result<(), String> {
        if name == "pi" {
            // The stdlib constant `pi` (interp: `env.define("pi", Dec(PI))`) as a
            // static dec box.
            let addr = self.intern_dec(std::f64::consts::PI);
            fx.op(I::I32Const(addr as i32));
            return Ok(());
        }
        if name == "none" {
            let addr = self.none_like_box("none");
            fx.op(I::I32Const(addr as i32));
            return Ok(());
        }
        // A nullary case of a local `DefType` variant/enum used as a value is
        // a payload-less variant box, exactly like `none` (4.1). A payloaded
        // case as a first-class value needs a closure wrapper — not yet.
        match self.local_cases.get(name) {
            Some(false) => {
                let addr = self.none_like_box(name);
                fx.op(I::I32Const(addr as i32));
                return Ok(());
            }
            Some(true) => {
                return Err(format!(
                    "variant case constructor `{name}` used as a value is not \
                     supported by the wasm backend yet (call it directly)"
                ));
            }
            None => {}
        }
        if self.funcs.contains_key(name) {
            let addr = self.fn_value_box(name)?;
            fx.op(I::I32Const(addr as i32));
            return Ok(());
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

