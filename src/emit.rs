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

mod core_module;
mod deps;
mod helpers;
mod macro_component;
mod resources;
mod scan;
mod wit_synth;
mod witty;

pub use deps::{dep_non_record_types, dep_record_types};
pub use macro_component::emit_macro_component;
pub use resources::ResourceFns;
pub use wit_synth::dep_package_wit;
pub use witty::TypeDef;

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
    /// v0 has no record boxes; the unit value `{}` shares the static false box.
    fn unit_addr(&self) -> u32 {
        self.false_addr
    }

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

    /// Build a list box `[TAG_LIST, len, elem ptrs…]` from element forms.
    fn list_box(&mut self, fx: &mut FnCtx, items: &[NodeId]) -> Result<(), String> {
        self.seq_box(fx, items, TAG_LIST)
    }

    /// Build a sequence box `[tag, len, elem ptrs…]`; `tag` is TAG_LIST or
    /// TAG_TUP (identical layout, distinct identity at the value level).
    fn seq_box(&mut self, fx: &mut FnCtx, items: &[NodeId], tag: i32) -> Result<(), String> {
        let n = items.len();
        let p = fx.local(ValType::I32);
        fx.op(I::I32Const(8 + 4 * n as i32));
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(p));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(tag));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(n as i32));
        fx.op(I::I32Store(ma(4, 2)));
        for (i, &item) in items.iter().enumerate() {
            fx.op(I::LocalGet(p));
            self.expr(fx, item, false)?;
            fx.op(I::I32Store(ma(8 + 4 * i as u64, 2)));
        }
        fx.op(I::LocalGet(p));
        Ok(())
    }

    /// Build a record box `[TAG_REC, n, (key str box, value box)…]` from field
    /// forms. Keys are interned static string boxes; insertion order preserved.
    fn rec_box(&mut self, fx: &mut FnCtx, fields: &[(String, NodeId)]) -> Result<(), String> {
        let n = fields.len();
        let p = fx.local(ValType::I32);
        fx.op(I::I32Const(8 + 8 * n as i32));
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(p));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(TAG_REC));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(n as i32));
        fx.op(I::I32Store(ma(4, 2)));
        for (i, (k, v)) in fields.iter().enumerate() {
            let kaddr = self.intern_str(k);
            fx.op(I::LocalGet(p));
            fx.op(I::I32Const(kaddr as i32));
            fx.op(I::I32Store(ma(8 + 8 * i as u64, 2)));
            fx.op(I::LocalGet(p));
            self.expr(fx, *v, false)?;
            fx.op(I::I32Store(ma(12 + 8 * i as u64, 2)));
        }
        fx.op(I::LocalGet(p));
        Ok(())
    }

    /// Build a variant box `[TAG_VAR, case str box, payload box]` for a case
    /// carrying a payload (`some`/`ok`/`err` and user cases). Leaves the box
    /// pointer on the stack; `payload` is the form for the carried value.
    /// Build a payloaded variant box `[TAG_VAR, case, payload]`. The payload is
    /// the call's bundled arguments, matching the interpreter's `ok`/`err`/`some`
    /// exactly: 0 args ⇒ the empty tuple, 1 arg ⇒ that value, ≥2 ⇒ a tuple.
    fn var_box(&mut self, fx: &mut FnCtx, case: &str, args: &[NodeId]) -> Result<(), String> {
        let caddr = self.intern_str(case);
        let p = fx.local(ValType::I32);
        fx.op(I::I32Const(12));
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(p));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(caddr as i32));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(p));
        match args {
            [one] => self.expr(fx, *one, false)?,
            _ => self.seq_box(fx, args, TAG_TUP)?,
        }
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(p));
        Ok(())
    }

    /// Build the box for a *quoted* form — the compile-time analogue of
    /// `value::form_to_value` (`value.rs:104`). Used by `Quote` and at the leaves
    /// of `Quasi`. Unlike `seq_box`/`rec_box`, the children are themselves quoted
    /// (built as data), never evaluated. A `Sym` becomes a payload-less `TAG_VAR`
    /// (`Sym → Variant(name, none)`), a `Qsym` the same over `"alias/name"`.
    fn quote_box(&mut self, fx: &mut FnCtx, id: NodeId) -> Result<(), String> {
        match self.arena.node(id).clone() {
            Node::Bool(b) => {
                let a = if b { self.true_addr } else { self.false_addr };
                fx.op(I::I32Const(a as i32));
            }
            Node::Int(n) => {
                fx.op(I::I64Const(n));
                fx.op(I::Call(self.h.box_int));
            }
            Node::Dec(d) => {
                fx.op(I::F64Const(d.into()));
                fx.op(I::Call(self.h.box_dec));
            }
            Node::Str(s) => {
                let a = self.intern_str(&s);
                fx.op(I::I32Const(a as i32));
            }
            Node::Sym(s) => {
                let a = self.none_like_box(&s);
                fx.op(I::I32Const(a as i32));
            }
            Node::Qsym(alias, name) => {
                let a = self.none_like_box(&format!("{alias}/{name}"));
                fx.op(I::I32Const(a as i32));
            }
            Node::Tup(items) => return self.quote_seq(fx, &items, TAG_TUP),
            Node::Lst(items) => return self.quote_seq(fx, &items, TAG_LIST),
            Node::Rec(fields) => return self.quote_rec(fx, &fields),
            Node::Flg(names) => {
                return {
                    self.flg_box(fx, &names);
                    Ok(())
                };
            }
            Node::Char(c) => {
                fx.op(I::I64Const(c as u32 as i64));
                self.box_char(fx);
            }
        }
        Ok(())
    }

    /// Build a flags *form* box `[TAG_FLG, n, name str boxes…]` (the box analogue
    /// of `Node::Flg`/`Value::Flg`). Names are interned static string boxes.
    fn flg_box(&mut self, fx: &mut FnCtx, names: &[String]) {
        let n = names.len();
        let p = fx.local(ValType::I32);
        fx.op(I::I32Const(8 + 4 * n as i32));
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(p));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(TAG_FLG));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(n as i32));
        fx.op(I::I32Store(ma(4, 2)));
        for (i, nm) in names.iter().enumerate() {
            let kaddr = self.intern_str(nm);
            fx.op(I::LocalGet(p));
            fx.op(I::I32Const(kaddr as i32));
            fx.op(I::I32Store(ma(8 + 4 * i as u64, 2)));
        }
        fx.op(I::LocalGet(p));
    }

    /// Stack `[i64 codepoint]` → `[char box]`: a `[TAG_CHAR, _, i64 @8]` box
    /// (the `TAG_INT` layout under a distinct tag, so `form-kind` and the wire
    /// `char-val` node stay distinct from plain ints).
    fn box_char(&mut self, fx: &mut FnCtx) {
        let cp = fx.local(ValType::I64);
        fx.op(I::LocalSet(cp));
        let p = fx.local(ValType::I32);
        fx.op(I::I32Const(16));
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalTee(p));
        fx.op(I::I32Const(TAG_CHAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(cp));
        fx.op(I::I64Store(ma(8, 3)));
        fx.op(I::LocalGet(p));
    }

    /// `quote_box` analogue of `seq_box`: a `[tag, len, quoted-elem ptrs…]` box
    /// whose elements are quoted, not evaluated.
    fn quote_seq(&mut self, fx: &mut FnCtx, items: &[NodeId], tag: i32) -> Result<(), String> {
        let n = items.len();
        let p = fx.local(ValType::I32);
        fx.op(I::I32Const(8 + 4 * n as i32));
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(p));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(tag));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(n as i32));
        fx.op(I::I32Store(ma(4, 2)));
        for (i, &item) in items.iter().enumerate() {
            fx.op(I::LocalGet(p));
            self.quote_box(fx, item)?;
            fx.op(I::I32Store(ma(8 + 4 * i as u64, 2)));
        }
        fx.op(I::LocalGet(p));
        Ok(())
    }

    /// `quote_box` analogue of `rec_box`: a record box whose values are quoted.
    fn quote_rec(&mut self, fx: &mut FnCtx, fields: &[(String, NodeId)]) -> Result<(), String> {
        let n = fields.len();
        let p = fx.local(ValType::I32);
        fx.op(I::I32Const(8 + 8 * n as i32));
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(p));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(TAG_REC));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(n as i32));
        fx.op(I::I32Store(ma(4, 2)));
        for (i, (k, v)) in fields.iter().enumerate() {
            let kaddr = self.intern_str(k);
            fx.op(I::LocalGet(p));
            fx.op(I::I32Const(kaddr as i32));
            fx.op(I::I32Store(ma(8 + 8 * i as u64, 2)));
            fx.op(I::LocalGet(p));
            self.quote_box(fx, *v)?;
            fx.op(I::I32Store(ma(12 + 8 * i as u64, 2)));
        }
        fx.op(I::LocalGet(p));
        Ok(())
    }

    /// Compile a `Quasi` template into a box, mirroring `Interp::quasi`
    /// (`interp.rs:350`) exactly. `depth` counts enclosing `Quasi`s: `Unquote`
    /// /`Splice` fire at depth 1 (the hole is the compiled expression) and are
    /// rebuilt as data one level shallower at greater depths; a nested `Quasi`
    /// recurses at `depth + 1`. Leaves are quoted (`quote_box`).
    fn quasi_box(&mut self, fx: &mut FnCtx, id: NodeId, depth: u32) -> Result<(), String> {
        match self.arena.node(id).clone() {
            Node::Tup(items) => {
                // The arity-1 special heads read as 2-element tuples
                // `[head-MACRO, arg]`; everything else is a sequence.
                if items.len() == 2
                    && let Node::Sym(name) = self.arena.node(items[0]).clone()
                {
                    let arg = items[1];
                    match name.as_str() {
                        "unquote-MACRO" if depth == 1 => return self.expr(fx, arg, false),
                        "splice-MACRO" if depth == 1 => {
                            return Err("Splice must appear inside a sequence".into());
                        }
                        "unquote-MACRO" | "splice-MACRO" if depth > 1 => {
                            return self.quasi_rebuild_head(fx, &name, arg, depth - 1);
                        }
                        "quasi-MACRO" => {
                            return self.quasi_rebuild_head(fx, &name, arg, depth + 1);
                        }
                        _ => {}
                    }
                }
                self.quasi_seq(fx, &items, TAG_TUP, depth)
            }
            Node::Lst(items) => self.quasi_seq(fx, &items, TAG_LIST, depth),
            Node::Rec(fields) => {
                let n = fields.len();
                let p = fx.local(ValType::I32);
                fx.op(I::I32Const(8 + 8 * n as i32));
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(p));
                fx.op(I::LocalGet(p));
                fx.op(I::I32Const(TAG_REC));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(p));
                fx.op(I::I32Const(n as i32));
                fx.op(I::I32Store(ma(4, 2)));
                for (i, (k, v)) in fields.iter().enumerate() {
                    let kaddr = self.intern_str(k);
                    fx.op(I::LocalGet(p));
                    fx.op(I::I32Const(kaddr as i32));
                    fx.op(I::I32Store(ma(8 + 8 * i as u64, 2)));
                    fx.op(I::LocalGet(p));
                    self.quasi_box(fx, *v, depth)?;
                    fx.op(I::I32Store(ma(12 + 8 * i as u64, 2)));
                }
                fx.op(I::LocalGet(p));
                Ok(())
            }
            _ => self.quote_box(fx, id),
        }
    }

    /// Rebuild a deeper-level `Unquote`/`Splice`/`Quasi` head as a 2-element
    /// `TAG_TUP` `[Variant(name, none), <recursed arg>]`, exactly as
    /// `Interp::quasi` does when `depth != 1`.
    fn quasi_rebuild_head(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        arg: NodeId,
        depth: u32,
    ) -> Result<(), String> {
        let head = self.none_like_box(name);
        let p = fx.local(ValType::I32);
        fx.op(I::I32Const(16));
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(p));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(TAG_TUP));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(2));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(head as i32));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(p));
        self.quasi_box(fx, arg, depth)?;
        fx.op(I::I32Store(ma(12, 2)));
        fx.op(I::LocalGet(p));
        Ok(())
    }

    /// Build a `Quasi` sequence box (`TAG_TUP`/`TAG_LIST`). Mirrors
    /// `Interp::quasi_seq` (`interp.rs:396`): at `depth == 1` a child
    /// `(Splice expr)` evaluates to a list whose elements are spliced into the
    /// surrounding sequence; every other child is built via `quasi_box`. When no
    /// splice is present the length is static; otherwise it is computed at
    /// runtime.
    fn quasi_seq(
        &mut self,
        fx: &mut FnCtx,
        items: &[NodeId],
        tag: i32,
        depth: u32,
    ) -> Result<(), String> {
        // Classify each child as `(is_splice, expr/item)`. A splice is only
        // recognised at depth 1, matching the interpreter.
        let mut segs: Vec<(bool, NodeId)> = Vec::with_capacity(items.len());
        for &item in items {
            if depth == 1
                && let Node::Tup(t) = self.arena.node(item).clone()
                && t.len() == 2
                && let Node::Sym(s) = self.arena.node(t[0]).clone()
                && s == "splice-MACRO"
            {
                segs.push((true, t[1]));
                continue;
            }
            segs.push((false, item));
        }

        // Static fast path: no splices ⇒ fixed length, like `quote_seq`.
        if segs.iter().all(|(sp, _)| !sp) {
            let n = segs.len();
            let p = fx.local(ValType::I32);
            fx.op(I::I32Const(8 + 4 * n as i32));
            fx.op(I::Call(self.h.alloc));
            fx.op(I::LocalSet(p));
            fx.op(I::LocalGet(p));
            fx.op(I::I32Const(tag));
            fx.op(I::I32Store(ma(0, 2)));
            fx.op(I::LocalGet(p));
            fx.op(I::I32Const(n as i32));
            fx.op(I::I32Store(ma(4, 2)));
            for (i, (_, item)) in segs.iter().enumerate() {
                fx.op(I::LocalGet(p));
                self.quasi_box(fx, *item, depth)?;
                fx.op(I::I32Store(ma(8 + 4 * i as u64, 2)));
            }
            fx.op(I::LocalGet(p));
            return Ok(());
        }

        // Dynamic path: evaluate each segment into a local, summing the total
        // element count (1 per ordinary child, the list length per splice).
        let total = fx.local(ValType::I32);
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(total));
        let mut seg_locals: Vec<(bool, u32)> = Vec::with_capacity(segs.len());
        for (is_splice, node) in &segs {
            let l = fx.local(ValType::I32);
            if *is_splice {
                self.expr(fx, *node, false)?;
                fx.op(I::LocalSet(l));
                // Splice expects a list (`interp.rs:411`); trap otherwise.
                fx.op(I::LocalGet(l));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(total));
                fx.op(I::LocalGet(l));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(total));
            } else {
                self.quasi_box(fx, *node, depth)?;
                fx.op(I::LocalSet(l));
                fx.op(I::LocalGet(total));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(total));
            }
            seg_locals.push((*is_splice, l));
        }

        // Allocate the final box (`8 + 4*total` bytes) and fill it, copying each
        // splice's elements element-by-element.
        let p = fx.local(ValType::I32);
        fx.op(I::LocalGet(total));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(p));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(tag));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(total));
        fx.op(I::I32Store(ma(4, 2)));
        let w = fx.local(ValType::I32);
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(w));
        for (is_splice, l) in seg_locals {
            if is_splice {
                let i = fx.local(ValType::I32);
                let len = fx.local(ValType::I32);
                fx.op(I::LocalGet(l));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(len));
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(len));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));
                // dst = p + 8 + 4*w
                fx.op(I::LocalGet(p));
                fx.op(I::LocalGet(w));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                // src value = load [l + 8 + 4*i]
                fx.op(I::LocalGet(l));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(8, 2)));
                fx.op(I::I32Store(ma(8, 2)));
                fx.op(I::LocalGet(w));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(w));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(i));
                fx.op(I::Br(0));
                fx.op(I::End);
                fx.op(I::End);
            } else {
                fx.op(I::LocalGet(p));
                fx.op(I::LocalGet(w));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::LocalGet(l));
                fx.op(I::I32Store(ma(8, 2)));
                fx.op(I::LocalGet(w));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(w));
            }
        }
        fx.op(I::LocalGet(p));
        Ok(())
    }

    /// Address of a static payload-less variant box `[TAG_VAR, case, 0]`
    /// (e.g. `none`); interned once per case name.
    fn none_like_box(&mut self, case: &str) -> u32 {
        if let Some(&a) = self.var_box_cache.get(case) {
            return a;
        }
        let caddr = self.intern_str(case);
        self.align8();
        let addr = DATA_BASE + self.data.len() as u32;
        self.put_i32(TAG_VAR);
        self.put_i32(caddr as i32);
        self.put_i32(0);
        self.var_box_cache.insert(case.to_string(), addr);
        addr
    }

    /// Stack `[payload_box]` → `[variant_box]`: allocate `[TAG_VAR, case, pay]`.
    fn wrap_variant(&mut self, fx: &mut FnCtx, case: &str) {
        let caddr = self.intern_str(case);
        let pay = fx.local(ValType::I32);
        let p = fx.local(ValType::I32);
        fx.op(I::LocalSet(pay));
        fx.op(I::I32Const(12));
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(p));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(caddr as i32));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(pay));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(p));
    }

    /// Static closure box for a named def used as a value: `[TAG_FN, slot, 0]`
    /// over a uniform wrapper that forwards to the direct function.
    fn fn_value_box(&mut self, name: &str) -> Result<u32, String> {
        if let Some(&a) = self.fn_box_cache.get(name) {
            return Ok(a);
        }
        let slot = self.def_wrapper_slot(name)?;
        self.align8();
        let addr = DATA_BASE + self.data.len() as u32;
        self.put_i32(TAG_FN);
        self.put_i32(slot as i32);
        self.put_i32(0);
        self.fn_box_cache.insert(name.to_string(), addr);
        Ok(addr)
    }

    /// Table slot of the uniform `(env, payload) -> box` wrapper for a named
    /// def: unpacks the payload per §4.2 by arity and tail-calls the function.
    fn def_wrapper_slot(&mut self, name: &str) -> Result<u32, String> {
        if let Some(&s) = self.fn_wrappers.get(name) {
            return Ok(s);
        }
        let (fidx, params, sig) = self.funcs[name].clone();
        let mut fx = FnCtx::new(2);
        match params.len() {
            0 => {}
            1 => {
                fx.op(I::LocalGet(1));
                // a typed sole param unboxes at the wrapper seam
                if let Repr::Scalar(k) = sig.params[0] {
                    self.unbox_scalar(&mut fx, k);
                }
            }
            n => {
                // payload must be a tuple box of exactly n elements (the
                // `payload_box` bundle), same guard `fn_form` emits, so a
                // malformed indirect call — e.g. a single list value passed to
                // an n-ary function (2.1 proposal 1) — traps rather than reading
                // garbage past the box.
                fx.op(I::LocalGet(1));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_TUP));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(1));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::I32Const(n as i32));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                for i in 0..n {
                    fx.op(I::LocalGet(1));
                    fx.op(I::I32Load(ma(8 + 4 * i as u64, 2)));
                    if let Repr::Scalar(k) = sig.params[i] {
                        self.unbox_scalar(&mut fx, k);
                    }
                }
            }
        }
        // the uniform wrapper returns a box: a typed result boxes at the seam
        match sig.result {
            Repr::Scalar(k) => {
                fx.op(I::Call(fidx));
                self.box_scalar(&mut fx, k);
            }
            Repr::Mem(t) => {
                fx.op(I::Call(fidx));
                let l = fx.local(ValType::I32);
                fx.op(I::LocalSet(l));
                let ty = self.mem_tys[t as usize].clone();
                self.load_from_mem(&mut fx, &ty, l, 0)?;
            }
            Repr::Boxed => fx.op(I::ReturnCall(fidx)),
        }
        let t = self.ty_idx(vec![ValType::I32; 2], vec![ValType::I32]);
        self.closure_bodies.push((t, fx.finish()));
        let slot = (self.closure_bodies.len() - 1) as u32;
        self.fn_wrappers.insert(name.to_string(), slot);
        Ok(slot)
    }

    /// `Fn {params} body` as an expression: compile the body to a uniform
    /// `(env, payload) -> box` table function capturing every visible local,
    /// and allocate a closure box `[TAG_FN, slot, k, captures…]` at the site.
    fn fn_form(&mut self, fx: &mut FnCtx, args: &[NodeId]) -> Result<(), String> {
        let [params_id, body] = *args else {
            return Err("malformed Fn".into());
        };
        let params = param_names(self.arena, params_id)?;

        // captures: every visible local by name (later scopes shadow earlier),
        // sorted so the layout is deterministic
        let mut cap_map: HashMap<String, Binding> = HashMap::new();
        for scope in &fx.scopes {
            for (k, &v) in scope {
                cap_map.insert(k.clone(), v);
            }
        }
        let mut caps: Vec<(String, Binding)> = cap_map.into_iter().collect();
        caps.sort_by(|a, b| a.0.cmp(&b.0));

        let mut cf = FnCtx::new(2);
        let mut scope = HashMap::new();
        for (j, (cname, _)) in caps.iter().enumerate() {
            let l = cf.local(ValType::I32);
            cf.op(I::LocalGet(0));
            cf.op(I::I32Load(ma(12 + 4 * j as u64, 2)));
            cf.op(I::LocalSet(l));
            scope.insert(cname.clone(), Binding::boxed(l));
        }
        match params.len() {
            0 => {}
            1 => {
                scope.insert(params[0].clone(), Binding::boxed(1));
            }
            n => {
                // payload must be a tuple box of exactly n elements (the
                // `payload_box` bundle); a single list value never spreads
                // across parameters (2.1 proposal 1).
                cf.op(I::LocalGet(1));
                cf.op(I::I32Load(ma(0, 2)));
                cf.op(I::I32Const(TAG_TUP));
                cf.op(I::I32Ne);
                cf.op(I::If(BlockType::Empty));
                cf.op(I::Unreachable);
                cf.op(I::End);
                cf.op(I::LocalGet(1));
                cf.op(I::I32Load(ma(4, 2)));
                cf.op(I::I32Const(n as i32));
                cf.op(I::I32Ne);
                cf.op(I::If(BlockType::Empty));
                cf.op(I::Unreachable);
                cf.op(I::End);
                for (i, p) in params.iter().enumerate() {
                    let l = cf.local(ValType::I32);
                    cf.op(I::LocalGet(1));
                    cf.op(I::I32Load(ma(8 + 4 * i as u64, 2)));
                    cf.op(I::LocalSet(l));
                    scope.insert(p.clone(), Binding::boxed(l));
                }
            }
        }
        cf.scopes.push(scope);
        self.expr(&mut cf, body, true)?;
        let t = self.ty_idx(vec![ValType::I32; 2], vec![ValType::I32]);
        self.closure_bodies.push((t, cf.finish()));
        let slot = (self.closure_bodies.len() - 1) as u32;

        let k = caps.len();
        let p = fx.local(ValType::I32);
        fx.op(I::I32Const(12 + 4 * k as i32));
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(p));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(TAG_FN));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(slot as i32));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(k as i32));
        fx.op(I::I32Store(ma(8, 2)));
        for (j, (_, cap)) in caps.iter().enumerate() {
            fx.op(I::LocalGet(p));
            // a goal-5 typed local boxes at the capture seam (closure
            // capture slots hold box pointers)
            match cap.repr {
                Repr::Boxed => fx.op(I::LocalGet(cap.local)),
                Repr::Scalar(kind) => {
                    fx.op(I::LocalGet(cap.local));
                    self.box_scalar(fx, kind);
                }
                Repr::Mem(t) => {
                    let ty = self.mem_tys[t as usize].clone();
                    self.load_from_mem(fx, &ty, cap.local, 0)?;
                }
            }
            fx.op(I::I32Store(ma(12 + 4 * j as u64, 2)));
        }
        fx.op(I::LocalGet(p));
        Ok(())
    }

    /// 5.8 capture devirtualization: lambda-lift a qualifying `Fn` literal
    /// (an inline Let init with a pre-reserved index) to a top-level core
    /// function and record it on the binding.
    ///
    /// The lifted function takes the lambda's captures — every currently-visible
    /// local, in the same name-sorted order `fn_form` uses — as leading typed
    /// parameters, followed by its value parameters (boxed, first cut), and
    /// returns a boxed result. Its body is compiled here (captures and params
    /// bound as direct wasm parameters, not env-unpacked) and stashed for the
    /// assembly step to push at `reserved_idx`. Returns the `known_lambdas`
    /// index, or `None` when the node was not pre-reserved (e.g. the macro
    /// component path, or a lambda in an un-scanned body).
    fn compile_known_lambda(
        &mut self,
        fx: &FnCtx,
        fn_node: NodeId,
    ) -> Result<Option<u32>, String> {
        let Some(&reserved_idx) = self.lambda_reserved.get(&fn_node) else {
            return Ok(None);
        };
        let Node::Tup(items) = self.arena.node(fn_node).clone() else {
            return Ok(None);
        };
        if items.len() != 3 {
            return Ok(None);
        }
        let (params_id, body) = (items[1], items[2]);
        let params = param_names(self.arena, params_id)?;

        // captures: every visible local by name (later scopes shadow earlier),
        // sorted so the layout is deterministic — identical to `fn_form`.
        let mut cap_map: HashMap<String, Binding> = HashMap::new();
        for scope in &fx.scopes {
            for (k, &v) in scope {
                cap_map.insert(k.clone(), v);
            }
        }
        let mut caps: Vec<(String, Binding)> = cap_map.into_iter().collect();
        caps.sort_by(|a, b| a.0.cmp(&b.0));
        let ncap = caps.len() as u32;

        let mut cf = FnCtx::new(ncap + params.len() as u32);
        let mut scope = HashMap::new();
        for (i, (cname, cb)) in caps.iter().enumerate() {
            // a capture becomes a direct typed parameter at its current repr
            scope.insert(cname.clone(), Binding::new(i as u32, cb.repr));
        }
        for (j, p) in params.iter().enumerate() {
            scope.insert(p.clone(), Binding::boxed(ncap + j as u32));
        }
        cf.scopes.push(scope);
        self.expr(&mut cf, body, true)?;

        let mut pvts: Vec<ValType> = caps.iter().map(|(_, b)| repr_vt(b.repr)).collect();
        pvts.extend(std::iter::repeat(ValType::I32).take(params.len()));
        let t = self.ty_idx(pvts, vec![ValType::I32]);
        self.lambda_stash.insert(fn_node, (t, cf.finish()));

        let captures: Vec<(Repr, u32)> = caps.iter().map(|(_, b)| (b.repr, b.local)).collect();
        self.known_lambdas.push(KnownLambda {
            reserved_idx,
            captures,
        });
        Ok(Some(self.known_lambdas.len() as u32 - 1))
    }

    /// Indirect call through a closure box: `(box, payload-box)` via the
    /// funcref table slot stored in the box at offset 4.
    fn closure_call(
        &mut self,
        fx: &mut FnCtx,
        head: NodeId,
        args: &[NodeId],
        tail: bool,
    ) -> Result<(), String> {
        self.expr(fx, head, false)?;
        let c = fx.local(ValType::I32);
        fx.op(I::LocalSet(c));
        fx.op(I::LocalGet(c)); // env argument = the closure box itself
        self.payload_box(fx, args)?;
        fx.op(I::LocalGet(c));
        fx.op(I::I32Load(ma(4, 2))); // table slot
        let t = self.ty_idx(vec![ValType::I32; 2], vec![ValType::I32]);
        fx.op(if tail {
            I::ReturnCallIndirect {
                type_index: t,
                table_index: 0,
            }
        } else {
            I::CallIndirect {
                type_index: t,
                table_index: 0,
            }
        });
        Ok(())
    }

    /// Bundle a call's evaluated arguments into one payload box, mirroring the
    /// interpreter's `bundle_args`: 0 args ⇒ the empty tuple, 1 arg ⇒ the value
    /// itself, ≥2 ⇒ a tuple box. The multi-arg bundle is a TAG_TUP (not a list)
    /// so the wrapper can tell an argument bundle apart from a single list value
    /// passed as the sole argument — a list never spreads across parameters
    /// (2.1 proposal 1), matching the interpreter.
    fn payload_box(&mut self, fx: &mut FnCtx, args: &[NodeId]) -> Result<(), String> {
        match args {
            [] => self.seq_box(fx, &[], TAG_TUP),
            [one] => self.expr(fx, *one, false),
            many => self.seq_box(fx, many, TAG_TUP),
        }
    }

    fn call(
        &mut self,
        fx: &mut FnCtx,
        head: NodeId,
        args: &[NodeId],
        tail: bool,
    ) -> Result<(), String> {
        let head_node = self.arena.node(head).clone();
        match head_node {
            Node::Qsym(alias, fname) => {
                // Every imported call goes through the generic canonical-ABI
                // bridge, driven by the import's parsed WIT signature (from a
                // sibling `.wlt` or a `wit/deps` package — host `wasi:*`
                // packages included).
                self.dep_call(fx, &alias, &fname, args, None)
            }
            Node::Sym(name) => match name.as_str() {
                "let-MACRO" => self.let_form(fx, args, Repr::Boxed, tail),
                "the-MACRO" => {
                    // args = [ty, expr]
                    let [_ty, expr] = *args else {
                        return Err("malformed The".into());
                    };
                    self.expr(fx, expr, tail)
                }
                "match-MACRO" => self.match_form(fx, args, Repr::Boxed, tail),
                "fn-MACRO" => self.fn_form(fx, args),
                "quote-MACRO" => {
                    let [form] = args else {
                        return Err("malformed Quote".into());
                    };
                    self.quote_box(fx, *form)
                }
                "quasi-MACRO" => {
                    let [form] = args else {
                        return Err("malformed Quasi".into());
                    };
                    self.quasi_box(fx, *form, 1)
                }
                "def-MACRO" | "defmacro-MACRO" => {
                    Err(format!("`{name}` not supported by the wasm backend yet"))
                }
                _ if fx.lookup(&name).is_some() => {
                    // 5.8 devirtualization: an apply through a binding known to
                    // hold exactly one named def compiles as a direct call —
                    // the same code path a direct `dname(args…)` takes — instead
                    // of bundling a payload and calling through the TAG_FN
                    // table. Semantics agree: the interpreter applies the same
                    // closure either way.
                    if let Some(k) = fx.lookup(&name).and_then(|b| b.known_fn) {
                        let dname = self.known_fn_names[k as usize].clone();
                        return self.internal_call(fx, &dname, args, Repr::Boxed, tail);
                    }
                    // 5.8 capture devirtualization: an apply through a binding
                    // holding a lambda-lifted `Fn` literal pushes the captured
                    // locals (verbatim, at their reprs) and the boxed arguments,
                    // then calls the lifted function directly — no closure box,
                    // no funcref dispatch. Arity is guaranteed by the checker
                    // (`check_indirect_apply`); the lifted body binds each value
                    // parameter boxed, exactly as the boxed closure would.
                    if let Some(kl_idx) = fx.lookup(&name).and_then(|b| b.known_lambda) {
                        let kl = self.known_lambdas[kl_idx as usize].clone();
                        for (_repr, local) in &kl.captures {
                            fx.op(I::LocalGet(*local));
                        }
                        for a in args {
                            self.expr(fx, *a, false)?;
                        }
                        fx.op(if tail {
                            I::ReturnCall(kl.reserved_idx)
                        } else {
                            I::Call(kl.reserved_idx)
                        });
                        return Ok(());
                    }
                    self.closure_call(fx, head, args, tail)
                }
                _ if BUILTINS.contains(&name.as_str()) => self.builtin(fx, &name, args),
                _ => {
                    if self.funcs.contains_key(&name) {
                        self.internal_call(fx, &name, args, Repr::Boxed, tail)
                    } else if self.value_globals.contains_key(&name) {
                        self.closure_call(fx, head, args, tail)
                    } else if let Some(&has_payload) = self.local_cases.get(name.as_str()) {
                        // A `DefType` variant case constructor call (4.1): build
                        // the variant box, bundling ≥2 payload args as a tuple
                        // exactly like the interpreter's `CaseCtor`.
                        if !has_payload || args.is_empty() {
                            return Err(format!(
                                "variant case `{name}` {} (wasm backend)",
                                if has_payload {
                                    "takes a payload, got no arguments"
                                } else {
                                    "is not callable"
                                }
                            ));
                        }
                        self.var_box(fx, &name, args)
                    } else {
                        Err(format!("unknown function `{name}` (wasm backend)"))
                    }
                }
            },
            // any other head evaluates to a closure box
            _ => self.closure_call(fx, head, args, tail),
        }
    }

    fn let_form(
        &mut self,
        fx: &mut FnCtx,
        args: &[NodeId],
        want: Repr,
        tail: bool,
    ) -> Result<(), String> {
        let [bindings, body] = *args else {
            return Err("malformed Let".into());
        };
        let Node::Rec(fields) = self.arena.node(bindings).clone() else {
            return Err("Let bindings must be a record".into());
        };
        fx.scopes.push(HashMap::new());
        for (k, v) in &fields {
            // Goal 5 (5.2): a binding with a statically-known scalar type
            // lives UNBOXED in a typed wasm local; scalar consumers read it
            // directly, boxed consumers box at the reference seam.
            let binding = if let Some(kind) = self.node_scalar(*v) {
                self.expr_scalar(fx, *v, kind)?;
                let l = fx.local(match kind {
                    Scalar::Int | Scalar::Char => ValType::I64,
                    Scalar::Float => ValType::F64,
                    Scalar::Bool => ValType::I32,
                });
                Binding::new(l, Repr::Scalar(kind))
            } else if let Some(t) = self.node_mem(fx, *v) {
                // 5.3: a record binding whose construction is provably
                // faithful to its static type lives in canonical layout;
                // boxed consumers rebuild the box at the reference seam
                self.expr_mem(fx, *v, t, false)?;
                Binding::new(fx.local(ValType::I32), Repr::Mem(t))
            } else {
                self.expr(fx, *v, false)?;
                let mut b = Binding::boxed(fx.local(ValType::I32));
                // 5.8 devirtualization: a binding initialised with a bare
                // module-level def reference whose checked type is a concrete
                // arrow holds exactly that function value — record the def so
                // apply sites through this binding compile to a direct call
                // (the zero-capture defunctionalization case). The boxed
                // closure value is still built: non-apply uses read it.
                if let Node::Sym(dname) = self.arena.node(*v)
                    && self.funcs.contains_key(dname)
                    && matches!(self.node_types.get(v), Some(crate::check::Type::Fn(..)))
                {
                    let idx = self
                        .known_fn_names
                        .iter()
                        .position(|n| n == dname)
                        .unwrap_or_else(|| {
                            self.known_fn_names.push(dname.clone());
                            self.known_fn_names.len() - 1
                        });
                    b.known_fn = Some(idx as u32);
                }
                // 5.8 capture devirtualization: an inline `Fn` literal init
                // with a concrete arrow type is lambda-lifted to a direct-call
                // core function; the boxed closure built above still serves any
                // non-apply use of the binding. (`compile_known_lambda` returns
                // `None` for a non-reserved node, so the two cases are disjoint.)
                if let Some(kl) = self.compile_known_lambda(fx, *v)? {
                    b.known_lambda = Some(kl);
                }
                b
            };
            fx.op(I::LocalSet(binding.local));
            fx.scopes.last_mut().unwrap().insert(k.clone(), binding);
        }
        let r = self.expr_repr(fx, body, want, tail);
        fx.scopes.pop();
        r
    }

    /// Each clause is a block: a failed test branches past the clause; a
    /// matched clause leaves its result and branches to the end. No clause
    /// matching traps (the interpreter raises "no Match clause" instead).
    fn match_form(
        &mut self,
        fx: &mut FnCtx,
        args: &[NodeId],
        want: Repr,
        tail: bool,
    ) -> Result<(), String> {
        let [scrut_form, clauses_form] = *args else {
            return Err("malformed Match".into());
        };
        let Node::Lst(clauses) = self.arena.node(clauses_form).clone() else {
            return Err("Match expects a list of (pattern result) clauses".into());
        };
        // The scrutinee's representation drives the pattern path: a scalar
        // kind lets a bare-name clause bind a TYPED local (5.2); a canonical
        // record (5.3) destructures by despec offsets with no boxes; the
        // boxed fallback walks boxes as before.
        let scrut_kind = self.node_scalar(scrut_form);
        let scrut_mem = self.node_mem(fx, scrut_form);
        match scrut_mem {
            Some(t) => self.expr_mem(fx, scrut_form, t, false)?,
            None => self.expr(fx, scrut_form, false)?,
        }
        let scrut = fx.local(ValType::I32);
        fx.op(I::LocalSet(scrut));
        fx.op(I::Block(BlockType::Result(repr_vt(want))));
        for &clause in &clauses {
            let pair = match self.arena.node(clause).clone() {
                Node::Tup(pair) if pair.len() == 2 => pair,
                _ => return Err("each Match clause must be a (pattern result) tuple".into()),
            };
            fx.op(I::Block(BlockType::Empty));
            fx.scopes.push(HashMap::new());
            let r = match scrut_mem {
                Some(t) => self.pattern_top_mem(fx, pair[0], scrut, t),
                None => self.pattern_top(fx, pair[0], scrut, scrut_kind),
            }
            .and_then(|()| self.expr_repr(fx, pair[1], want, tail));
            fx.scopes.pop();
            r?;
            fx.op(I::Br(1));
            fx.op(I::End);
        }
        fx.op(I::Unreachable);
        fx.op(I::End);
        Ok(())
    }

    /// A clause's top-level pattern over a canonical-layout scrutinee (5.3):
    /// a bare binder aliases the pointer (a `Repr::Mem` binding), a record
    /// pattern destructures a record layout at despec offsets, a tuple
    /// pattern destructures a tuple layout element-wise, and any other
    /// pattern rebuilds the box once and delegates to the uniform matcher
    /// (where a mismatched pattern fails, like the oracle).
    fn pattern_top_mem(
        &mut self,
        fx: &mut FnCtx,
        pat: NodeId,
        v: u32,
        t: MemTy,
    ) -> Result<(), String> {
        let ty = self.mem_tys[t as usize].clone();
        match self.arena.node(pat).clone() {
            Node::Sym(name) if name != "none" && self.local_cases.get(&name) != Some(&false) => {
                fx.scopes.last_mut().unwrap().insert(
                    name,
                    Binding::new(v, Repr::Mem(t)),
                );
                Ok(())
            }
            Node::Rec(fields) => self.pattern_mem_rec(fx, &fields, &ty, v, 0, 0),
            Node::Tup(pats) if matches!(ty, WitTy::Tuple(_)) => {
                self.pattern_mem_tup(fx, &pats, &ty, v, 0, 0)
            }
            Node::Lst(pats) if matches!(ty, WitTy::List(_)) => {
                self.pattern_mem_lst(fx, &pats, &ty, v, 0, 0)
            }
            Node::Tup(pats)
                if !pats.is_empty()
                    && ty.variant_cases().is_some()
                    && matches!(self.arena.node(pats[0]), Node::Sym(_)) =>
            {
                self.pattern_mem_var(fx, &pats, &ty, v, 0, 0)
            }
            // A bare nullary-case Sym (`none` or a DefType case registered
            // payload-less) over a canonical variant scrutinee: route it to
            // the discriminant matcher as a one-element pattern slice instead
            // of reboxing. `pattern_mem_var`'s `(0, None)` arm matches on the
            // disc alone — equivalent to the boxed matcher's "TAG_VAR + name
            // eq + payload absent" for a payload-less case (5.4 residue).
            Node::Sym(name)
                if (name == "none" || self.local_cases.get(&name) == Some(&false))
                    && ty.variant_cases().is_some() =>
            {
                self.pattern_mem_var(fx, &[pat], &ty, v, 0, 0)
            }
            Node::Qsym(..) => Err("qualified names cannot appear in patterns".into()),
            _ => {
                let l = fx.local(ValType::I32);
                self.load_from_mem(fx, &ty, v, 0)?;
                fx.op(I::LocalSet(l));
                self.pattern(fx, pat, l, 0)
            }
        }
    }

    /// A record pattern over canonical layout: each named sub-pattern
    /// destructures at its despec offset (a subset of fields, like the boxed
    /// path). A field the static type lacks can never match — the clause
    /// branches out, exactly where the interpreter's match fails.
    fn pattern_mem_rec(
        &mut self,
        fx: &mut FnCtx,
        pats: &[(String, NodeId)],
        ty: &WitTy,
        v: u32,
        off: u64,
        fail: u32,
    ) -> Result<(), String> {
        let WitTy::Record(tfs) = ty else {
            return Err("internal: record pattern over a non-record layout".into());
        };
        let offsets = record_field_offsets(ty);
        for (k, p) in pats {
            let Some(i) = tfs.iter().position(|(n, _)| n == k) else {
                // statically absent field: this clause cannot match
                fx.op(I::Br(fail));
                return Ok(());
            };
            let (o, tf) = &offsets[i];
            self.pattern_mem_field(fx, *p, tf, v, off + o, fail)?;
        }
        Ok(())
    }

    /// A tuple pattern over canonical layout: element sub-patterns
    /// destructure positionally at their despec offsets. The scrutinee is
    /// statically a tuple, so EVERY tuple pattern destructures element-wise
    /// — the oracle disambiguates tuple-vs-variant patterns by the VALUE,
    /// and a tuple value never matches a variant case — which is why a
    /// Sym-headed pattern binds its first element here instead of reading
    /// as a variant case (the boxed matcher's recorded 5.9 limitation). A
    /// length mismatch can never match: the clause branches out.
    fn pattern_mem_tup(
        &mut self,
        fx: &mut FnCtx,
        pats: &[NodeId],
        ty: &WitTy,
        v: u32,
        off: u64,
        fail: u32,
    ) -> Result<(), String> {
        let WitTy::Tuple(elems) = ty else {
            return Err("internal: tuple pattern over a non-tuple layout".into());
        };
        if pats.len() != elems.len() {
            fx.op(I::Br(fail));
            return Ok(());
        }
        for (&p, (o, tf)) in pats.iter().zip(record_field_offsets(ty)) {
            self.pattern_mem_field(fx, p, &tf, v, off + o, fail)?;
        }
        Ok(())
    }

    /// A list pattern over a canonical (ptr, len) list at `v + off` (5.5):
    /// the length must equal the pattern's arity (checked at runtime — a
    /// list's length is a value property, unlike a tuple's static arity),
    /// then each element sub-pattern destructures the packed element at its
    /// stride offset.
    fn pattern_mem_lst(
        &mut self,
        fx: &mut FnCtx,
        pats: &[NodeId],
        ty: &WitTy,
        v: u32,
        off: u64,
        fail: u32,
    ) -> Result<(), String> {
        let WitTy::List(elem) = ty else {
            return Err("internal: list pattern over a non-list layout".into());
        };
        fx.op(I::LocalGet(v));
        fx.op(I::I32Load(ma(off + 4, 2)));
        fx.op(I::I32Const(pats.len() as i32));
        fx.op(I::I32Ne);
        fx.op(I::BrIf(fail));
        let base = fx.local(ValType::I32);
        fx.op(I::LocalGet(v));
        fx.op(I::I32Load(ma(off, 2)));
        fx.op(I::LocalSet(base));
        let esz = elem_size(elem);
        for (i, &p) in pats.iter().enumerate() {
            self.pattern_mem_field(fx, p, elem, base, i as u64 * esz, fail)?;
        }
        Ok(())
    }

    /// A variant-case pattern `(case p…)` over a canonical variant layout
    /// (5.4): the case name resolves to its numeric discriminant at COMPILE
    /// time, so the match is one integer comparison — no runtime case-name
    /// strings — and the payload destructures at the canonical payload
    /// offset. A case the static type lacks, or an arity the case's payload
    /// cannot satisfy, can never match: the clause branches out.
    fn pattern_mem_var(
        &mut self,
        fx: &mut FnCtx,
        pats: &[NodeId],
        ty: &WitTy,
        v: u32,
        off: u64,
        fail: u32,
    ) -> Result<(), String> {
        let cases = ty
            .variant_cases()
            .ok_or("internal: case pattern over a non-variant layout")?;
        let Node::Sym(head) = self.arena.node(pats[0]).clone() else {
            return Err("internal: pattern_mem_var expects a Sym-headed pattern".into());
        };
        let Some(i) = cases.iter().position(|(n, _)| *n == head) else {
            fx.op(I::Br(fail));
            return Ok(());
        };
        let payload = cases[i].1.cloned();
        fx.op(I::LocalGet(v));
        fx.op(I::I32Load8U(ma(off, 0)));
        fx.op(I::I32Const(i as i32));
        fx.op(I::I32Ne);
        fx.op(I::BrIf(fail));
        let poff = variant_payload_offset(ty);
        let rest = &pats[1..];
        match (rest.len(), payload) {
            (0, None) => Ok(()),
            (1, Some(pt)) => self.pattern_mem_field(fx, rest[0], &pt, v, off + poff, fail),
            // several sub-patterns destructure a tuple payload element-wise,
            // exactly like the interpreter's `(case p q …)` rule
            (n, Some(WitTy::Tuple(es))) if n > 1 && es.len() == n => {
                let t = WitTy::Tuple(es);
                self.pattern_mem_tup(fx, rest, &t, v, off + poff, fail)
            }
            // payload/arity mismatch can never match
            _ => {
                fx.op(I::Br(fail));
                Ok(())
            }
        }
    }

    /// One canonical field against a sub-pattern: a bare binder binds the
    /// field at its natural representation (typed scalar / interior record
    /// pointer / rebuilt box), a nested record pattern recurses in place,
    /// and any other pattern reboxes just this field and delegates to the
    /// uniform matcher.
    fn pattern_mem_field(
        &mut self,
        fx: &mut FnCtx,
        pat: NodeId,
        tf: &WitTy,
        v: u32,
        off: u64,
        fail: u32,
    ) -> Result<(), String> {
        match self.arena.node(pat).clone() {
            Node::Sym(name) if name != "none" && self.local_cases.get(&name) != Some(&false) => {
                let b = self.mem_field_binding(fx, tf, v, off)?;
                fx.scopes.last_mut().unwrap().insert(name, b);
                Ok(())
            }
            Node::Rec(fields) if matches!(tf, WitTy::Record(_)) => {
                self.pattern_mem_rec(fx, &fields, tf, v, off, fail)
            }
            Node::Tup(pats) if matches!(tf, WitTy::Tuple(_)) => {
                self.pattern_mem_tup(fx, &pats, tf, v, off, fail)
            }
            Node::Lst(pats) if matches!(tf, WitTy::List(_)) => {
                self.pattern_mem_lst(fx, &pats, tf, v, off, fail)
            }
            Node::Tup(pats)
                if !pats.is_empty()
                    && tf.variant_cases().is_some()
                    && matches!(self.arena.node(pats[0]), Node::Sym(_)) =>
            {
                self.pattern_mem_var(fx, &pats, tf, v, off, fail)
            }
            // A bare nullary-case Sym (`none` or a DefType case registered
            // payload-less) as a NESTED field pattern over a canonical variant
            // field: route it to the discriminant matcher as a one-element
            // pattern slice instead of reboxing the field and running the boxed
            // string-compare matcher. Mirrors pattern_top_mem's arm (5.4 residue
            // / TAG_VAR consumer shrink); behavior-preserving.
            Node::Sym(name)
                if (name == "none" || self.local_cases.get(&name) == Some(&false))
                    && tf.variant_cases().is_some() =>
            {
                self.pattern_mem_var(fx, &[pat], tf, v, off, fail)
            }
            Node::Qsym(..) => Err("qualified names cannot appear in patterns".into()),
            _ => {
                let l = fx.local(ValType::I32);
                self.load_from_mem(fx, tf, v, off)?;
                fx.op(I::LocalSet(l));
                self.pattern(fx, pat, l, fail)
            }
        }
    }

    /// Bind one canonical field at its natural representation: scalar fields
    /// load unboxed into typed locals (widening to the interpreter's value
    /// domains), a nested record or tuple binds an interior pointer
    /// (headerless layout — 5.1), everything else rebuilds its box.
    fn mem_field_binding(
        &mut self,
        fx: &mut FnCtx,
        tf: &WitTy,
        v: u32,
        off: u64,
    ) -> Result<Binding, String> {
        let scalar = |fx: &mut FnCtx, vt: ValType, kind: Scalar| {
            let l = fx.local(vt);
            fx.op(I::LocalSet(l));
            Binding::new(l, Repr::Scalar(kind))
        };
        Ok(match tf {
            WitTy::Bool => {
                fx.op(I::LocalGet(v));
                fx.op(I::I32Load8U(ma(off, 0)));
                scalar(fx, ValType::I32, Scalar::Bool)
            }
            WitTy::Char => {
                fx.op(I::LocalGet(v));
                fx.op(I::I32Load(ma(off, 2)));
                fx.op(I::I64ExtendI32U);
                scalar(fx, ValType::I64, Scalar::Char)
            }
            WitTy::IntS(w) => {
                fx.op(I::LocalGet(v));
                match *w {
                    1 => fx.op(I::I32Load8S(ma(off, 0))),
                    2 => fx.op(I::I32Load16S(ma(off, 1))),
                    _ => fx.op(I::I32Load(ma(off, 2))),
                }
                fx.op(I::I64ExtendI32S);
                scalar(fx, ValType::I64, Scalar::Int)
            }
            WitTy::IntU(w) => {
                fx.op(I::LocalGet(v));
                match *w {
                    1 => fx.op(I::I32Load8U(ma(off, 0))),
                    2 => fx.op(I::I32Load16U(ma(off, 1))),
                    _ => fx.op(I::I32Load(ma(off, 2))),
                }
                fx.op(I::I64ExtendI32U);
                scalar(fx, ValType::I64, Scalar::Int)
            }
            WitTy::S64 => {
                fx.op(I::LocalGet(v));
                fx.op(I::I64Load(ma(off, 3)));
                scalar(fx, ValType::I64, Scalar::Int)
            }
            WitTy::F64 => {
                fx.op(I::LocalGet(v));
                fx.op(I::F64Load(ma(off, 3)));
                scalar(fx, ValType::F64, Scalar::Float)
            }
            WitTy::Record(_) | WitTy::Tuple(_) | WitTy::Str | WitTy::List(_) => {
                fx.op(I::LocalGet(v));
                if off > 0 {
                    fx.op(I::I32Const(off as i32));
                    fx.op(I::I32Add);
                }
                let l = fx.local(ValType::I32);
                fx.op(I::LocalSet(l));
                Binding::new(l, Repr::Mem(self.mem_ty(tf)))
            }
            _ => {
                let l = fx.local(ValType::I32);
                self.load_from_mem(fx, tf, v, off)?;
                fx.op(I::LocalSet(l));
                Binding::boxed(l)
            }
        })
    }

    /// A clause's top-level pattern: like [`Self::pattern`], except a bare
    /// binder over a scrutinee with a known scalar kind binds a TYPED local
    /// (unboxed once, at bind time) instead of a box pointer.
    fn pattern_top(
        &mut self,
        fx: &mut FnCtx,
        pat: NodeId,
        v: u32,
        scrut_kind: Option<Scalar>,
    ) -> Result<(), String> {
        if let Node::Sym(name) = self.arena.node(pat).clone()
            && name != "none"
            && self.local_cases.get(&name) != Some(&false)
            && let Some(kind) = scrut_kind
        {
            let l = fx.local(repr_vt(Repr::Scalar(kind)));
            fx.op(I::LocalGet(v));
            self.unbox_scalar(fx, kind);
            fx.op(I::LocalSet(l));
            fx.scopes.last_mut().unwrap().insert(
                name,
                Binding::new(l, Repr::Scalar(kind)),
            );
            return Ok(());
        }
        self.pattern(fx, pat, v, 0)
    }

    /// Compile a pattern test against the box in local `v`; on mismatch branch
    /// `fail` levels out (the enclosing clause block). Names bind into the
    /// current scope. Nested patterns keep `fail` because no blocks are opened.
    fn pattern(&mut self, fx: &mut FnCtx, pat: NodeId, v: u32, fail: u32) -> Result<(), String> {
        match self.arena.node(pat).clone() {
            // `none` and the nullary cases of local `DefType` variants/enums
            // match by equality; every other bare name binds. Mirrors the
            // interpreter, which keys this off names bound to a payload-less
            // variant (`none` builtin, DefType case bindings — 4.1).
            Node::Sym(name) if name == "none" || self.local_cases.get(&name) == Some(&false) => {
                let naddr = self.intern_str(&name);
                fx.op(I::LocalGet(v));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_VAR));
                fx.op(I::I32Ne);
                fx.op(I::BrIf(fail));
                fx.op(I::LocalGet(v));
                fx.op(I::I32Load(ma(8, 2))); // payload must be absent
                fx.op(I::BrIf(fail));
                fx.op(I::LocalGet(v));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::I32Const(naddr as i32)); // case name must equal
                fx.op(I::Call(self.h.eq_raw));
                fx.op(I::I32Eqz);
                fx.op(I::BrIf(fail));
                Ok(())
            }
            Node::Sym(name) => {
                let l = fx.local(ValType::I32);
                fx.op(I::LocalGet(v));
                fx.op(I::LocalSet(l));
                fx.scopes
                    .last_mut()
                    .unwrap()
                    .insert(name, Binding::boxed(l));
                Ok(())
            }
            // Literal patterns match by structural equality — the same
            // eq_raw the `eq` builtin uses, so char (codepoint) and flags
            // (set-name list) literals match exactly like the interpreter's
            // `match_pattern` (5.9).
            // Float literals are rejected as patterns at check time (2.1
            // proposal 2); the backend refuses them too, matching the
            // interpreter's `match_pattern`, so the two never diverge.
            Node::Dec(_) => Err("a float literal cannot be a Match pattern".into()),
            Node::Int(_)
            | Node::Bool(_)
            | Node::Str(_)
            | Node::Char(_)
            | Node::Flg(_) => {
                fx.op(I::LocalGet(v));
                self.expr(fx, pat, false)?;
                fx.op(I::Call(self.h.eq_raw));
                fx.op(I::I32Eqz);
                fx.op(I::BrIf(fail));
                Ok(())
            }
            // the interpreter's exact wording, for error parity
            Node::Qsym(..) => Err("qualified names cannot appear in patterns".into()),
            Node::Lst(pats) => self.seq_pattern(fx, &pats, v, fail, TAG_LIST),
            // A tuple pattern is disambiguated by its first element: a `Sym`
            // head is a variant-case pattern (`ok(x)`, `some(x)`, `none`, …);
            // anything else is a tuple destructure. (Limitation: a tuple
            // pattern whose first element is a bare name is always read as a
            // variant case here, never as a tuple binding the first element.)
            Node::Tup(pats) => match pats.first().map(|&p| self.arena.node(p).clone()) {
                Some(Node::Sym(case)) => {
                    let caddr = self.intern_str(&case);
                    fx.op(I::LocalGet(v));
                    fx.op(I::I32Load(ma(0, 2)));
                    fx.op(I::I32Const(TAG_VAR));
                    fx.op(I::I32Ne);
                    fx.op(I::BrIf(fail));
                    fx.op(I::LocalGet(v));
                    fx.op(I::I32Load(ma(4, 2)));
                    fx.op(I::I32Const(caddr as i32));
                    fx.op(I::Call(self.h.eq_raw));
                    fx.op(I::I32Eqz);
                    fx.op(I::BrIf(fail));
                    match pats.len() {
                        1 => {
                            // payload must be absent
                            fx.op(I::LocalGet(v));
                            fx.op(I::I32Load(ma(8, 2)));
                            fx.op(I::BrIf(fail));
                            Ok(())
                        }
                        2 => {
                            let inner = fx.local(ValType::I32);
                            fx.op(I::LocalGet(v));
                            fx.op(I::I32Load(ma(8, 2)));
                            fx.op(I::LocalTee(inner));
                            fx.op(I::I32Eqz);
                            fx.op(I::BrIf(fail));
                            self.pattern(fx, pats[1], inner, fail)
                        }
                        _ => {
                            // payload is a tuple; destructure it element-wise
                            let inner = fx.local(ValType::I32);
                            fx.op(I::LocalGet(v));
                            fx.op(I::I32Load(ma(8, 2)));
                            fx.op(I::LocalTee(inner));
                            fx.op(I::I32Eqz);
                            fx.op(I::BrIf(fail));
                            self.seq_pattern(fx, &pats[1..], inner, fail, TAG_TUP)
                        }
                    }
                }
                _ => self.seq_pattern(fx, &pats, v, fail, TAG_TUP),
            },
            Node::Rec(fields) => {
                fx.op(I::LocalGet(v));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_REC));
                fx.op(I::I32Ne);
                fx.op(I::BrIf(fail));
                // A record pattern matches a subset of fields: each named field
                // must be present (rec_get returns 0 when absent) and its
                // sub-pattern must match. Extra value fields are ignored.
                for (k, p) in &fields {
                    let kaddr = self.intern_str(k);
                    let elem = fx.local(ValType::I32);
                    fx.op(I::LocalGet(v));
                    fx.op(I::I32Const(kaddr as i32));
                    fx.op(I::Call(self.h.rec_get));
                    fx.op(I::LocalTee(elem));
                    fx.op(I::I32Eqz);
                    fx.op(I::BrIf(fail));
                    self.pattern(fx, *p, elem, fail)?;
                }
                Ok(())
            }
        }
    }

    /// List/tuple pattern: tag + length check, then element sub-patterns.
    fn seq_pattern(
        &mut self,
        fx: &mut FnCtx,
        pats: &[NodeId],
        v: u32,
        fail: u32,
        tag: i32,
    ) -> Result<(), String> {
        fx.op(I::LocalGet(v));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(tag));
        fx.op(I::I32Ne);
        fx.op(I::BrIf(fail));
        fx.op(I::LocalGet(v));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Const(pats.len() as i32));
        fx.op(I::I32Ne);
        fx.op(I::BrIf(fail));
        for (i, &p) in pats.iter().enumerate() {
            let elem = fx.local(ValType::I32);
            fx.op(I::LocalGet(v));
            fx.op(I::I32Load(ma(8 + 4 * i as u64, 2)));
            fx.op(I::LocalSet(elem));
            self.pattern(fx, p, elem, fail)?;
        }
        Ok(())
    }

    /// Mirror of the interpreter's §4.2 argument-binding rule, at compile time.
    /// `args` are the call's argument forms (`Tup[head, …args]`).
    fn bind_args(&self, args: &[NodeId], params: &[String]) -> Result<BoundArgs, String> {
        // named: a single record arg whose keys are exactly the parameters
        if let [only] = args
            && let Node::Rec(fields) = self.arena.node(*only)
        {
            let mut keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
            let mut want: Vec<&str> = params.iter().map(|s| s.as_str()).collect();
            keys.sort();
            want.sort();
            if keys == want {
                let map: HashMap<&str, NodeId> =
                    fields.iter().map(|(k, v)| (k.as_str(), *v)).collect();
                return Ok(BoundArgs::PerParam(
                    params.iter().map(|p| map[p.as_str()]).collect(),
                ));
            }
        }
        // positional: one arg per parameter (covers the scalar 1/1 and 0/0 cases)
        if args.len() == params.len() {
            return Ok(BoundArgs::PerParam(args.to_vec()));
        }
        // a sole parameter receives the whole bundle as a tuple
        if params.len() == 1 {
            return Ok(BoundArgs::Bundle);
        }
        Err(format!(
            "payload does not match parameters ({})",
            params.join(", ")
        ))
    }

    /// Call an internal def, producing the result in representation `want`
    /// (`None` = boxed). Arguments are emitted per the callee's repr
    /// signature — typed slots receive unboxed scalars directly. A tail call
    /// stays a `return_call` when caller and callee agree on the result
    /// representation; a representation seam (box/unbox/convert after the
    /// call) forces a plain call.
    fn internal_call(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        args: &[NodeId],
        want: Repr,
        tail: bool,
    ) -> Result<(), String> {
        let (idx, params, sig) = self.funcs[name].clone();
        match self.bind_args(args, &params)? {
            BoundArgs::PerParam(nodes) => {
                for (a, slot) in nodes.into_iter().zip(sig.params.iter()) {
                    self.expr_repr(fx, a, *slot, false)?;
                }
            }
            BoundArgs::Bundle => {
                self.seq_box(fx, args, TAG_TUP)?;
                // sole parameter; a typed slot unboxes the bundle — trapping,
                // as the interpreter's use of a mis-bound bundle would error
                if let [Repr::Scalar(k)] = sig.params[..] {
                    self.unbox_scalar(fx, k);
                }
            }
        }
        if tail && want == sig.result {
            fx.op(I::ReturnCall(idx));
            return Ok(());
        }
        fx.op(I::Call(idx));
        // representation seam on the result
        match (sig.result, want) {
            (a, b) if a == b => {}
            (Repr::Scalar(Scalar::Int), Repr::Scalar(Scalar::Float)) => {
                fx.op(I::F64ConvertI64S);
            }
            (Repr::Scalar(k), Repr::Boxed) => self.box_scalar(fx, k),
            (Repr::Boxed, Repr::Scalar(k)) => self.unbox_scalar(fx, k),
            (Repr::Mem(t), Repr::Boxed) => {
                let l = fx.local(ValType::I32);
                fx.op(I::LocalSet(l));
                let ty = self.mem_tys[t as usize].clone();
                self.load_from_mem(fx, &ty, l, 0)?;
            }
            (Repr::Mem(t), Repr::Scalar(k)) => {
                // rebuild the box, then unbox: traps exactly where the boxed
                // path would (a record where a scalar is required)
                let l = fx.local(ValType::I32);
                fx.op(I::LocalSet(l));
                let ty = self.mem_tys[t as usize].clone();
                self.load_from_mem(fx, &ty, l, 0)?;
                self.unbox_scalar(fx, k);
            }
            (a, b) => {
                return Err(format!(
                    "internal: `{name}` returns {a:?} where {b:?} is expected"
                ));
            }
        }
        Ok(())
    }

    /// The [`Dep`] an import alias resolves to, if it names one in the build set.
    fn dep_for_alias(&self, alias: &str) -> Result<&Dep, String> {
        let imp = self
            .info
            .imports
            .iter()
            .find(|i| i.alias == alias)
            .ok_or(format!("unknown import alias `{alias}`"))?;
        self.deps.get(&imp.package).ok_or(format!(
            "dependency `{}` is not in the build set",
            imp.package
        ))
    }

    fn dep_call(
        &mut self,
        fx: &mut FnCtx,
        alias: &str,
        fname: &str,
        args: &[NodeId],
        want_mem: Option<MemTy>,
    ) -> Result<(), String> {
        // A functor op (`pts/new`, `pts/add`, `pts/contains`, `pts/size`) is not a
        // runtime import: it routes to the locally-emitted `set` resource's core
        // funcs (`ResourceFns`, indices reserved in `em.functor_fns`). The mapping
        // mirrors the interpreter's `bind_functor` (`builtins.rs`): `new`→ctor,
        // `add`/`contains`/`size`→methods.
        //
        // HANDLE CONVENTION (step 04 ABI decision; see `summaries/04-routing.typ`):
        // a `set` Wavelet value carries the OWN handle minted by the constructor's
        // `resource.new`, boxed as an int box (TAG_INT) — the same opaque-handle
        // carriage `lower`/`lift` already use for `WitTy::Handle`, so the export
        // boundary (`nearest-set -> own<set>`) needs no special-casing: it just
        // unboxes the i32. A method's core body, however, receives the REP as
        // `self` (summary 01: the canonical ABI hands an exported resource's method
        // the rep directly). So intra-guest we convert handle→rep with the
        // `[resource-rep]set` intrinsic before each method call. (This is the
        // alternative to carrying the rep guest-side and minting at the boundary;
        // it keeps `lower`/`lift` untouched and reuses the ctor verbatim.)
        if let Some(inst) = self.info.functors.iter().find(|f| f.alias == alias) {
            let fns = *self.functor_fns.get(alias).ok_or_else(|| {
                format!("internal error: no emitted `set` resource for functor alias `{alias}`")
            })?;
            match fname {
                "new" => {
                    if !args.is_empty() {
                        return Err(format!("`{alias}/new` takes no arguments"));
                    }
                    fx.op(I::Call(fns.ctor)); // () -> i32 own handle
                    self.lift(fx, &WitTy::Handle); // box the handle as the set value
                }
                "add" | "contains" | "size" => {
                    let want = if fname == "size" { 1 } else { 2 };
                    if args.len() != want {
                        return Err(format!(
                            "`{alias}/{fname}` takes {want} argument{}",
                            if want == 1 { "" } else { "s" }
                        ));
                    }
                    // arg 0 is the set: unbox its handle, recover the rep (the
                    // method's `self`), and stash it below the flattened value args.
                    let rep = fx.local(ValType::I32);
                    self.expr(fx, args[0], false)?;
                    fx.op(I::Call(self.h.unbox_int));
                    fx.op(I::I32WrapI64);
                    fx.op(I::Call(fns.rep_import)); // handle -> rep
                    fx.op(I::LocalSet(rep));
                    fx.op(I::LocalGet(rep));
                    if fname != "size" {
                        // the element value, flattened in canonical-ABI order
                        let elem = wit_ty(&inst.elem, &self.type_env)?;
                        self.expr(fx, args[1], false)?;
                        self.lower(fx, &elem)?;
                    }
                    match fname {
                        "add" => {
                            fx.op(I::Call(fns.add)); // (rep, <elem>) -> ()
                            fx.op(I::I32Const(self.unit_addr() as i32));
                        }
                        "contains" => {
                            fx.op(I::Call(fns.contains)); // -> i32 0/1
                            self.lift(fx, &WitTy::Bool);
                        }
                        _ => {
                            fx.op(I::Call(fns.size)); // -> i32 u32
                            self.lift(fx, &WitTy::IntU(4));
                        }
                    }
                }
                other => {
                    return Err(format!(
                        "functor `set` (alias `{alias}`) has no op `{other}`; \
                         expected new / add / contains / size"
                    ));
                }
            }
            return Ok(());
        }
        // A user-declared resource (4.5): `counter/next(c)` (method) or
        // `counter/sum(vs)` (static). A resource value is carried guest-internally
        // as its rep (the `New` cell), so `self` is simply the first argument — a
        // method or static is therefore an ordinary internal call. The own/borrow
        // handle conversions happen only at the component boundary.
        if let Some(ur) = self.user_res.get(alias).cloned() {
            let mname = format!("{alias}/{fname}");
            if ur.methods.contains(fname) || ur.statics.contains(fname) {
                return self.internal_call(fx, &mname, args, Repr::Boxed, false);
            }
            return Err(format!("resource `{alias}` has no member `{fname}`"));
        }
        let imp = self
            .info
            .imports
            .iter()
            .find(|i| i.alias == alias)
            .ok_or(format!("unknown import alias `{alias}`"))?;
        let dep = self.deps.get(&imp.package).ok_or(format!(
            "dependency `{}` is not in the build set",
            imp.package
        ))?;
        let iface = import_iface(&imp.path);
        // Resolve freestanding names directly, and resource operations
        // (`[method]`/`[static]`/`[constructor]`/`[resource-drop]`) by their
        // bare op name. A name that is no function but IS a case of one of the
        // dep's variant/enum types is a case constructor call (4.1).
        let sig = match resolve_dep_func(dep, &iface, fname) {
            Ok(sig) => sig.clone(),
            Err(e) => {
                return match dep_case(dep, fname) {
                    Some(true) if !args.is_empty() => self.var_box(fx, fname, args),
                    Some(true) => Err(format!(
                        "variant case `{alias}/{fname}` takes a payload, got no arguments"
                    )),
                    Some(false) if args.is_empty() => {
                        let addr = self.none_like_box(fname);
                        fx.op(I::I32Const(addr as i32));
                        Ok(())
                    }
                    Some(false) => Err(format!(
                        "variant case `{alias}/{fname}` is not callable (use the bare name)"
                    )),
                    None => Err(e),
                };
            }
        };
        let module = versioned_iface(&dep.package, &iface);
        // The host import is keyed by the *mangled* WIT name (`sig.name`), which
        // is what the import-signature loop declares and what `wit-component`
        // re-validates against the WIT.
        let fidx = self.import_idx(&module, &sig.name);

        let param_names: Vec<String> = sig.params.iter().map(|(n, _)| n.clone()).collect();
        let arg_nodes = match self.bind_args(args, &param_names)? {
            BoundArgs::PerParam(nodes) => nodes,
            BoundArgs::Bundle => {
                // 5.10: several arguments bundle into the sole parameter as a
                // tuple value — exactly how the interpreter binds a call's
                // payload to one parameter. Build the tuple box and lower it
                // against the parameter's (tuple) WIT type here; the empty
                // node list below then has nothing left to lower.
                self.seq_box(fx, args, TAG_TUP)?;
                let (_, t) = &sig.params[0];
                let ty = wit_ty(t, &self.type_env)?;
                self.lower(fx, &ty)?;
                Vec::new()
            }
        };
        for (a, (_, t)) in arg_nodes.iter().zip(&sig.params) {
            self.expr(fx, *a, false)?;
            let pty = wit_ty(t, &self.type_env)?;
            self.lower(fx, &pty)?;
        }
        if want_mem.is_some() && !matches!(flat_result(&sig, &self.type_env), Ok(FlatRes::Retptr)) {
            return Err("internal: Mem repr requested for a non-retptr dep call (5.3)".into());
        }
        match flat_result(&sig, &self.type_env)? {
            FlatRes::None => {
                fx.op(I::Call(fidx));
                fx.op(I::I32Const(self.unit_addr() as i32));
            }
            FlatRes::One(t) => {
                fx.op(I::Call(fidx));
                self.lift(fx, &t);
            }
            FlatRes::Retptr => {
                let rty = wit_ty(sig.result.as_deref().unwrap(), &self.type_env)?;
                if matches!(
                    rty,
                    WitTy::Record(_)
                        | WitTy::Tuple(_)
                        | WitTy::Option(_)
                        | WitTy::Result(..)
                        | WitTy::Variant(_)
                ) || (want_mem.is_some() && matches!(rty, WitTy::Str | WitTy::List(_)))
                {
                    // allocate a result area sized to the value, pass it as the
                    // canonical retptr, then read the value back out of it —
                    // or, when the caller wants this canonical layout (5.3),
                    // the area IS the value: no lift, no boxes. A string/list
                    // result's canonical form is the (ptr, len) pair the area
                    // carries (5.5), so the same fast path applies.
                    let area = fx.local(ValType::I32);
                    fx.op(I::I32Const(size_of(&rty) as i32));
                    fx.op(I::Call(self.h.alloc));
                    fx.op(I::LocalTee(area));
                    fx.op(I::Call(fidx));
                    match want_mem {
                        Some(t) if self.mem_tys[t as usize] == rty => {
                            fx.op(I::LocalGet(area));
                        }
                        Some(_) => {
                            return Err(
                                "internal: dep call's canonical layout does not match                                  the requested Mem type (5.3)"
                                    .into(),
                            );
                        }
                        None => self.load_from_mem(fx, &rty, area, 0)?,
                    }
                } else {
                    fx.op(I::I32Const(SCRATCH));
                    fx.op(I::Call(fidx));
                    // (ptr, len) written at the scratch area
                    let p = fx.local(ValType::I32);
                    let l = fx.local(ValType::I32);
                    fx.op(I::I32Const(SCRATCH));
                    fx.op(I::I32Load(ma(0, 2)));
                    fx.op(I::LocalSet(p));
                    fx.op(I::I32Const(SCRATCH));
                    fx.op(I::I32Load(ma(4, 2)));
                    fx.op(I::LocalSet(l));
                    match rty {
                        WitTy::List(elem) => self.lift_list(fx, p, l, &elem)?,
                        _ => {
                            fx.op(I::LocalGet(p));
                            fx.op(I::LocalGet(l));
                            fx.op(I::Call(self.h.box_str));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// box on stack → flat value(s) on stack
    fn lower(&mut self, fx: &mut FnCtx, ty: &WitTy) -> Result<(), String> {
        match ty {
            WitTy::Bool => fx.op(I::Call(self.h.truthy)),
            WitTy::Char => {
                fx.op(I::Call(self.h.unbox_char));
                fx.op(I::I32WrapI64);
            }
            WitTy::IntS(_) | WitTy::IntU(_) | WitTy::Handle => {
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::I32WrapI64);
            }
            WitTy::S64 => fx.op(I::Call(self.h.unbox_int)),
            WitTy::F32 => {
                // boundary-only f32: internally an f64 Dec box, demoted here
                fx.op(I::Call(self.h.unbox_dec));
                fx.op(I::F32DemoteF64);
            }
            WitTy::F64 => fx.op(I::Call(self.h.unbox_dec)),
            WitTy::Str => {
                let t = fx.local(ValType::I32);
                fx.op(I::LocalTee(t));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(t));
                fx.op(I::I32Load(ma(4, 2)));
            }
            WitTy::List(elem) if is_byte_elem(elem) => {
                // `list<u8>` accepts a Wavelet string directly: its bytes are
                // already contiguous (a string box is `[tag, len, bytes…]`), so a
                // string lowers to `(box+8, len)` with no copy. A real list box
                // still goes through the element-by-element builder. The branch is
                // on the box tag so e.g. an http body can be written from a string
                // (`blocking-write-and-flush` takes `list<u8>`).
                let b = fx.local(ValType::I32);
                fx.op(I::LocalSet(b));
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_STR));
                fx.op(I::I32Eq);
                let rty = self.ty_idx(vec![], vec![ValType::I32, ValType::I32]);
                fx.op(I::If(BlockType::FunctionType(rty)));
                // string box → (ptr = box+8, len = load@4)
                fx.op(I::LocalGet(b));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::Else);
                fx.op(I::LocalGet(b));
                self.lower_list(fx, elem)?;
                fx.op(I::End);
            }
            WitTy::List(elem) => self.lower_list(fx, elem)?,
            WitTy::Record(fields) => {
                // record box on stack → field flats pushed in declaration order
                let b = fx.local(ValType::I32);
                fx.op(I::LocalSet(b));
                for (k, ft) in fields {
                    let kaddr = self.intern_str(k);
                    fx.op(I::LocalGet(b));
                    fx.op(I::I32Const(kaddr as i32));
                    fx.op(I::Call(self.h.rec_get));
                    self.lower(fx, ft)?;
                }
            }
            WitTy::Tuple(elems) => {
                // TAG_TUP box on stack → element flats in order (element boxes
                // live at @8+4i, the list/tuple layout)
                let b = fx.local(ValType::I32);
                fx.op(I::LocalSet(b));
                for (i, et) in elems.iter().enumerate() {
                    fx.op(I::LocalGet(b));
                    fx.op(I::I32Load(ma(8 + 4 * i as u64, 2)));
                    self.lower(fx, et)?;
                }
            }
            WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_) => {
                // variant box → [disc i32] ++ joined payload flats; every arm
                // produces the same flat shape (zero-padded where shorter). A
                // chain of `case == name ? lower(case) : …` over all cases.
                let cases: Vec<(String, Option<WitTy>)> = ty
                    .variant_cases()
                    .unwrap()
                    .into_iter()
                    .map(|(n, p)| (n.to_string(), p.cloned()))
                    .collect();
                let full = flat(ty);
                let joined: Vec<ValType> = full[1..].to_vec();
                let resty = self.ty_idx(vec![], full);
                let b = fx.local(ValType::I32);
                fx.op(I::LocalSet(b));
                self.lower_variant_chain(fx, b, &cases, &joined, resty, 0)?;
            }
            WitTy::Enum(cases) => {
                // payload-less variant box → discriminant i32. Compare the box's
                // case-name against each enum case, yielding its ordinal.
                let resty = self.ty_idx(vec![], vec![ValType::I32]);
                let b = fx.local(ValType::I32);
                fx.op(I::LocalSet(b));
                self.lower_enum_chain(fx, b, cases, resty, 0)?;
            }
            WitTy::Flags(names) => {
                // flags value box (TAG_FLG: the set names, like the
                // interpreter's `Value::Flg`) → bitset i32: OR `1<<i` for
                // each declared member present in the box. Names the type
                // does not declare contribute no bit (the checker rejects
                // them statically for typed literals).
                let b = fx.local(ValType::I32);
                let acc = fx.local(ValType::I32);
                fx.op(I::LocalSet(b));
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(acc));
                for (i, name) in names.iter().enumerate() {
                    let kaddr = self.intern_str(name);
                    let needle = fx.local(ValType::I32);
                    fx.op(I::I32Const(kaddr as i32));
                    fx.op(I::LocalSet(needle));
                    fx.op(I::LocalGet(acc));
                    emit_list_contains(self, fx, b, needle);
                    fx.op(I::I32Const(i as i32));
                    fx.op(I::I32Shl);
                    fx.op(I::I32Or);
                    fx.op(I::LocalSet(acc));
                }
                fx.op(I::LocalGet(acc));
            }
        }
        Ok(())
    }

    /// Lower an N-case variant box to `[disc] ++ joined`: emit
    /// `name==cases[i] ? lower(i) : <recurse i+1>`; the last case is the else.
    fn lower_variant_chain(
        &mut self,
        fx: &mut FnCtx,
        b: u32,
        cases: &[(String, Option<WitTy>)],
        joined: &[ValType],
        resty: u32,
        i: usize,
    ) -> Result<(), String> {
        if i + 1 == cases.len() {
            return self.lower_variant_case(fx, b, i as i32, cases[i].1.as_ref(), joined);
        }
        let naddr = self.intern_str(&cases[i].0);
        fx.op(I::LocalGet(b));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Const(naddr as i32));
        fx.op(I::Call(self.h.eq_raw));
        fx.op(I::If(BlockType::FunctionType(resty)));
        self.lower_variant_case(fx, b, i as i32, cases[i].1.as_ref(), joined)?;
        fx.op(I::Else);
        self.lower_variant_chain(fx, b, cases, joined, resty, i + 1)?;
        fx.op(I::End);
        Ok(())
    }

    /// Lower an N-case enum box to its discriminant: emit
    /// `name==cases[i] ? i : <recurse i+1>`; the last case is the else.
    fn lower_enum_chain(
        &mut self,
        fx: &mut FnCtx,
        b: u32,
        cases: &[String],
        resty: u32,
        i: usize,
    ) -> Result<(), String> {
        if i + 1 == cases.len() {
            fx.op(I::I32Const(i as i32));
            return Ok(());
        }
        let naddr = self.intern_str(&cases[i]);
        fx.op(I::LocalGet(b));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Const(naddr as i32));
        fx.op(I::Call(self.h.eq_raw));
        fx.op(I::If(BlockType::FunctionType(resty)));
        fx.op(I::I32Const(i as i32));
        fx.op(I::Else);
        self.lower_enum_chain(fx, b, cases, resty, i + 1)?;
        fx.op(I::End);
        Ok(())
    }

    /// One arm of a lowered option/result: push the discriminant, the payload's
    /// flats (if any) widened into the joined slot types, then zero-pad the
    /// remaining joined positions.
    ///
    /// Canonical-ABI variant flattening widens each arm's payload to a shared
    /// union (`join`), so a payload flat (e.g. `i32`) may have to be coerced into
    /// a wider joined slot (e.g. `i64`). We materialise the payload flats into
    /// payload-typed locals first, then re-push each coerced to its joined slot.
    fn lower_variant_case(
        &mut self,
        fx: &mut FnCtx,
        b: u32,
        disc: i32,
        pay: Option<&WitTy>,
        joined: &[ValType],
    ) -> Result<(), String> {
        fx.op(I::I32Const(disc));
        let consumed = match pay {
            Some(pt) => {
                let pflat = flat(pt);
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(8, 2)));
                self.lower(fx, pt)?;
                // Pop the payload's flats (last-first) into payload-typed locals.
                let locals: Vec<u32> = pflat.iter().rev().map(|&vt| fx.local(vt)).collect();
                for &l in &locals {
                    fx.op(I::LocalSet(l));
                }
                // Re-push in order, widening each into its joined slot type.
                for (i, &have) in pflat.iter().enumerate() {
                    fx.op(I::LocalGet(locals[pflat.len() - 1 - i]));
                    coerce_flat_to(fx, have, joined[i]);
                }
                pflat.len()
            }
            None => 0,
        };
        for &vt in &joined[consumed..] {
            push_zero(fx, vt);
        }
        Ok(())
    }

    /// list box on stack → canonical (ptr, len) on stack: a fresh buffer of
    /// `len` elements, each stored at its canonical size/stride.
    fn lower_list(&mut self, fx: &mut FnCtx, elem: &WitTy) -> Result<(), String> {
        use ValType::I32;
        let size = elem_size(elem);
        let b = fx.local(I32);
        let n = fx.local(I32);
        let buf = fx.local(I32);
        let i = fx.local(I32);
        fx.op(I::LocalSet(b));
        fx.op(I::LocalGet(b));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(n));
        fx.op(I::LocalGet(n));
        fx.op(I::I32Const(size as i32));
        fx.op(I::I32Mul);
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(buf));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(n));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // dst = buf + i*size ; store the i-th element there in canonical layout
        let dst = fx.local(I32);
        fx.op(I::LocalGet(buf));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(size as i32));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalSet(dst));
        let elembox = fx.local(I32);
        fx.op(I::LocalGet(b));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalSet(elembox));
        self.store_to_mem(fx, elem, elembox, dst, 0)?;
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(buf));
        fx.op(I::LocalGet(n));
        Ok(())
    }

    /// canonical (ptr, len) in the given locals → list box on stack
    fn lift_list(
        &mut self,
        fx: &mut FnCtx,
        ptr: u32,
        len: u32,
        elem: &WitTy,
    ) -> Result<(), String> {
        use ValType::I32;
        let size = elem_size(elem);
        let lst = fx.local(I32);
        let i = fx.local(I32);
        fx.op(I::I32Const(8));
        fx.op(I::LocalGet(len));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(lst));
        fx.op(I::LocalGet(lst));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(lst));
        fx.op(I::LocalGet(len));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(len));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        let src = fx.local(I32);
        fx.op(I::LocalGet(ptr));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(size as i32));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalSet(src));
        // destination slot address, then the lifted element box
        fx.op(I::LocalGet(lst));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        self.load_from_mem(fx, elem, src, 0)?;
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(lst));
        Ok(())
    }

    /// flat value on stack → box on stack (single-flat types only)
    fn lift(&mut self, fx: &mut FnCtx, ty: &WitTy) {
        match ty {
            WitTy::Bool => fx.op(I::Call(self.h.box_bool)),
            WitTy::IntS(_) => {
                fx.op(I::I64ExtendI32S);
                fx.op(I::Call(self.h.box_int));
            }
            WitTy::Char => {
                fx.op(I::I64ExtendI32U);
                self.box_char(fx);
            }
            WitTy::IntU(_) | WitTy::Handle => {
                fx.op(I::I64ExtendI32U);
                fx.op(I::Call(self.h.box_int));
            }
            WitTy::S64 => fx.op(I::Call(self.h.box_int)),
            WitTy::F32 => {
                fx.op(I::F64PromoteF32);
                fx.op(I::Call(self.h.box_dec));
            }
            WitTy::F64 => fx.op(I::Call(self.h.box_dec)),
            WitTy::Enum(cases) => {
                // disc i32 on stack → payload-less variant box of the i-th case.
                let d = fx.local(ValType::I32);
                fx.op(I::LocalSet(d));
                self.lift_enum(fx, d, cases, 0);
            }
            WitTy::Flags(names) => {
                // bitset i32 on stack → record box of name → bool (set/clear).
                let v = fx.local(ValType::I32);
                fx.op(I::LocalSet(v));
                self.lift_flags(fx, v, names);
            }
            // A variant whose every case is payload-less (e.g. a bare
            // `result`, 4.2) flattens to its lone i32 discriminant — exactly
            // an enum's shape, so lift it the same way.
            WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_) if flat_len(ty) == 1 => {
                let cases: Vec<String> = ty
                    .variant_cases()
                    .expect("variant-shaped type has cases")
                    .iter()
                    .map(|(n, _)| n.to_string())
                    .collect();
                let d = fx.local(ValType::I32);
                fx.op(I::LocalSet(d));
                self.lift_enum(fx, d, &cases, 0);
            }
            WitTy::Str
            | WitTy::List(_)
            | WitTy::Record(_)
            | WitTy::Tuple(_)
            | WitTy::Option(_)
            | WitTy::Result(..)
            | WitTy::Variant(_) => {
                unreachable!("never a single flat value")
            }
        }
    }

    /// disc in local `d` → a payload-less variant box of `cases[d]`. Built as a
    /// chain `d==i ? box(cases[i]) : <recurse>`; falls through to the last case.
    fn lift_enum(&mut self, fx: &mut FnCtx, d: u32, cases: &[String], i: usize) {
        if i + 1 == cases.len() {
            let a = self.none_like_box(&cases[i]);
            fx.op(I::I32Const(a as i32));
            return;
        }
        fx.op(I::LocalGet(d));
        fx.op(I::I32Const(i as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(ValType::I32)));
        let a = self.none_like_box(&cases[i]);
        fx.op(I::I32Const(a as i32));
        fx.op(I::Else);
        self.lift_enum(fx, d, cases, i + 1);
        fx.op(I::End);
    }

    /// bitset in local `v` → flags value box (TAG_FLG: the names of the set
    /// bits, in declaration order — the interpreter's `Value::Flg`).
    fn lift_flags(&mut self, fx: &mut FnCtx, v: u32, names: &[String]) {
        let n = names.len();
        let p = fx.local(ValType::I32);
        let idx = fx.local(ValType::I32);
        // allocate for the worst case (all bits set); the bump allocator
        // wastes the unset slots, which die with the arena
        fx.op(I::I32Const(8 + 4 * n as i32));
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(p));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(TAG_FLG));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(idx));
        for (i, name) in names.iter().enumerate() {
            let kaddr = self.intern_str(name);
            // if (v >> i) & 1 { box[8 + 4*idx] = name; idx += 1 }
            fx.op(I::LocalGet(v));
            fx.op(I::I32Const(i as i32));
            fx.op(I::I32ShrU);
            fx.op(I::I32Const(1));
            fx.op(I::I32And);
            fx.op(I::If(BlockType::Empty));
            fx.op(I::LocalGet(p));
            fx.op(I::I32Const(8));
            fx.op(I::I32Add);
            fx.op(I::LocalGet(idx));
            fx.op(I::I32Const(2));
            fx.op(I::I32Shl);
            fx.op(I::I32Add);
            fx.op(I::I32Const(kaddr as i32));
            fx.op(I::I32Store(ma(0, 2)));
            fx.op(I::LocalGet(idx));
            fx.op(I::I32Const(1));
            fx.op(I::I32Add);
            fx.op(I::LocalSet(idx));
            fx.op(I::End);
        }
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(idx));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(p));
    }

    /// Lift a value passed flattened across the boundary: read `flat(ty)`
    /// consecutive flat locals starting at `base`, leave a boxed value on the
    /// stack. Generalizes the per-type lifting for scalars, strings, lists, and
    /// (recursively) records.
    fn lift_flat(&mut self, fx: &mut FnCtx, ty: &WitTy, base: u32) -> Result<(), String> {
        match ty {
            WitTy::Str => {
                fx.op(I::LocalGet(base));
                fx.op(I::LocalGet(base + 1));
                fx.op(I::Call(self.h.box_str));
            }
            WitTy::List(elem) => self.lift_list(fx, base, base + 1, elem)?,
            WitTy::Record(fields) => {
                let n = fields.len();
                let p = fx.local(ValType::I32);
                fx.op(I::I32Const(8 + 8 * n as i32));
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(p));
                fx.op(I::LocalGet(p));
                fx.op(I::I32Const(TAG_REC));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(p));
                fx.op(I::I32Const(n as i32));
                fx.op(I::I32Store(ma(4, 2)));
                let mut off = base;
                for (i, (k, ft)) in fields.iter().enumerate() {
                    let kaddr = self.intern_str(k);
                    fx.op(I::LocalGet(p));
                    fx.op(I::I32Const(kaddr as i32));
                    fx.op(I::I32Store(ma(8 + 8 * i as u64, 2)));
                    fx.op(I::LocalGet(p));
                    self.lift_flat(fx, ft, off)?;
                    fx.op(I::I32Store(ma(12 + 8 * i as u64, 2)));
                    off += flat(ft).len() as u32;
                }
                fx.op(I::LocalGet(p));
            }
            WitTy::Tuple(elems) => {
                // build a TAG_TUP box `[tag, n, elem ptrs…]` from the flat locals
                let n = elems.len();
                let p = fx.local(ValType::I32);
                fx.op(I::I32Const(8 + 4 * n as i32));
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(p));
                fx.op(I::LocalGet(p));
                fx.op(I::I32Const(TAG_TUP));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(p));
                fx.op(I::I32Const(n as i32));
                fx.op(I::I32Store(ma(4, 2)));
                let mut off = base;
                for (i, et) in elems.iter().enumerate() {
                    fx.op(I::LocalGet(p));
                    self.lift_flat(fx, et, off)?;
                    fx.op(I::I32Store(ma(8 + 4 * i as u64, 2)));
                    off += flat(et).len() as u32;
                }
                fx.op(I::LocalGet(p));
            }
            WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_) => {
                // disc at `base`, payload union starting at `base + 1`
                let cases: Vec<(String, Option<WitTy>)> = ty
                    .variant_cases()
                    .unwrap()
                    .into_iter()
                    .map(|(n, p)| (n.to_string(), p.cloned()))
                    .collect();
                let joined: Vec<ValType> = flat(ty)[1..].to_vec();
                self.lift_variant_flat_chain(fx, base, &cases, &joined, 0)?;
            }
            _ => {
                fx.op(I::LocalGet(base));
                self.lift(fx, ty);
            }
        }
        Ok(())
    }

    /// Lift an N-case variant passed flattened: dispatch on the disc at `base`
    /// (`disc==i ? lift(case i) : <recurse>`); payload union starts at `base+1`.
    fn lift_variant_flat_chain(
        &mut self,
        fx: &mut FnCtx,
        base: u32,
        cases: &[(String, Option<WitTy>)],
        joined: &[ValType],
        i: usize,
    ) -> Result<(), String> {
        if i + 1 == cases.len() {
            return self.lift_variant_case(fx, &cases[i].0, cases[i].1.as_ref(), base + 1, joined);
        }
        fx.op(I::LocalGet(base));
        fx.op(I::I32Const(i as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(ValType::I32)));
        self.lift_variant_case(fx, &cases[i].0, cases[i].1.as_ref(), base + 1, joined)?;
        fx.op(I::Else);
        self.lift_variant_flat_chain(fx, base, cases, joined, i + 1)?;
        fx.op(I::End);
        Ok(())
    }

    /// Build one arm of a lifted option/result: a payload-carrying case lifts
    /// its payload from the flat locals and wraps it; a payload-less case is the
    /// static box.
    fn lift_variant_case(
        &mut self,
        fx: &mut FnCtx,
        case: &str,
        pay: Option<&WitTy>,
        payload_base: u32,
        joined: &[ValType],
    ) -> Result<(), String> {
        match pay {
            Some(pt) => {
                let pflat = flat(pt);
                // The payload was widened into the joined union slots; narrow
                // each joined-typed local back to the payload's flat type into a
                // fresh contiguous block, then lift from that block. When no slot
                // needs narrowing this is a straight copy.
                let needs_narrowing = pflat.iter().zip(joined).any(|(have, want)| have != want);
                if needs_narrowing {
                    // Allocate the payload-typed block contiguously.
                    let block: Vec<u32> = pflat.iter().map(|&vt| fx.local(vt)).collect();
                    for (i, &have) in pflat.iter().enumerate() {
                        fx.op(I::LocalGet(payload_base + i as u32));
                        coerce_flat_from(fx, joined[i], have);
                        fx.op(I::LocalSet(block[i]));
                    }
                    self.lift_flat(fx, pt, block[0])?;
                } else {
                    self.lift_flat(fx, pt, payload_base)?;
                }
                self.wrap_variant(fx, case);
            }
            None => {
                let a = self.none_like_box(case);
                fx.op(I::I32Const(a as i32));
            }
        }
        Ok(())
    }

    /// Store the canonical in-memory representation of `src` (a boxed value in
    /// the given local) at `dst + off`. Records lay fields out at aligned
    /// offsets; scalar fields only (string/list inside a boundary record are
    /// not supported by the wasm backend yet).
    fn store_to_mem(
        &mut self,
        fx: &mut FnCtx,
        ty: &WitTy,
        src: u32,
        dst: u32,
        off: u64,
    ) -> Result<(), String> {
        match ty {
            WitTy::Bool => {
                fx.op(I::LocalGet(dst));
                fx.op(I::LocalGet(src));
                fx.op(I::Call(self.h.truthy));
                fx.op(I::I32Store8(ma(off, 0)));
            }
            WitTy::Char => {
                fx.op(I::LocalGet(dst));
                fx.op(I::LocalGet(src));
                fx.op(I::Call(self.h.unbox_char));
                fx.op(I::I32WrapI64);
                fx.op(I::I32Store(ma(off, 2)));
            }
            WitTy::Handle => {
                fx.op(I::LocalGet(dst));
                fx.op(I::LocalGet(src));
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::I32WrapI64);
                fx.op(I::I32Store(ma(off, 2)));
            }
            WitTy::IntS(w) | WitTy::IntU(w) => {
                let w = *w;
                fx.op(I::LocalGet(dst));
                fx.op(I::LocalGet(src));
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::I32WrapI64);
                match w {
                    1 => fx.op(I::I32Store8(ma(off, 0))),
                    2 => fx.op(I::I32Store16(ma(off, 1))),
                    _ => fx.op(I::I32Store(ma(off, 2))),
                }
            }
            WitTy::S64 => {
                fx.op(I::LocalGet(dst));
                fx.op(I::LocalGet(src));
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::I64Store(ma(off, 3)));
            }
            WitTy::F32 => {
                fx.op(I::LocalGet(dst));
                fx.op(I::LocalGet(src));
                fx.op(I::Call(self.h.unbox_dec));
                fx.op(I::F32DemoteF64);
                fx.op(I::F32Store(ma(off, 2)));
            }
            WitTy::F64 => {
                fx.op(I::LocalGet(dst));
                fx.op(I::LocalGet(src));
                fx.op(I::Call(self.h.unbox_dec));
                fx.op(I::F64Store(ma(off, 3)));
            }
            WitTy::Record(fields) => {
                for ((o, ft), (k, _)) in record_field_offsets(ty).into_iter().zip(fields) {
                    let kaddr = self.intern_str(k);
                    let fld = fx.local(ValType::I32);
                    fx.op(I::LocalGet(src));
                    fx.op(I::I32Const(kaddr as i32));
                    fx.op(I::Call(self.h.rec_get));
                    fx.op(I::LocalSet(fld));
                    self.store_to_mem(fx, &ft, fld, dst, off + o)?;
                }
            }
            WitTy::Tuple(_) => {
                // element boxes live at @8+4i in the TAG_TUP box
                for (i, (o, et)) in record_field_offsets(ty).into_iter().enumerate() {
                    let fld = fx.local(ValType::I32);
                    fx.op(I::LocalGet(src));
                    fx.op(I::I32Load(ma(8 + 4 * i as u64, 2)));
                    fx.op(I::LocalSet(fld));
                    self.store_to_mem(fx, &et, fld, dst, off + o)?;
                }
            }
            WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_) => {
                let cases: Vec<(String, Option<WitTy>)> = ty
                    .variant_cases()
                    .unwrap()
                    .into_iter()
                    .map(|(n, p)| (n.to_string(), p.cloned()))
                    .collect();
                if cases.len() > 0x100 {
                    return Err("variant with more than 256 cases is not supported \
                                by the wasm backend yet"
                        .into());
                }
                let poff = variant_payload_offset(ty);
                self.store_variant_chain(fx, src, &cases, dst, off, poff, 0)?;
            }
            WitTy::Enum(cases) => {
                if cases.len() > 0x100 {
                    return Err("enum with more than 256 cases is not supported \
                                by the wasm backend yet"
                        .into());
                }
                // store the box's case ordinal as a 1-byte discriminant
                fx.op(I::LocalGet(dst));
                let resty = self.ty_idx(vec![], vec![ValType::I32]);
                let b = fx.local(ValType::I32);
                fx.op(I::LocalGet(src));
                fx.op(I::LocalSet(b));
                self.lower_enum_chain(fx, b, cases, resty, 0)?;
                fx.op(I::I32Store8(ma(off, 0)));
            }
            WitTy::Flags(names) => {
                if names.len() > 32 {
                    return Err("flags with more than 32 members is not supported \
                                by the wasm backend yet"
                        .into());
                }
                // OR the set flags into a bitset word, then store it at the
                // canonical width (1/2/4 bytes for ≤8/≤16/≤32 members).
                fx.op(I::LocalGet(dst));
                fx.op(I::LocalGet(src));
                self.lower(fx, &WitTy::Flags(names.clone()))?;
                match names.len() {
                    0..=8 => fx.op(I::I32Store8(ma(off, 0))),
                    9..=16 => fx.op(I::I32Store16(ma(off, 1))),
                    _ => fx.op(I::I32Store(ma(off, 2))),
                }
            }
            WitTy::Str => {
                // canonical string in memory is (ptr, len); the component adapter
                // copies the bytes via our cabi_realloc when lifting
                fx.op(I::LocalGet(dst));
                fx.op(I::LocalGet(src));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add); // bytes begin after the [tag, len] header
                fx.op(I::I32Store(ma(off, 2)));
                fx.op(I::LocalGet(dst));
                fx.op(I::LocalGet(src));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::I32Store(ma(off + 4, 2)));
            }
            WitTy::List(elem) => {
                // lower to a canonical (ptr, len) buffer, then store both words
                fx.op(I::LocalGet(src));
                self.lower_list(fx, elem)?;
                let len = fx.local(ValType::I32);
                let ptr = fx.local(ValType::I32);
                fx.op(I::LocalSet(len));
                fx.op(I::LocalSet(ptr));
                fx.op(I::LocalGet(dst));
                fx.op(I::LocalGet(ptr));
                fx.op(I::I32Store(ma(off, 2)));
                fx.op(I::LocalGet(dst));
                fx.op(I::LocalGet(len));
                fx.op(I::I32Store(ma(off + 4, 2)));
            }
        }
        Ok(())
    }

    /// Store one arm of an option/result to memory: the 1-byte discriminant at
    /// `off`, then (if present) the payload at `off + payload_offset`.
    fn store_variant_case(
        &mut self,
        fx: &mut FnCtx,
        src: u32,
        disc: i32,
        pay: Option<&WitTy>,
        dst: u32,
        off: u64,
        poff: u64,
    ) -> Result<(), String> {
        fx.op(I::LocalGet(dst));
        fx.op(I::I32Const(disc));
        fx.op(I::I32Store8(ma(off, 0)));
        if let Some(pt) = pay {
            let fld = fx.local(ValType::I32);
            fx.op(I::LocalGet(src));
            fx.op(I::I32Load(ma(8, 2))); // variant payload box
            fx.op(I::LocalSet(fld));
            self.store_to_mem(fx, pt, fld, dst, off + poff)?;
        }
        Ok(())
    }

    /// Store an N-case variant box to memory: match the box's case-name against
    /// each case (`name==cases[i] ? store(i) : <recurse>`), the last is the else.
    #[allow(clippy::too_many_arguments)]
    fn store_variant_chain(
        &mut self,
        fx: &mut FnCtx,
        src: u32,
        cases: &[(String, Option<WitTy>)],
        dst: u32,
        off: u64,
        poff: u64,
        i: usize,
    ) -> Result<(), String> {
        if i + 1 == cases.len() {
            return self.store_variant_case(fx, src, i as i32, cases[i].1.as_ref(), dst, off, poff);
        }
        let naddr = self.intern_str(&cases[i].0);
        fx.op(I::LocalGet(src));
        fx.op(I::I32Load(ma(4, 2))); // TAG_VAR case-name box
        fx.op(I::I32Const(naddr as i32));
        fx.op(I::Call(self.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        self.store_variant_case(fx, src, i as i32, cases[i].1.as_ref(), dst, off, poff)?;
        fx.op(I::Else);
        self.store_variant_chain(fx, src, cases, dst, off, poff, i + 1)?;
        fx.op(I::End);
        Ok(())
    }

    /// Inverse of [`store_to_mem`]: read the canonical representation of `ty`
    /// at `src + off` and leave a boxed value on the stack.
    fn load_from_mem(
        &mut self,
        fx: &mut FnCtx,
        ty: &WitTy,
        src: u32,
        off: u64,
    ) -> Result<(), String> {
        match ty {
            WitTy::Bool => {
                fx.op(I::LocalGet(src));
                fx.op(I::I32Load8U(ma(off, 0)));
                fx.op(I::Call(self.h.box_bool));
            }
            WitTy::IntS(w) => {
                fx.op(I::LocalGet(src));
                match *w {
                    1 => fx.op(I::I32Load8S(ma(off, 0))),
                    2 => fx.op(I::I32Load16S(ma(off, 1))),
                    _ => fx.op(I::I32Load(ma(off, 2))),
                }
                fx.op(I::I64ExtendI32S);
                fx.op(I::Call(self.h.box_int));
            }
            WitTy::IntU(w) => {
                fx.op(I::LocalGet(src));
                match *w {
                    1 => fx.op(I::I32Load8U(ma(off, 0))),
                    2 => fx.op(I::I32Load16U(ma(off, 1))),
                    _ => fx.op(I::I32Load(ma(off, 2))),
                }
                fx.op(I::I64ExtendI32U);
                fx.op(I::Call(self.h.box_int));
            }
            WitTy::Char => {
                fx.op(I::LocalGet(src));
                fx.op(I::I32Load(ma(off, 2)));
                fx.op(I::I64ExtendI32U);
                self.box_char(fx);
            }
            WitTy::Handle => {
                fx.op(I::LocalGet(src));
                fx.op(I::I32Load(ma(off, 2)));
                fx.op(I::I64ExtendI32U);
                fx.op(I::Call(self.h.box_int));
            }
            WitTy::S64 => {
                fx.op(I::LocalGet(src));
                fx.op(I::I64Load(ma(off, 3)));
                fx.op(I::Call(self.h.box_int));
            }
            WitTy::F32 => {
                fx.op(I::LocalGet(src));
                fx.op(I::F32Load(ma(off, 2)));
                fx.op(I::F64PromoteF32);
                fx.op(I::Call(self.h.box_dec));
            }
            WitTy::F64 => {
                fx.op(I::LocalGet(src));
                fx.op(I::F64Load(ma(off, 3)));
                fx.op(I::Call(self.h.box_dec));
            }
            WitTy::Record(fields) => {
                let n = fields.len();
                let p = fx.local(ValType::I32);
                fx.op(I::I32Const(8 + 8 * n as i32));
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(p));
                fx.op(I::LocalGet(p));
                fx.op(I::I32Const(TAG_REC));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(p));
                fx.op(I::I32Const(n as i32));
                fx.op(I::I32Store(ma(4, 2)));
                for (i, ((o, ft), (k, _))) in
                    record_field_offsets(ty).into_iter().zip(fields).enumerate()
                {
                    let kaddr = self.intern_str(k);
                    fx.op(I::LocalGet(p));
                    fx.op(I::I32Const(kaddr as i32));
                    fx.op(I::I32Store(ma(8 + 8 * i as u64, 2)));
                    fx.op(I::LocalGet(p));
                    self.load_from_mem(fx, &ft, src, off + o)?;
                    fx.op(I::I32Store(ma(12 + 8 * i as u64, 2)));
                }
                fx.op(I::LocalGet(p));
            }
            WitTy::Tuple(elems) => {
                let n = elems.len();
                let p = fx.local(ValType::I32);
                fx.op(I::I32Const(8 + 4 * n as i32));
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(p));
                fx.op(I::LocalGet(p));
                fx.op(I::I32Const(TAG_TUP));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(p));
                fx.op(I::I32Const(n as i32));
                fx.op(I::I32Store(ma(4, 2)));
                for (i, (o, et)) in record_field_offsets(ty).into_iter().enumerate() {
                    fx.op(I::LocalGet(p));
                    self.load_from_mem(fx, &et, src, off + o)?;
                    fx.op(I::I32Store(ma(8 + 4 * i as u64, 2)));
                }
                fx.op(I::LocalGet(p));
            }
            WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_) => {
                let cases: Vec<(String, Option<WitTy>)> = ty
                    .variant_cases()
                    .unwrap()
                    .into_iter()
                    .map(|(n, p)| (n.to_string(), p.cloned()))
                    .collect();
                let poff = variant_payload_offset(ty);
                // read the 1-byte disc into a local, then dispatch on it
                let d = fx.local(ValType::I32);
                fx.op(I::LocalGet(src));
                fx.op(I::I32Load8U(ma(off, 0)));
                fx.op(I::LocalSet(d));
                self.load_variant_chain(fx, d, &cases, src, off + poff, 0)?;
            }
            WitTy::Enum(cases) => {
                // 1-byte disc → payload-less variant box of the i-th case
                let d = fx.local(ValType::I32);
                fx.op(I::LocalGet(src));
                fx.op(I::I32Load8U(ma(off, 0)));
                fx.op(I::LocalSet(d));
                self.lift_enum(fx, d, cases, 0);
            }
            WitTy::Flags(names) => {
                // bitset word → record box of name → bool
                let v = fx.local(ValType::I32);
                fx.op(I::LocalGet(src));
                if names.len() <= 8 {
                    fx.op(I::I32Load8U(ma(off, 0)));
                } else if names.len() <= 16 {
                    fx.op(I::I32Load16U(ma(off, 1)));
                } else {
                    fx.op(I::I32Load(ma(off, 2)));
                }
                fx.op(I::LocalSet(v));
                self.lift_flags(fx, v, names);
            }
            WitTy::Str => {
                fx.op(I::LocalGet(src));
                fx.op(I::I32Load(ma(off, 2))); // ptr (into our memory)
                fx.op(I::LocalGet(src));
                fx.op(I::I32Load(ma(off + 4, 2))); // len
                fx.op(I::Call(self.h.box_str));
            }
            WitTy::List(elem) => {
                let ptr = fx.local(ValType::I32);
                let len = fx.local(ValType::I32);
                fx.op(I::LocalGet(src));
                fx.op(I::I32Load(ma(off, 2)));
                fx.op(I::LocalSet(ptr));
                fx.op(I::LocalGet(src));
                fx.op(I::I32Load(ma(off + 4, 2)));
                fx.op(I::LocalSet(len));
                self.lift_list(fx, ptr, len, elem)?;
            }
        }
        Ok(())
    }

    /// Build one arm of an option/result loaded from memory: read the payload at
    /// `payload_addr` and wrap it, or yield the static payload-less box.
    fn load_variant_case(
        &mut self,
        fx: &mut FnCtx,
        case: &str,
        pay: Option<&WitTy>,
        src: u32,
        payload_off: u64,
    ) -> Result<(), String> {
        match pay {
            Some(pt) => {
                self.load_from_mem(fx, pt, src, payload_off)?;
                self.wrap_variant(fx, case);
            }
            None => {
                let a = self.none_like_box(case);
                fx.op(I::I32Const(a as i32));
            }
        }
        Ok(())
    }

    /// Load an N-case variant from memory: dispatch on the disc in local `d`
    /// (`d==i ? load(case i) : <recurse>`); the last case is the else.
    fn load_variant_chain(
        &mut self,
        fx: &mut FnCtx,
        d: u32,
        cases: &[(String, Option<WitTy>)],
        src: u32,
        payload_off: u64,
        i: usize,
    ) -> Result<(), String> {
        if i + 1 == cases.len() {
            return self.load_variant_case(fx, &cases[i].0, cases[i].1.as_ref(), src, payload_off);
        }
        fx.op(I::LocalGet(d));
        fx.op(I::I32Const(i as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(ValType::I32)));
        self.load_variant_case(fx, &cases[i].0, cases[i].1.as_ref(), src, payload_off)?;
        fx.op(I::Else);
        self.load_variant_chain(fx, d, cases, src, payload_off, i + 1)?;
        fx.op(I::End);
        Ok(())
    }

    /// Whether a canonical-layout type is eligible for the type-indexed
    /// structural `eq` fast path (5.6): its in-memory representation can be
    /// compared field-by-field with the same boolean the interpreter's
    /// value-level `eq` (`Value: PartialEq`) would produce.
    ///
    /// Excluded (kept on the boxed `eq_raw` path):
    /// - **Flags**: the canonical bitset is order-free, but the oracle's
    ///   `Value::Flg` is a `Vec` whose `eq` is order-sensitive; two bitsets
    ///   could compare equal where the boxed values do not.
    /// - **Handle**: resource equality is Rc identity in the oracle, not the
    ///   opaque i32 the canonical form carries.
    fn mem_eq_eligible(&self, ty: &WitTy) -> bool {
        match ty {
            WitTy::Bool
            | WitTy::Char
            | WitTy::IntS(_)
            | WitTy::IntU(_)
            | WitTy::S64
            | WitTy::F32
            | WitTy::F64
            | WitTy::Str
            | WitTy::Enum(_) => true,
            WitTy::List(elem) => self.mem_eq_eligible(elem),
            WitTy::Record(fields) => fields.iter().all(|(_, t)| self.mem_eq_eligible(t)),
            WitTy::Tuple(elems) => elems.iter().all(|t| self.mem_eq_eligible(t)),
            WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_) => ty
                .variant_cases()
                .unwrap()
                .iter()
                .all(|(_, p)| p.is_none_or(|pt| self.mem_eq_eligible(pt))),
            WitTy::Flags(_) | WitTy::Handle => false,
        }
    }

    /// Emit a structural equality test of the two canonical values of type
    /// `ty` at `a + off` and `b + off` (both `a`/`b` are i32 base-pointer
    /// locals). Leaves an i32 `1`/`0` on the stack. Recurses over the layout
    /// so the result matches the interpreter's value-level `eq` exactly:
    /// scalars compare their loaded value, records/tuples compare every field,
    /// variants compare the discriminant and (when it matches) the active
    /// case's payload, strings and lists compare length then contents.
    fn emit_mem_eq(
        &mut self,
        fx: &mut FnCtx,
        ty: &WitTy,
        a: u32,
        b: u32,
        off: u64,
    ) -> Result<(), String> {
        match ty {
            WitTy::Bool => {
                fx.op(I::LocalGet(a));
                fx.op(I::I32Load8U(ma(off, 0)));
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load8U(ma(off, 0)));
                fx.op(I::I32Eq);
            }
            WitTy::IntS(w) | WitTy::IntU(w) => {
                // width-appropriate loads; equality is sign-agnostic, so an
                // unsigned load on each side compares the stored bytes.
                let w = *w;
                fx.op(I::LocalGet(a));
                match w {
                    1 => fx.op(I::I32Load8U(ma(off, 0))),
                    2 => fx.op(I::I32Load16U(ma(off, 1))),
                    _ => fx.op(I::I32Load(ma(off, 2))),
                }
                fx.op(I::LocalGet(b));
                match w {
                    1 => fx.op(I::I32Load8U(ma(off, 0))),
                    2 => fx.op(I::I32Load16U(ma(off, 1))),
                    _ => fx.op(I::I32Load(ma(off, 2))),
                }
                fx.op(I::I32Eq);
            }
            WitTy::Char => {
                fx.op(I::LocalGet(a));
                fx.op(I::I32Load(ma(off, 2)));
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(off, 2)));
                fx.op(I::I32Eq);
            }
            WitTy::S64 => {
                fx.op(I::LocalGet(a));
                fx.op(I::I64Load(ma(off, 3)));
                fx.op(I::LocalGet(b));
                fx.op(I::I64Load(ma(off, 3)));
                fx.op(I::I64Eq);
            }
            WitTy::F32 => {
                fx.op(I::LocalGet(a));
                fx.op(I::F32Load(ma(off, 2)));
                fx.op(I::LocalGet(b));
                fx.op(I::F32Load(ma(off, 2)));
                fx.op(I::F32Eq);
            }
            WitTy::F64 => {
                fx.op(I::LocalGet(a));
                fx.op(I::F64Load(ma(off, 3)));
                fx.op(I::LocalGet(b));
                fx.op(I::F64Load(ma(off, 3)));
                fx.op(I::F64Eq);
            }
            WitTy::Enum(_) => {
                // payload-less: the 1-byte discriminant carries the whole value.
                fx.op(I::LocalGet(a));
                fx.op(I::I32Load8U(ma(off, 0)));
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load8U(ma(off, 0)));
                fx.op(I::I32Eq);
            }
            WitTy::Record(_) | WitTy::Tuple(_) => {
                let fields = record_field_offsets(ty);
                if fields.is_empty() {
                    fx.op(I::I32Const(1));
                } else {
                    for (i, (o, ft)) in fields.iter().enumerate() {
                        self.emit_mem_eq(fx, ft, a, b, off + o)?;
                        if i > 0 {
                            fx.op(I::I32And);
                        }
                    }
                }
            }
            WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_) => {
                let cases: Vec<(String, Option<WitTy>)> = ty
                    .variant_cases()
                    .unwrap()
                    .into_iter()
                    .map(|(n, p)| (n.to_string(), p.cloned()))
                    .collect();
                let poff = off + variant_payload_offset(ty);
                let da = fx.local(ValType::I32);
                fx.op(I::LocalGet(a));
                fx.op(I::I32Load8U(ma(off, 0)));
                fx.op(I::LocalSet(da));
                fx.op(I::LocalGet(da));
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load8U(ma(off, 0)));
                fx.op(I::I32Eq);
                fx.op(I::If(BlockType::Result(ValType::I32)));
                // discriminants agree: compare the active case's payload.
                self.emit_variant_payload_eq(fx, &cases, da, a, b, poff, 0)?;
                fx.op(I::Else);
                fx.op(I::I32Const(0));
                fx.op(I::End);
            }
            WitTy::Str => {
                // (ptr, len) inline at `off`; equal iff same byte length and
                // same bytes. `len` here is the byte length the canonical word
                // stores — byte equality is string equality.
                let pa = fx.local(ValType::I32);
                let pb = fx.local(ValType::I32);
                let la = fx.local(ValType::I32);
                fx.op(I::LocalGet(a));
                fx.op(I::I32Load(ma(off, 2)));
                fx.op(I::LocalSet(pa));
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(off, 2)));
                fx.op(I::LocalSet(pb));
                fx.op(I::LocalGet(a));
                fx.op(I::I32Load(ma(off + 4, 2)));
                fx.op(I::LocalTee(la));
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(off + 4, 2)));
                fx.op(I::I32Eq);
                fx.op(I::If(BlockType::Result(ValType::I32)));
                self.emit_bytes_eq(fx, pa, pb, la);
                fx.op(I::Else);
                fx.op(I::I32Const(0));
                fx.op(I::End);
            }
            WitTy::List(elem) => {
                // (ptr, len) inline at `off`; `len` is the element count.
                // Equal iff same count and every element (at its stride)
                // compares equal.
                let stride = elem_size(elem);
                let elem = (**elem).clone();
                let pa = fx.local(ValType::I32);
                let pb = fx.local(ValType::I32);
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(a));
                fx.op(I::I32Load(ma(off, 2)));
                fx.op(I::LocalSet(pa));
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(off, 2)));
                fx.op(I::LocalSet(pb));
                fx.op(I::LocalGet(a));
                fx.op(I::I32Load(ma(off + 4, 2)));
                fx.op(I::LocalTee(n));
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(off + 4, 2)));
                fx.op(I::I32Eq);
                fx.op(I::If(BlockType::Result(ValType::I32)));
                self.emit_list_eq(fx, &elem, pa, pb, n, stride)?;
                fx.op(I::Else);
                fx.op(I::I32Const(0));
                fx.op(I::End);
            }
            WitTy::Flags(_) | WitTy::Handle => {
                return Err("internal: mem eq over an ineligible type".into());
            }
        }
        Ok(())
    }

    /// Chain that compares the active variant case's payload once the
    /// discriminants are known equal (in local `d`). Mirrors
    /// `load_variant_chain`: `d==i ? <payload eq of case i> : <recurse>`, with
    /// the last case as the else. Payload-less cases contribute `1`.
    fn emit_variant_payload_eq(
        &mut self,
        fx: &mut FnCtx,
        cases: &[(String, Option<WitTy>)],
        d: u32,
        a: u32,
        b: u32,
        payload_off: u64,
        i: usize,
    ) -> Result<(), String> {
        if i + 1 == cases.len() {
            return match cases[i].1.as_ref() {
                Some(pt) => self.emit_mem_eq(fx, &pt.clone(), a, b, payload_off),
                None => {
                    fx.op(I::I32Const(1));
                    Ok(())
                }
            };
        }
        fx.op(I::LocalGet(d));
        fx.op(I::I32Const(i as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(ValType::I32)));
        match cases[i].1.as_ref() {
            Some(pt) => self.emit_mem_eq(fx, &pt.clone(), a, b, payload_off)?,
            None => fx.op(I::I32Const(1)),
        }
        fx.op(I::Else);
        self.emit_variant_payload_eq(fx, cases, d, a, b, payload_off, i + 1)?;
        fx.op(I::End);
        Ok(())
    }

    /// Byte-equality loop: 1 iff the `len` bytes at `pa` equal those at `pb`.
    /// Leaves an i32 `1`/`0`. (Callers have already checked equal lengths.)
    fn emit_bytes_eq(&mut self, fx: &mut FnCtx, pa: u32, pb: u32, len: u32) {
        let i = fx.local(ValType::I32);
        let res = fx.local(ValType::I32);
        fx.op(I::I32Const(1));
        fx.op(I::LocalSet(res));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        // done when i >= len
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(len));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // bytes differ -> res = 0, break
        fx.op(I::LocalGet(pa));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(0, 0)));
        fx.op(I::LocalGet(pb));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(0, 0)));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(res));
        fx.op(I::Br(2));
        fx.op(I::End);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(res));
    }

    /// List-equality loop: 1 iff every one of the `n` elements at `pa` equals
    /// the element at `pb` (both stride `stride`, element type `elem`). Leaves
    /// an i32 `1`/`0`. (Callers have already checked equal element counts.)
    fn emit_list_eq(
        &mut self,
        fx: &mut FnCtx,
        elem: &WitTy,
        pa: u32,
        pb: u32,
        n: u32,
        stride: u64,
    ) -> Result<(), String> {
        let i = fx.local(ValType::I32);
        let res = fx.local(ValType::I32);
        let ea = fx.local(ValType::I32);
        let eb = fx.local(ValType::I32);
        fx.op(I::I32Const(1));
        fx.op(I::LocalSet(res));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(n));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // element base pointers pa + i*stride, pb + i*stride
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(stride as i32));
        fx.op(I::I32Mul);
        fx.op(I::LocalGet(pa));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(ea));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(stride as i32));
        fx.op(I::I32Mul);
        fx.op(I::LocalGet(pb));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(eb));
        self.emit_mem_eq(fx, elem, ea, eb, 0)?;
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(res));
        fx.op(I::Br(2));
        fx.op(I::End);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(res));
        Ok(())
    }

    /// `form-kind`: a string box naming the form's kind by box tag (mirrors
    /// `builtins.rs:391`). A payloaded `TAG_VAR` is a quoted call ("call"), a
    /// payload-less one a symbol ("sym"). A non-form (e.g. a closure) traps,
    /// matching the interpreter's `form-kind expects a form` error.
    fn form_kind(&mut self, fx: &mut FnCtx, arg: NodeId) -> Result<(), String> {
        let v = fx.local(ValType::I32);
        self.expr(fx, arg, false)?;
        fx.op(I::LocalSet(v));
        let t = fx.local(ValType::I32);
        fx.op(I::LocalGet(v));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(t));
        let r = fx.local(ValType::I32);
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(r));
        for (tag, kind) in [
            (TAG_BOOL, "bool"),
            (TAG_INT, "int"),
            (TAG_STR, "str"),
            (TAG_LIST, "lst"),
            (TAG_DEC, "dec"),
            (TAG_REC, "rec"),
            (TAG_TUP, "tup"),
            (TAG_FLG, "flg"),
            (TAG_CHAR, "char"),
        ] {
            let s = self.intern_str(kind) as i32;
            fx.op(I::LocalGet(t));
            fx.op(I::I32Const(tag));
            fx.op(I::I32Eq);
            fx.op(I::If(BlockType::Empty));
            fx.op(I::I32Const(s));
            fx.op(I::LocalSet(r));
            fx.op(I::End);
        }
        let sym = self.intern_str("sym") as i32;
        let call = self.intern_str("call") as i32;
        fx.op(I::LocalGet(t));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(v));
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(call));
        fx.op(I::LocalSet(r));
        fx.op(I::Else);
        fx.op(I::I32Const(sym));
        fx.op(I::LocalSet(r));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(r));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(r));
        Ok(())
    }

    /// Trap unless `rp` holds a non-empty record box, matching the
    /// interpreter's `rec-key`/`rec-val` "expects a non-empty record" error.
    fn rec_guard(&mut self, fx: &mut FnCtx, rp: u32) {
        fx.op(I::LocalGet(rp));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_REC));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(rp));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
    }

    /// `gensym`: a fresh payload-less variant `g{n}-gen`, `n` from the
    /// per-instance i64 counter global (mirrors `builtins.rs:360`).
    /// Deterministic and collision-free across every expansion in one
    /// component instance.
    fn gensym(&mut self, fx: &mut FnCtx) -> Result<(), String> {
        let g = 1 + self.info.value_defs.len() as u32;
        let n = fx.local(ValType::I64);
        fx.op(I::GlobalGet(g));
        fx.op(I::LocalTee(n));
        fx.op(I::I64Const(1));
        fx.op(I::I64Add);
        fx.op(I::GlobalSet(g));
        let gpfx = self.intern_str("g") as i32;
        let gsfx = self.intern_str("-gen") as i32;
        fx.op(I::I32Const(gpfx));
        fx.op(I::LocalGet(n));
        fx.op(I::Call(self.h.box_int));
        fx.op(I::Call(self.h.to_str));
        fx.op(I::Call(self.h.strcat2));
        fx.op(I::I32Const(gsfx));
        fx.op(I::Call(self.h.strcat2));
        let casebox = fx.local(ValType::I32);
        fx.op(I::LocalSet(casebox));
        let p = fx.local(ValType::I32);
        fx.op(I::I32Const(12));
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(p));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(casebox));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(0));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(p));
        Ok(())
    }

    /// The scalar kind of `id`'s checker-inferred static type, if any
    /// (goal 5). Absent/unknown/compound types return `None` — the boxed
    /// fallback.
    fn node_scalar(&self, id: NodeId) -> Option<Scalar> {
        Scalar::of(self.node_types.get(&id)?)
    }

    /// Unbox a box pointer on the stack into an unboxed scalar (the
    /// boxed→typed seam). Traps on a tag the static type ruled out — exactly
    /// where the boxed path traps inside its polymorphic runtime helper.
    fn unbox_scalar(&mut self, fx: &mut FnCtx, kind: Scalar) {
        match kind {
            Scalar::Int => fx.op(I::Call(self.h.unbox_int)),
            Scalar::Float => fx.op(I::Call(self.h.as_f64)),
            Scalar::Bool => fx.op(I::Call(self.h.truthy)),
            Scalar::Char => fx.op(I::Call(self.h.unbox_char)),
        }
    }

    /// Box an unboxed scalar on the stack (the typed→boxed seam).
    fn box_scalar(&mut self, fx: &mut FnCtx, kind: Scalar) {
        match kind {
            Scalar::Int => fx.op(I::Call(self.h.box_int)),
            Scalar::Float => fx.op(I::Call(self.h.box_dec)),
            Scalar::Bool => fx.op(I::Call(self.h.box_bool)),
            Scalar::Char => self.box_char(fx),
        }
    }

    // -------------------------------------------- canonical memory (5.3)

    /// Intern a canonical-layout type; equal types share a [`MemTy`] index.
    fn mem_ty(&mut self, ty: &WitTy) -> MemTy {
        if let Some(i) = self.mem_tys.iter().position(|t| t == ty) {
            return i as MemTy;
        }
        self.mem_tys.push(ty.clone());
        (self.mem_tys.len() - 1) as MemTy
    }

    /// Map a checker type to the WIT type that drives canonical layout.
    /// Unresolved int/float literals live in the interpreter's full value
    /// domains (`Value::Int` = i64, `Value::Dec` = f64), so they lay out as
    /// s64/f64 — the layout must round-trip the oracle's value exactly.
    /// `None` = no (known) canonical layout; the boxed repr stays.
    fn wit_of_check_type(&self, t: &crate::check::Type) -> Option<WitTy> {
        use crate::check::Type as T;
        Some(match t {
            T::Bool => WitTy::Bool,
            T::U8 => WitTy::IntU(1),
            T::U16 => WitTy::IntU(2),
            T::U32 => WitTy::IntU(4),
            T::S8 => WitTy::IntS(1),
            T::S16 => WitTy::IntS(2),
            T::S32 => WitTy::IntS(4),
            // u64 rides the interpreter's i64 domain (the 5.2 residue)
            T::U64 | T::S64 | T::IntLit(_) => WitTy::S64,
            T::F32 => WitTy::F32,
            T::F64 | T::FloatLit => WitTy::F64,
            T::Char => WitTy::Char,
            T::String => WitTy::Str,
            T::Flags(names) => WitTy::Flags(names.clone()),
            T::List(e) => WitTy::List(Box::new(self.wit_of_check_type(e)?)),
            T::Option(e) => WitTy::Option(Box::new(self.wit_of_check_type(e)?)),
            T::Result(o, e) => WitTy::Result(
                Box::new(self.wit_of_check_type(o)?),
                Box::new(self.wit_of_check_type(e)?),
            ),
            T::Tuple(es) => WitTy::Tuple(
                es.iter()
                    .map(|e| self.wit_of_check_type(e))
                    .collect::<Option<Vec<_>>>()?,
            ),
            T::Record(fs) if !fs.is_empty() => WitTy::Record(
                fs.iter()
                    .map(|(k, ft)| Some((k.clone(), self.wit_of_check_type(ft)?)))
                    .collect::<Option<Vec<_>>>()?,
            ),
            // a nominal name resolves through the boundary type env (declared
            // records/variants/aliases); unknown names stay boxed
            T::Named(n) => return wit_ty(n, &self.type_env).ok(),
            _ => return None,
        })
    }

    /// When `id` is a variant-case constructor call/reference for the
    /// variant-shaped layout `ty` — one the current name resolution actually
    /// routes to a constructor — its case index, payload type, and argument
    /// forms. Mirrors `expr`'s routing exactly: a local binding shadows
    /// everything; `some`/`ok`/`err` are builtins (which outrank defs);
    /// any other head must resolve through `local_cases` (module defs and
    /// value globals shadow those); a bare Sym is a constructor only for
    /// `none` and nullary local cases (`value_def_ref`'s rule). `fx` is
    /// `None` in a prediction walk, where local shadowing is unknowable —
    /// the emission side falls back to the boxed store when resolution
    /// turns out different, so an optimistic answer here stays sound.
    fn ctor_parts(
        &self,
        fx: Option<&FnCtx>,
        id: NodeId,
        ty: &WitTy,
    ) -> Option<(usize, Option<WitTy>, Vec<NodeId>)> {
        let cases = ty.variant_cases()?;
        let (head, args, bare): (String, Vec<NodeId>, bool) = match self.arena.node(id).clone() {
            Node::Sym(n) => (n, vec![], true),
            Node::Tup(items) if !items.is_empty() => match self.arena.node(items[0]).clone() {
                Node::Sym(n) => (n, items[1..].to_vec(), false),
                _ => return None,
            },
            _ => return None,
        };
        if let Some(fx) = fx
            && fx.lookup(&head).is_some()
        {
            return None;
        }
        if bare {
            if head != "none" && self.local_cases.get(&head) != Some(&false) {
                return None;
            }
        } else if !matches!(head.as_str(), "some" | "ok" | "err") {
            if self.funcs.contains_key(&head) || self.value_globals.contains_key(&head) {
                return None;
            }
            // a payload-less local case is not callable (nor is any other name)
            if self.local_cases.get(&head) != Some(&true) {
                return None;
            }
        }
        let i = cases.iter().position(|(n, _)| *n == head)?;
        let payload = cases[i].1.cloned();
        Some((i, payload, args))
    }

    /// 5.4 construction gating: is `id` a case-constructor form whose value
    /// can be BUILT natively in the canonical layout of `ty` — the numeric
    /// discriminant plus a losslessly-stored payload? A case's value is its
    /// name plus payload (no field-order hazard), so the gate is just the
    /// constructor resolution plus per-payload lossless storability, with
    /// the interpreter's bundling rule: one argument is the payload, two or
    /// more bundle into a tuple payload.
    fn ctor_admissible(&self, look: &MemLookup, id: NodeId, ty: &WitTy) -> bool {
        let fx = match look {
            MemLookup::Fx(fx) => Some(*fx),
            MemLookup::Sim(_) => None,
        };
        let Some((_, payload, args)) = self.ctor_parts(fx, id, ty) else {
            return false;
        };
        // 1-byte discriminants only (the large-types child rides 5.4)
        if ty.variant_cases().is_some_and(|c| c.len() > 0x100) {
            return false;
        }
        match (args.len(), payload) {
            (0, None) => true,
            (1, Some(pt)) => self.mem_field_ok(look, args[0], &pt),
            (n, Some(WitTy::Tuple(es))) if n >= 2 && es.len() == n => args
                .iter()
                .zip(&es)
                .all(|(&a, et)| self.mem_field_ok(look, a, et)),
            _ => false,
        }
    }

    /// 5.3 gating: may expression `id` be emitted NATIVELY in the canonical
    /// layout of `ty`, yielding exactly the value the interpreter would
    /// build? Field order is observable (`eq`/`to-string` compare records
    /// positionally), so only a record literal whose field order matches the
    /// layout's — with losslessly-storable fields — or a binding already
    /// carrying this layout qualifies. Everything else stays boxed.
    fn can_mem_as(&self, look: &MemLookup, id: NodeId, ty: &WitTy) -> bool {
        match (self.arena.node(id), ty) {
            (Node::Rec(fields), WitTy::Record(tfs)) if !fields.is_empty() => {
                fields.len() == tfs.len()
                    && fields
                        .iter()
                        .zip(tfs)
                        .all(|((k, v), (tk, tf))| k == tk && self.mem_field_ok(look, *v, tf))
            }
            (Node::Lst(items), WitTy::List(elem)) => items
                .iter()
                .all(|&v| self.mem_field_ok(look, v, elem)),
            (Node::Sym(name), _) => {
                self.lookup_mem(look, name).is_some_and(|t| t == *ty)
                    // `none` / a nullary local case as a value (5.4)
                    || (self.lookup_mem(look, name).is_none()
                        && self.ctor_admissible(look, id, ty))
            }
            // A dep call whose declared result IS this layout: the retptr
            // area arrives in canonical form, in declared field order —
            // exactly the order the interpreter's boundary lift produces.
            (
                Node::Tup(items),
                WitTy::Record(_)
                | WitTy::Tuple(_)
                | WitTy::Str
                | WitTy::List(_)
                | WitTy::Option(_)
                | WitTy::Result(..)
                | WitTy::Variant(_),
            ) if !items.is_empty() => {
                match self.arena.node(items[0]) {
                    Node::Qsym(alias, fname) => self
                        .dep_result_mem_ty(alias, fname)
                        .is_some_and(|t| t == *ty),
                    // `tail` of a canonical list is itself canonical: the
                    // result is a fresh (ptr, len) pair sharing the operand's
                    // element buffer from element[1] (zero-copy — 5.6). tail
                    // preserves the list type, so the operand's Mem layout must
                    // equal this result's.
                    Node::Sym(name)
                        if name.as_str() == "tail"
                            && matches!(ty, WitTy::List(_))
                            && items.len() == 2 =>
                    {
                        self.node_mem_ty(look, items[1]).as_ref() == Some(ty)
                    }
                    // a variant-case constructor call builds in place (5.4)
                    Node::Sym(_) => self.ctor_admissible(look, id, ty),
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// The canonical layout a dep call's result area carries, when it is a
    /// retptr-lowered non-empty record or tuple, a string or list (whose
    /// canonical form is the (ptr, len) pair the area carries — 5.5), or an
    /// option/result/variant (numeric discriminant + payload at the
    /// canonical offset — 5.4). `None` for functor ops (they are not
    /// imports), scalar/One-flat results, and anything unresolvable.
    fn dep_result_mem_ty(&self, alias: &str, fname: &str) -> Option<WitTy> {
        let imp = self.info.imports.iter().find(|i| i.alias == alias)?;
        let dep = self.deps.get(&imp.package)?;
        let iface = import_iface(&imp.path);
        let sig = resolve_dep_func(dep, &iface, fname).ok()?.clone();
        if !matches!(flat_result(&sig, &self.type_env), Ok(FlatRes::Retptr)) {
            return None;
        }
        let rty = wit_ty(sig.result.as_deref()?, &self.type_env).ok()?;
        (matches!(&rty, WitTy::Record(fs) if !fs.is_empty())
            || matches!(&rty, WitTy::Tuple(es) if !es.is_empty())
            || matches!(
                &rty,
                WitTy::Str | WitTy::List(_) | WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_)
            ))
        .then_some(rty)
    }

    /// The canonical-layout type a name is bound at, if any — through the
    /// live emission scopes, or through the simulated scopes a def-signature
    /// prediction walks ([`Self::predict_body_mem`]) before any body exists.
    fn lookup_mem(&self, look: &MemLookup, name: &str) -> Option<WitTy> {
        match look {
            MemLookup::Fx(fx) => match fx.lookup(name)?.repr {
                Repr::Mem(t) => Some(self.mem_tys[t as usize].clone()),
                _ => None,
            },
            MemLookup::Sim(env) => env.iter().rev().find_map(|scope| scope.get(name)).cloned(),
        }
    }

    /// Whether field expression `v` can be stored directly at canonical
    /// field type `tf` with no observable difference from the boxed value
    /// the interpreter carries: exact kind, lossless width, and
    /// (recursively) faithful field order. Anything else keeps the record
    /// boxed.
    fn mem_field_ok(&self, look: &MemLookup, v: NodeId, tf: &WitTy) -> bool {
        use crate::check::Type as T;
        let vt = self.node_types.get(&v);
        match tf {
            WitTy::Bool => matches!(vt, Some(T::Bool)),
            WitTy::Char => matches!(vt, Some(T::Char)),
            WitTy::S64 => self.node_scalar(v) == Some(Scalar::Int),
            WitTy::F64 => self.node_scalar(v) == Some(Scalar::Float),
            WitTy::IntS(_) | WitTy::IntU(_) => {
                match (vt.and_then(check_int_range), wit_int_range(tf)) {
                    (Some((lo, hi)), Some((tlo, thi))) => tlo <= lo && hi <= thi,
                    _ => false,
                }
            }
            WitTy::Str => matches!(vt, Some(T::String)),
            WitTy::Record(_) => {
                matches!(self.arena.node(v), Node::Rec(_)) && self.can_mem_as(look, v, tf)
            }
            WitTy::List(_) => {
                matches!(self.arena.node(v), Node::Lst(_)) && self.can_mem_as(look, v, tf)
            }
            WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_) => {
                self.can_mem_as(look, v, tf)
            }
            // A flags literal goes canonical only when its members are a
            // duplicate-free subsequence of the declared members in
            // declaration order. The canonical bitset is order-free, but
            // `load_from_mem`/`lift_flags` rebuild the box in declaration
            // order, whereas the oracle's `Value::Flg` keeps source order
            // (its eq/to-string/patterns are order-observable). So a
            // reordered or duplicated literal (`{exec read}`, `{read read}`)
            // would round-trip to a different value and must stay boxed.
            WitTy::Flags(decl) => match self.arena.node(v) {
                Node::Flg(names) => flags_is_ordered_subseq(names, decl),
                _ => false,
            },
            // f32 (demoting would lose the f64 the oracle keeps), handles:
            // not yet
            _ => false,
        }
    }

    /// The canonical layout for `id` when 5.3/5.5 admit one: a known
    /// non-empty record or tuple, or a string or list, whose construction
    /// here is provably faithful.
    fn node_mem_ty(&self, look: &MemLookup, id: NodeId) -> Option<WitTy> {
        let t = self.node_types.get(&id)?.clone();
        let ty = self.wit_of_check_type(&t)?;
        if !(matches!(&ty, WitTy::Record(fs) if !fs.is_empty())
            || matches!(&ty, WitTy::Tuple(es) if !es.is_empty())
            || matches!(
                &ty,
                WitTy::Str | WitTy::List(_) | WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_)
            ))
        {
            return None;
        }
        if !self.can_mem_as(look, id, &ty) {
            return None;
        }
        Some(ty)
    }

    /// [`Self::node_mem_ty`], interned.
    fn node_mem(&mut self, fx: &FnCtx, id: NodeId) -> Option<MemTy> {
        let ty = self.node_mem_ty(&MemLookup::Fx(fx), id)?;
        Some(self.mem_ty(&ty))
    }

    /// Compute a def's representation signature from its declared parameter
    /// types and the checker's recorded type for its body (5.2), plus the
    /// 5.3 canonical-layout prediction for record-typed bodies. Anything
    /// the checker left gradual stays a boxed slot.
    fn def_sig(&mut self, params_id: NodeId, body: NodeId) -> FnSig {
        let ptys = crate::check::parse_params(self.arena, params_id);
        let params = ptys
            .iter()
            .map(|(_, t)| Repr::of_scalar(Scalar::of(t)))
            .collect();
        let result = if let Some(k) = self.node_types.get(&body).and_then(Scalar::of) {
            Repr::Scalar(k)
        } else if let Some(ty) = self.predict_body_mem(body, &mut Vec::new()) {
            Repr::Mem(self.mem_ty(&ty))
        } else {
            Repr::Boxed
        };
        FnSig { params, result }
    }

    /// Predict whether a def BODY will be emitted natively in canonical
    /// layout, and of which type. A def's result representation is fixed
    /// before any body is emitted, so this walks the same form kinds
    /// [`Self::expr_mem`] can route (record literals, Mem-bound names,
    /// If/Do/Let/the), simulating exactly the binding decisions `let_form`
    /// makes; anything else predicts boxed. `env` carries the simulated
    /// `Let` scopes.
    fn predict_body_mem(&self, id: NodeId, env: &mut Vec<HashMap<String, WitTy>>) -> Option<WitTy> {
        match self.arena.node(id).clone() {
            Node::Rec(_) | Node::Lst(_) => self.node_mem_ty(&MemLookup::Sim(env), id),
            Node::Sym(name) => env
                .iter()
                .rev()
                .find_map(|s| s.get(&name))
                .cloned()
                // `none` / a nullary case as the whole body (5.4)
                .or_else(|| self.node_mem_ty(&MemLookup::Sim(env), id)),
            Node::Tup(items) if !items.is_empty() => {
                if matches!(self.arena.node(items[0]), Node::Qsym(..)) {
                    return self.node_mem_ty(&MemLookup::Sim(env), id);
                }
                let Node::Sym(head) = self.arena.node(items[0]).clone() else {
                    return None;
                };
                let args = &items[1..];
                match (head.as_str(), args) {
                    // `If`/`Do` are stdlib macros that expand to `Match`
                    // (5.7); their Mem prediction is `Match`'s: every clause
                    // result must predict the same Mem layout. Pattern-bound
                    // names are not modelled here (best effort), so a clause
                    // body that depends on one predicts boxed — exactly as an
                    // unpredicted body did before.
                    ("match-MACRO", [_scrut, clauses]) => {
                        let Node::Lst(items) = self.arena.node(*clauses).clone() else {
                            return None;
                        };
                        let mut acc: Option<WitTy> = None;
                        for clause in items {
                            let Node::Tup(pair) = self.arena.node(clause).clone() else {
                                return None;
                            };
                            if pair.len() != 2 {
                                return None;
                            }
                            let r = self.predict_body_mem(pair[1], env)?;
                            match &acc {
                                None => acc = Some(r),
                                Some(prev) if *prev == r => {}
                                Some(_) => return None,
                            }
                        }
                        acc
                    }
                    ("let-MACRO", [bindings, body]) => {
                        let Node::Rec(fields) = self.arena.node(*bindings).clone() else {
                            return None;
                        };
                        env.push(HashMap::new());
                        for (k, v) in &fields {
                            // mirror let_form: scalar wins, then Mem, else boxed
                            if self.node_scalar(*v).is_none()
                                && let Some(ty) = self.node_mem_ty(&MemLookup::Sim(env), *v)
                            {
                                env.last_mut().unwrap().insert(k.clone(), ty);
                            }
                        }
                        let r = self.predict_body_mem(*body, env);
                        env.pop();
                        r
                    }
                    ("the-MACRO", [_ty, expr]) => self.predict_body_mem(*expr, env),
                    // a case-constructor call (5.4); non-constructors gate
                    // to None inside can_mem_as
                    _ => self.node_mem_ty(&MemLookup::Sim(env), id),
                }
            }
            _ => None,
        }
    }

    /// Emit `id` as a pointer to its canonical layout of type `t`
    /// ([`Repr::Mem`]). Control forms thread the Mem want through their
    /// result positions (mirroring the scalar path); leaf callers must have
    /// established eligibility via [`Self::can_mem_as`] /
    /// [`Self::node_mem`] / [`Self::predict_body_mem`].
    fn expr_mem(&mut self, fx: &mut FnCtx, id: NodeId, t: MemTy, tail: bool) -> Result<(), String> {
        let ty = self.mem_tys[t as usize].clone();
        match self.arena.node(id).clone() {
            // a binding already in this layout: alias it (values are
            // immutable, so sharing the memory is unobservable)
            Node::Sym(name) => {
                match fx.lookup(&name) {
                    Some(b) => match b.repr {
                        Repr::Mem(bt) if bt == t => {
                            fx.op(I::LocalGet(b.local));
                            Ok(())
                        }
                        // a boxed binding of a variant-shaped type stores
                        // canonically (name+payload — no order hazard)
                        Repr::Boxed if ty.variant_cases().is_some() => {
                            let l = fx.local(ValType::I32);
                            fx.op(I::LocalGet(b.local));
                            fx.op(I::LocalSet(l));
                            let a = fx.local(ValType::I32);
                            fx.op(I::I32Const(size_of(&ty) as i32));
                            fx.op(I::Call(self.h.alloc));
                            fx.op(I::LocalSet(a));
                            self.store_to_mem(fx, &ty, l, a, 0)?;
                            fx.op(I::LocalGet(a));
                            Ok(())
                        }
                        _ => Err("internal: Mem repr requested for a non-Mem binding".into()),
                    },
                    // `none` / a nullary local case as a value (5.4), or —
                    // if resolution differs from the prediction — the boxed
                    // fallback inside mem_var_into
                    None if ty.variant_cases().is_some() => {
                        let a = fx.local(ValType::I32);
                        fx.op(I::I32Const(size_of(&ty) as i32));
                        fx.op(I::Call(self.h.alloc));
                        fx.op(I::LocalSet(a));
                        self.mem_var_into(fx, id, &ty, a, 0)?;
                        fx.op(I::LocalGet(a));
                        Ok(())
                    }
                    None => Err("internal: Mem repr requested for an unbound name".into()),
                }
            }
            Node::Rec(_) => {
                let p = fx.local(ValType::I32);
                fx.op(I::I32Const(size_of(&ty) as i32));
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(p));
                self.expr_mem_into(fx, id, &ty, p, 0)?;
                fx.op(I::LocalGet(p));
                Ok(())
            }
            Node::Lst(_) => {
                // the canonical list VALUE is a pointer to its (ptr, len)
                // pair; the elements pack into their own buffer (5.5)
                let p = fx.local(ValType::I32);
                fx.op(I::I32Const(8));
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(p));
                self.mem_field_into(fx, id, &ty, p, 0)?;
                fx.op(I::LocalGet(p));
                Ok(())
            }
            Node::Tup(items) if !items.is_empty() => {
                if let Node::Qsym(alias, fname) = self.arena.node(items[0]).clone() {
                    // gated by can_mem_as: the call's retptr area IS the value
                    return self.dep_call(fx, &alias, &fname, &items[1..], Some(t));
                }
                if let Node::Sym(head) = self.arena.node(items[0]).clone() {
                    let args = &items[1..];
                    match head.as_str() {
                        "let-MACRO" => return self.let_form(fx, args, Repr::Mem(t), tail),
                        "match-MACRO" => return self.match_form(fx, args, Repr::Mem(t), tail),
                        "the-MACRO" => {
                            if let [_ty, expr] = *args {
                                return self.expr_mem(fx, expr, t, tail);
                            }
                        }
                        // `tail` of a canonical list, zero-copy (5.6): a fresh
                        // (ptr, len) pair whose data pointer is the operand's
                        // advanced one element and whose length is one less —
                        // it shares the operand's packed element buffer (values
                        // are immutable and the arena is not reset mid-call, so
                        // sharing is unobservable). Empty operand traps, like
                        // the boxed `tail_h` / the oracle's `tail` of empty.
                        "tail" if matches!(ty, WitTy::List(_)) => {
                            let WitTy::List(elem) = ty.clone() else {
                                unreachable!()
                            };
                            let [operand] = args else {
                                return Err("malformed tail".into());
                            };
                            let ot = self.node_mem(fx, *operand).ok_or(
                                "internal: canonical tail over a non-canonical operand",
                            )?;
                            let area = fx.local(ValType::I32);
                            self.expr_mem(fx, *operand, ot, false)?;
                            fx.op(I::LocalSet(area));
                            // trap on the empty list
                            fx.op(I::LocalGet(area));
                            fx.op(I::I32Load(ma(4, 2)));
                            fx.op(I::I32Eqz);
                            fx.op(I::If(BlockType::Empty));
                            fx.op(I::Unreachable);
                            fx.op(I::End);
                            let p = fx.local(ValType::I32);
                            fx.op(I::I32Const(8));
                            fx.op(I::Call(self.h.alloc));
                            fx.op(I::LocalSet(p));
                            // new.ptr = operand.ptr + elem_size
                            fx.op(I::LocalGet(p));
                            fx.op(I::LocalGet(area));
                            fx.op(I::I32Load(ma(0, 2)));
                            fx.op(I::I32Const(elem_size(&elem) as i32));
                            fx.op(I::I32Add);
                            fx.op(I::I32Store(ma(0, 2)));
                            // new.len = operand.len - 1
                            fx.op(I::LocalGet(p));
                            fx.op(I::LocalGet(area));
                            fx.op(I::I32Load(ma(4, 2)));
                            fx.op(I::I32Const(1));
                            fx.op(I::I32Sub);
                            fx.op(I::I32Store(ma(4, 2)));
                            fx.op(I::LocalGet(p));
                            return Ok(());
                        }
                        _ if ty.variant_cases().is_some() => {
                            // a case-constructor call builds disc+payload in
                            // place (5.4); mem_var_into falls back to the
                            // boxed store when resolution differs from the
                            // gate's view
                            let a = fx.local(ValType::I32);
                            fx.op(I::I32Const(size_of(&ty) as i32));
                            fx.op(I::Call(self.h.alloc));
                            fx.op(I::LocalSet(a));
                            self.mem_var_into(fx, id, &ty, a, 0)?;
                            fx.op(I::LocalGet(a));
                            return Ok(());
                        }
                        _ => {}
                    }
                }
                Err("internal: expression is not Mem-eligible (5.3)".into())
            }
            _ => Err("internal: expression is not Mem-eligible (5.3)".into()),
        }
    }

    /// Construct `id`'s value directly INTO canonical memory at `dst + off`
    /// (in place: a nested record writes its fields into the parent's
    /// interior — no intermediate allocation, no boxes).
    fn expr_mem_into(
        &mut self,
        fx: &mut FnCtx,
        id: NodeId,
        ty: &WitTy,
        dst: u32,
        off: u64,
    ) -> Result<(), String> {
        let Node::Rec(fields) = self.arena.node(id).clone() else {
            return Err("internal: expr_mem_into expects a record literal".into());
        };
        for ((o, tf), (_, v)) in record_field_offsets(ty).into_iter().zip(&fields) {
            self.mem_field_into(fx, *v, &tf, dst, off + o)?;
        }
        Ok(())
    }

    /// Store a variant-shaped value at `dst + off` (5.4): a case-constructor
    /// form stores its numeric discriminant and constructs the payload in
    /// place at the canonical payload offset (bundling several arguments as
    /// a tuple payload, the interpreter's rule). Anything else — a bound
    /// name, a call, a resolution the prediction walk could not see —
    /// evaluates boxed and stores through the canonical seam: a variant's
    /// value is its case name plus payload, so unlike records there is no
    /// field-order hazard in the box-to-mem direction.
    fn mem_var_into(
        &mut self,
        fx: &mut FnCtx,
        id: NodeId,
        ty: &WitTy,
        dst: u32,
        off: u64,
    ) -> Result<(), String> {
        let parts = self.ctor_parts(Some(fx), id, ty).filter(|_| {
            let look = MemLookup::Fx(fx);
            self.ctor_admissible(&look, id, ty)
        });
        let Some((i, payload, args)) = parts else {
            let l = fx.local(ValType::I32);
            self.expr(fx, id, false)?;
            fx.op(I::LocalSet(l));
            return self.store_to_mem(fx, ty, l, dst, off);
        };
        fx.op(I::LocalGet(dst));
        fx.op(I::I32Const(i as i32));
        fx.op(I::I32Store8(ma(off, 0)));
        let poff = variant_payload_offset(ty);
        match (args.len(), payload) {
            (0, None) => {}
            (1, Some(pt)) => self.mem_field_into(fx, args[0], &pt, dst, off + poff)?,
            (_, Some(WitTy::Tuple(es))) => {
                let t = WitTy::Tuple(es);
                for ((o, et), &a) in record_field_offsets(&t).into_iter().zip(&args) {
                    self.mem_field_into(fx, a, &et, dst, off + poff + o)?;
                }
            }
            _ => return Err("internal: ctor_admissible admitted a payload/arity mismatch".into()),
        }
        Ok(())
    }

    /// Store field expression `v` at canonical field type `tf`, `dst + off`.
    /// Scalar fields evaluate unboxed and store at WIT width (lossless — the
    /// gate verified the static range); strings evaluate boxed and store
    /// through the boundary seam; nested records construct in place.
    fn mem_field_into(
        &mut self,
        fx: &mut FnCtx,
        v: NodeId,
        tf: &WitTy,
        dst: u32,
        off: u64,
    ) -> Result<(), String> {
        match tf {
            WitTy::Bool => {
                fx.op(I::LocalGet(dst));
                self.expr_scalar(fx, v, Scalar::Bool)?;
                fx.op(I::I32Store8(ma(off, 0)));
            }
            WitTy::Char => {
                fx.op(I::LocalGet(dst));
                self.expr_scalar(fx, v, Scalar::Char)?;
                fx.op(I::I32WrapI64);
                fx.op(I::I32Store(ma(off, 2)));
            }
            WitTy::IntS(w) | WitTy::IntU(w) => {
                fx.op(I::LocalGet(dst));
                self.expr_scalar(fx, v, Scalar::Int)?;
                fx.op(I::I32WrapI64);
                match *w {
                    1 => fx.op(I::I32Store8(ma(off, 0))),
                    2 => fx.op(I::I32Store16(ma(off, 1))),
                    _ => fx.op(I::I32Store(ma(off, 2))),
                }
            }
            WitTy::S64 => {
                fx.op(I::LocalGet(dst));
                self.expr_scalar(fx, v, Scalar::Int)?;
                fx.op(I::I64Store(ma(off, 3)));
            }
            WitTy::F64 => {
                fx.op(I::LocalGet(dst));
                self.expr_scalar(fx, v, Scalar::Float)?;
                fx.op(I::F64Store(ma(off, 3)));
            }
            WitTy::Str => {
                let l = fx.local(ValType::I32);
                self.expr(fx, v, false)?;
                fx.op(I::LocalSet(l));
                self.store_to_mem(fx, tf, l, dst, off)?;
            }
            WitTy::Record(_) => self.expr_mem_into(fx, v, tf, dst, off)?,
            WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_) => {
                self.mem_var_into(fx, v, tf, dst, off)?;
            }
            WitTy::Flags(decl) => {
                // `mem_field_ok` admits only a flags literal whose members are
                // an ordered, duplicate-free subsequence of `decl`, so the
                // bitset is a compile-time constant: set the declared-position
                // bit for each listed member. Stored at the canonical width.
                let Node::Flg(names) = self.arena.node(v) else {
                    unreachable!("mem_field_ok admits only a flags literal here")
                };
                let mut bits: i32 = 0;
                for name in names {
                    let i = decl
                        .iter()
                        .position(|d| d == name)
                        .expect("subsequence member is declared");
                    bits |= 1 << i;
                }
                fx.op(I::LocalGet(dst));
                fx.op(I::I32Const(bits));
                match decl.len() {
                    0..=8 => fx.op(I::I32Store8(ma(off, 0))),
                    9..=16 => fx.op(I::I32Store16(ma(off, 1))),
                    _ => fx.op(I::I32Store(ma(off, 2))),
                }
            }
            WitTy::List(elem) => {
                // a list literal packs its elements at their canonical
                // stride into a fresh buffer; the field itself is the
                // (ptr, len) pair (5.5)
                let Node::Lst(items) = self.arena.node(v).clone() else {
                    return Err("internal: canonical list store expects a list literal".into());
                };
                let esz = elem_size(elem);
                let buf = fx.local(ValType::I32);
                fx.op(I::I32Const((items.len() as u64 * esz) as i32));
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(buf));
                for (i, &it) in items.iter().enumerate() {
                    self.mem_field_into(fx, it, elem, buf, i as u64 * esz)?;
                }
                fx.op(I::LocalGet(dst));
                fx.op(I::LocalGet(buf));
                fx.op(I::I32Store(ma(off, 2)));
                fx.op(I::LocalGet(dst));
                fx.op(I::I32Const(items.len() as i32));
                fx.op(I::I32Store(ma(off + 4, 2)));
            }
            _ => return Err("internal: field type not Mem-storable yet (5.3)".into()),
        }
        Ok(())
    }

    /// Emit expression `id` UNBOXED as scalar kind `want` (goal 5, 5.2).
    ///
    /// The caller must have established that `id`'s static value kind is
    /// `want` — or is `Int` while `want` is `Float`, which widens like the
    /// interpreter's `want_num` in mixed arithmetic. Literals and nested
    /// scalar operations compile natively with no intermediate boxes;
    /// anything else falls back to the boxed emitter plus one unbox. The
    /// unbox helpers trap on a tag the static type ruled out, exactly where
    /// the boxed path traps inside the polymorphic runtime helper, so the
    /// two representations fail on the same programs.
    fn expr_scalar(&mut self, fx: &mut FnCtx, id: NodeId, want: Scalar) -> Result<(), String> {
        self.expr_scalar_t(fx, id, want, false)
    }

    /// Emit `id` in representation `want` — `None` = boxed (an i32 box
    /// pointer), `Some(kind)` = unboxed scalar. The single entry point that
    /// lets control forms and internal calls carry a typed result through
    /// tail position (5.2).
    fn expr_repr(
        &mut self,
        fx: &mut FnCtx,
        id: NodeId,
        want: Repr,
        tail: bool,
    ) -> Result<(), String> {
        match want {
            Repr::Boxed => self.expr(fx, id, tail),
            Repr::Scalar(k) => self.expr_scalar_t(fx, id, k, tail),
            Repr::Mem(t) => self.expr_mem(fx, id, t, tail),
        }
    }

    /// [`Self::expr_scalar`] with tail-position awareness: control forms
    /// thread `tail` into their result branches, and a tail call to an
    /// internal function with the same result representation compiles to
    /// `return_call` (preserving the tail recursion the boxed path has).
    fn expr_scalar_t(
        &mut self,
        fx: &mut FnCtx,
        id: NodeId,
        want: Scalar,
        tail: bool,
    ) -> Result<(), String> {
        match self.arena.node(id).clone() {
            Node::Int(n) => {
                if want == Scalar::Float {
                    // an int literal in float context: the interpreter widens
                    fx.op(I::F64Const((n as f64).into()));
                } else {
                    fx.op(I::I64Const(n));
                }
                return Ok(());
            }
            Node::Dec(d) => {
                fx.op(I::F64Const(d.into()));
                return Ok(());
            }
            Node::Bool(b) => {
                fx.op(I::I32Const(b as i32));
                return Ok(());
            }
            Node::Char(c) => {
                fx.op(I::I64Const(c as u32 as i64));
                return Ok(());
            }
            Node::Sym(name) => {
                // a goal-5 typed local reads directly — no box round-trip
                if let Some(b) = fx.lookup(&name)
                    && let Repr::Scalar(kind) = b.repr
                {
                    fx.op(I::LocalGet(b.local));
                    if kind == Scalar::Int && want == Scalar::Float {
                        fx.op(I::F64ConvertI64S);
                    }
                    return Ok(());
                }
            }
            Node::Tup(items) if !items.is_empty() => {
                if let Node::Sym(name) = self.arena.node(items[0]).clone() {
                    let args = &items[1..];
                    // Control forms carry the typed result straight through
                    // (including tail position). Dispatch order mirrors
                    // `call`: special forms, then locals, then builtins,
                    // then internal defs.
                    match name.as_str() {
                        "let-MACRO" => return self.let_form(fx, args, Repr::Scalar(want), tail),
                        "match-MACRO" => {
                            return self.match_form(fx, args, Repr::Scalar(want), tail);
                        }
                        "the-MACRO" => {
                            if let [_ty, expr] = *args {
                                return self.expr_scalar_t(fx, expr, want, tail);
                            }
                        }
                        _ => {}
                    }
                    if fx.lookup(&name).is_none() {
                        if BUILTINS.contains(&name.as_str()) {
                            if let Some(kind) = self.scalar_op(fx, &name, args)? {
                                if kind == Scalar::Int && want == Scalar::Float {
                                    fx.op(I::F64ConvertI64S);
                                }
                                return Ok(());
                            }
                        } else if self.funcs.contains_key(name.as_str()) {
                            return self.internal_call(fx, &name, args, Repr::Scalar(want), tail);
                        }
                    }
                }
            }
            _ => {}
        }
        // Boxed fallback + one unbox at the seam.
        self.expr(fx, id, false)?;
        if self.node_scalar(id) == Some(Scalar::Int) && want == Scalar::Float {
            fx.op(I::Call(self.h.unbox_int));
            fx.op(I::F64ConvertI64S);
        } else {
            self.unbox_scalar(fx, want);
        }
        Ok(())
    }

    /// If `name(args)` is a scalar builtin whose operand kinds are statically
    /// known, emit it UNBOXED and return the result kind; `Ok(None)` (with
    /// nothing emitted) means "not eligible — use the boxed path". This is
    /// the goal-5 static resolution of the polymorphic builtins (5.6.1):
    /// arithmetic picks the int/float path at compile time, comparisons pick
    /// codepoint/numeric at compile time, and eq compiles to one machine
    /// comparison. Semantics are shared with the boxed path via the
    /// arith_int/cmp_f64 helper cores.
    fn scalar_op(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        args: &[NodeId],
    ) -> Result<Option<Scalar>, String> {
        use Scalar::*;
        match name {
            "add" | "sub" | "mul" | "div" | "rem" if args.len() == 2 => {
                let (Some(ka), Some(kb)) = (self.node_scalar(args[0]), self.node_scalar(args[1]))
                else {
                    return Ok(None);
                };
                if !matches!(ka, Int | Float) || !matches!(kb, Int | Float) {
                    return Ok(None);
                }
                if ka == Float || kb == Float {
                    // float path: both widened to f64, like the interpreter
                    self.expr_scalar(fx, args[0], Float)?;
                    self.expr_scalar(fx, args[1], Float)?;
                    match name {
                        "add" => fx.op(I::F64Add),
                        "sub" => fx.op(I::F64Sub),
                        "mul" => fx.op(I::F64Mul),
                        "div" => fx.op(I::F64Div),
                        _ => {
                            // rem: x - trunc(x/y)*y (Rust f64 `%`, like arith_raw)
                            let y = fx.local(ValType::F64);
                            let x = fx.local(ValType::F64);
                            fx.op(I::LocalSet(y));
                            fx.op(I::LocalSet(x));
                            fx.op(I::LocalGet(x));
                            fx.op(I::LocalGet(x));
                            fx.op(I::LocalGet(y));
                            fx.op(I::F64Div);
                            fx.op(I::F64Trunc);
                            fx.op(I::LocalGet(y));
                            fx.op(I::F64Mul);
                            fx.op(I::F64Sub);
                        }
                    }
                    Ok(Some(Float))
                } else {
                    // int path: the shared checked-arithmetic core
                    self.expr_scalar(fx, args[0], Int)?;
                    self.expr_scalar(fx, args[1], Int)?;
                    fx.op(I::I32Const(match name {
                        "add" => 0,
                        "sub" => 1,
                        "mul" => 2,
                        "div" => 3,
                        _ => 4,
                    }));
                    fx.op(I::Call(self.h.arith_int));
                    Ok(Some(Int))
                }
            }
            "neg" if args.len() == 1 => match self.node_scalar(args[0]) {
                Some(Int) => {
                    // wrapping 0 - x, exactly neg_raw's int arm
                    fx.op(I::I64Const(0));
                    self.expr_scalar(fx, args[0], Int)?;
                    fx.op(I::I64Sub);
                    Ok(Some(Int))
                }
                Some(Float) => {
                    self.expr_scalar(fx, args[0], Float)?;
                    fx.op(I::F64Neg);
                    Ok(Some(Float))
                }
                _ => Ok(None),
            },
            "lt" | "le" | "gt" | "ge" if args.len() == 2 => {
                let (Some(ka), Some(kb)) = (self.node_scalar(args[0]), self.node_scalar(args[1]))
                else {
                    return Ok(None);
                };
                match (ka, kb) {
                    (Char, Char) => {
                        // by codepoint, like cmp_raw's char arm
                        self.expr_scalar(fx, args[0], Char)?;
                        self.expr_scalar(fx, args[1], Char)?;
                    }
                    (Int | Float, Int | Float) => {
                        // widened to f64 (cmp_f64 core), like the
                        // interpreter's `compare` — ints included
                        self.expr_scalar(fx, args[0], Float)?;
                        self.expr_scalar(fx, args[1], Float)?;
                        fx.op(I::Call(self.h.cmp_f64));
                        fx.op(I::I64ExtendI32S);
                        fx.op(I::I64Const(0));
                    }
                    // strings keep the boxed cmp_raw; mixed char/number is a
                    // runtime error either way (boxed path traps in as_f64)
                    _ => return Ok(None),
                }
                fx.op(match name {
                    "lt" => I::I64LtS,
                    "le" => I::I64LeS,
                    "gt" => I::I64GtS,
                    _ => I::I64GeS,
                });
                Ok(Some(Bool))
            }
            "eq" if args.len() == 2 => {
                let (Some(ka), Some(kb)) = (self.node_scalar(args[0]), self.node_scalar(args[1]))
                else {
                    return Ok(None);
                };
                if ka != kb {
                    // Int-vs-Float etc. is `false` at the value level, but the
                    // boxed eq_raw already answers that: keep one source of
                    // truth for cross-kind eq.
                    return Ok(None);
                }
                self.expr_scalar(fx, args[0], ka)?;
                self.expr_scalar(fx, args[1], ka)?;
                fx.op(match ka {
                    Int | Char => I::I64Eq,
                    Float => I::F64Eq,
                    Bool => I::I32Eq,
                });
                Ok(Some(Bool))
            }
            "not" if args.len() == 1 => {
                if self.node_scalar(args[0]) != Some(Bool) {
                    return Ok(None);
                }
                self.expr_scalar(fx, args[0], Bool)?;
                fx.op(I::I32Eqz);
                Ok(Some(Bool))
            }
            // Numeric conversions (5.6): the interpreter accepts an int
            // (range-checked), a char (its codepoint), or a whole float
            // (truncated), and yields Value::Int; to-f32/f64 widen to
            // Value::Dec. Statically-typed operands compile per kind; a
            // gradual operand keeps the honest "not supported" error.
            "to-u8" | "to-u16" | "to-u32" | "to-u64" | "to-s8" | "to-s16" | "to-s32" | "to-s64"
                if args.len() == 1 =>
            {
                match self.node_scalar(args[0]) {
                    Some(k @ (Int | Char)) => self.expr_scalar(fx, args[0], k)?,
                    Some(Float) => {
                        // `Dec(f) if f.fract() == 0.0` — else the interpreter
                        // errors (NaN included); then Rust's saturating `as`
                        self.expr_scalar(fx, args[0], Float)?;
                        let f = fx.local(ValType::F64);
                        fx.op(I::LocalTee(f));
                        fx.op(I::LocalGet(f));
                        fx.op(I::F64Trunc);
                        fx.op(I::F64Ne); // true for any fraction and for NaN
                        fx.op(I::If(BlockType::Empty));
                        fx.op(I::Unreachable);
                        fx.op(I::End);
                        fx.op(I::LocalGet(f));
                        fx.op(I::I64TruncSatF64S);
                    }
                    _ => return Ok(None),
                }
                // range check, trapping exactly where the interpreter errors
                let range = match name {
                    "to-u8" => Some((0, u8::MAX as i64)),
                    "to-u16" => Some((0, u16::MAX as i64)),
                    "to-u32" => Some((0, u32::MAX as i64)),
                    "to-u64" => Some((0, i64::MAX)),
                    "to-s8" => Some((i8::MIN as i64, i8::MAX as i64)),
                    "to-s16" => Some((i16::MIN as i64, i16::MAX as i64)),
                    "to-s32" => Some((i32::MIN as i64, i32::MAX as i64)),
                    _ => None, // to-s64: every i64 is in range
                };
                if let Some((lo, hi)) = range {
                    let n = fx.local(ValType::I64);
                    fx.op(I::LocalTee(n));
                    fx.op(I::I64Const(lo));
                    fx.op(I::I64LtS);
                    fx.op(I::LocalGet(n));
                    fx.op(I::I64Const(hi));
                    fx.op(I::I64GtS);
                    fx.op(I::I32Or);
                    fx.op(I::If(BlockType::Empty));
                    fx.op(I::Unreachable);
                    fx.op(I::End);
                    fx.op(I::LocalGet(n));
                }
                Ok(Some(Int))
            }
            "to-f32" | "to-f64" if args.len() == 1 => match self.node_scalar(args[0]) {
                Some(Int | Float) => {
                    // want_num widening: int → f64, float unchanged
                    self.expr_scalar(fx, args[0], Float)?;
                    Ok(Some(Float))
                }
                _ => Ok(None),
            },
            "to-char" if args.len() == 1 => match self.node_scalar(args[0]) {
                Some(Char) => {
                    // a char passes through
                    self.expr_scalar(fx, args[0], Char)?;
                    Ok(Some(Char))
                }
                Some(Int) => {
                    // must be a Unicode scalar: ≤ 0x10FFFF (unsigned, catches
                    // negatives) and not a surrogate
                    self.expr_scalar(fx, args[0], Int)?;
                    let n = fx.local(ValType::I64);
                    fx.op(I::LocalTee(n));
                    fx.op(I::I64Const(0x10FFFF));
                    fx.op(I::I64GtU);
                    fx.op(I::If(BlockType::Empty));
                    fx.op(I::Unreachable);
                    fx.op(I::End);
                    fx.op(I::LocalGet(n));
                    fx.op(I::I64Const(0xD800));
                    fx.op(I::I64GeU);
                    fx.op(I::LocalGet(n));
                    fx.op(I::I64Const(0xDFFF));
                    fx.op(I::I64LeU);
                    fx.op(I::I32And);
                    fx.op(I::If(BlockType::Empty));
                    fx.op(I::Unreachable);
                    fx.op(I::End);
                    fx.op(I::LocalGet(n));
                    Ok(Some(Char))
                }
                _ => Ok(None),
            },
            _ => Ok(None),
        }
    }

    fn builtin(&mut self, fx: &mut FnCtx, name: &str, args: &[NodeId]) -> Result<(), String> {
        // Goal 5 (5.2/5.6.1): when the operand types are statically known
        // scalars, compile the polymorphic builtin as unboxed per-type code
        // and box once at the seam. Ineligible calls (unknown/compound
        // operand types, strings, arity errors) fall through to the boxed
        // arms below unchanged.
        if matches!(
            name,
            "eq" | "not"
                | "lt"
                | "le"
                | "gt"
                | "ge"
                | "add"
                | "sub"
                | "mul"
                | "div"
                | "rem"
                | "neg"
                | "to-u8"
                | "to-u16"
                | "to-u32"
                | "to-u64"
                | "to-s8"
                | "to-s16"
                | "to-s32"
                | "to-s64"
                | "to-f32"
                | "to-f64"
                | "to-char"
        ) && let Some(kind) = self.scalar_op(fx, name, args)?
        {
            self.box_scalar(fx, kind);
            return Ok(());
        }
        let items = args;
        // Boxed-path dispatch: route each builtin to the helper that owns its
        // group. Every arm below emits the polymorphic, box-once code for its
        // family; the helpers share the `nargs` arity check and operate on the
        // `items` operand slice. Adding a builtin means extending both this
        // classifier and the relevant `builtin_*` method (plus `BUILTINS`).
        match name {
            "eq" | "not" | "lt" | "le" | "gt" | "ge" | "add" | "sub" | "mul" | "div" | "rem" | "neg" | "abs" | "min" | "max" => self.builtin_numeric(fx, name, items),
            "len" | "head" | "tail" | "reverse" | "range" | "empty" | "drop" => self.builtin_seq(fx, name, items),
            "map" | "fold" | "filter" | "zip" | "apply" => self.builtin_higher_order(fx, name, items),
            "get" | "put" | "push" | "concat" => self.builtin_index(fx, name, items),
            "contains" | "join" | "split" => self.builtin_search(fx, name, items),
            "str-cat" | "to-string" | "to-char" | "upper" | "lower" => self.builtin_string(fx, name, items),
            "some" | "ok" | "err" => self.builtin_variant(fx, name, items),
            "cell-new" | "cell-get" | "cell-set" => self.builtin_cell(fx, name, items),
            "form-kind" | "rec-key" | "rec-val" | "gensym" | "expand" => self.builtin_form(fx, name, items),
            other => Err(format!(
                "builtin `{other}` not supported by the wasm backend yet"
            )),
        }
    }

    /// Comparison and arithmetic builtins: `eq`/`not`, the ordering
    /// comparisons, the binary arithmetic ops, `neg`/`abs`, and `min`/`max`.
    /// These are the boxed fallbacks for calls the unboxed scalar fast path
    /// in `builtin` did not claim (unknown/compound operands, arity errors).
    fn builtin_numeric(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` expects {want} argument(s), got {}",
                    items.len()
                ))
            }
        };
        match name {
            "eq" => {
                nargs(2)?;
                // Type-indexed structural eq (5.6): when both operands are the
                // SAME statically-canonical Mem layout, compare their canonical
                // representations field-by-field instead of reboxing both and
                // calling `eq_raw`. A reordered/renamed record literal never
                // reaches the same MemTy (`can_mem_as` requires declaration
                // order), so field-order-sensitive `eq` stays correct; flags
                // and handles are ineligible (see `mem_eq_eligible`).
                if let (Some(ta), Some(tb)) =
                    (self.node_mem(fx, items[0]), self.node_mem(fx, items[1]))
                    && ta == tb
                {
                    let ty = self.mem_tys[ta as usize].clone();
                    if self.mem_eq_eligible(&ty) {
                        let pa = fx.local(ValType::I32);
                        self.expr_mem(fx, items[0], ta, false)?;
                        fx.op(I::LocalSet(pa));
                        let pb = fx.local(ValType::I32);
                        self.expr_mem(fx, items[1], tb, false)?;
                        fx.op(I::LocalSet(pb));
                        self.emit_mem_eq(fx, &ty, pa, pb, 0)?;
                        fx.op(I::Call(self.h.box_bool));
                        return Ok(());
                    }
                }
                self.expr(fx, items[0], false)?;
                self.expr(fx, items[1], false)?;
                fx.op(I::Call(self.h.eq_raw));
                fx.op(I::Call(self.h.box_bool));
            }
            "not" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.truthy));
                fx.op(I::I32Eqz);
                fx.op(I::Call(self.h.box_bool));
            }
            "lt" | "le" | "gt" | "ge" => {
                // cmp_raw yields -1/0/1 over ints, decs, strings and chars,
                // matching the interpreter's `compare`.
                nargs(2)?;
                self.expr(fx, items[0], false)?;
                self.expr(fx, items[1], false)?;
                fx.op(I::Call(self.h.cmp_raw));
                fx.op(I::I32Const(0));
                fx.op(match name {
                    "lt" => I::I32LtS,
                    "le" => I::I32LeS,
                    "gt" => I::I32GtS,
                    _ => I::I32GeS,
                });
                fx.op(I::Call(self.h.box_bool));
            }
            "add" | "sub" | "mul" | "div" | "rem" => {
                // strictly binary, like the interpreter's `args_n(arg, 2)`;
                // arith_raw dispatches int (checked) vs float at runtime.
                nargs(2)?;
                self.expr(fx, items[0], false)?;
                self.expr(fx, items[1], false)?;
                fx.op(I::I32Const(match name {
                    "add" => 0,
                    "sub" => 1,
                    "mul" => 2,
                    "div" => 3,
                    _ => 4,
                }));
                fx.op(I::Call(self.h.arith_raw));
            }
            "neg" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.neg_raw));
            }
            "abs" => {
                // |n| over an int (branch: n<0 ? -n : n) or a dec (F64Abs);
                // any other tag traps, matching the oracle's `want_num`.
                nargs(1)?;
                let b = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(b));
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_INT));
                fx.op(I::I32Eq);
                fx.op(I::If(BlockType::Result(ValType::I32)));
                let n = fx.local(ValType::I64);
                fx.op(I::LocalGet(b));
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::LocalTee(n));
                fx.op(I::I64Const(0));
                fx.op(I::I64LtS);
                fx.op(I::If(BlockType::Result(ValType::I64)));
                fx.op(I::I64Const(0));
                fx.op(I::LocalGet(n));
                fx.op(I::I64Sub);
                fx.op(I::Else);
                fx.op(I::LocalGet(n));
                fx.op(I::End);
                fx.op(I::Call(self.h.box_int));
                fx.op(I::Else);
                // must be a dec, else trap (want_num)
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_DEC));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(b));
                fx.op(I::F64Load(ma(8, 3)));
                fx.op(I::F64Abs);
                fx.op(I::Call(self.h.box_dec));
                fx.op(I::End);
            }
            "min" | "max" => {
                // The oracle: `min` returns a[1] when compare(a,b)==Greater else
                // a[0]; `max` returns a[1] when compare(a,b)==Less else a[0].
                // `cmp_raw` yields 1 for Greater, -1 for Less over the same total
                // order (ints, decs, strings, chars).
                nargs(2)?;
                let av = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(av));
                let bv = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::LocalSet(bv));
                fx.op(I::LocalGet(av));
                fx.op(I::LocalGet(bv));
                fx.op(I::Call(self.h.cmp_raw));
                fx.op(I::I32Const(if name == "min" { 1 } else { -1 }));
                fx.op(I::I32Eq);
                fx.op(I::If(BlockType::Result(ValType::I32)));
                fx.op(I::LocalGet(bv));
                fx.op(I::Else);
                fx.op(I::LocalGet(av));
                fx.op(I::End);
            }
            _ => unreachable!("builtin_numeric routed unexpected name `{name}`"),
        }
        Ok(())
    }

    /// Sequence access and construction: `len`/`head`/`tail`, `reverse`,
    /// `range`, the `empty` predicate, and `drop` (evaluate for effect →
    /// unit).
    fn builtin_seq(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` expects {want} argument(s), got {}",
                    items.len()
                ))
            }
        };
        match name {
            "len" => {
                nargs(1)?;
                // Type-indexed (5.6): a statically-canonical LIST operand's
                // length IS the len word of its (ptr, len) area — read it
                // directly instead of reboxing and walking `len_raw`. Only
                // lists qualify: a string's `len` is its CHAR count (not the
                // byte length the word stores), and a tuple's canonical layout
                // is inline fields (no len word) — both keep the boxed path.
                if let Some(mt) = self.node_mem(fx, items[0])
                    && matches!(self.mem_tys[mt as usize], WitTy::List(_))
                {
                    self.expr_mem(fx, items[0], mt, false)?;
                    fx.op(I::I32Load(ma(4, 2)));
                    fx.op(I::I64ExtendI32U);
                    fx.op(I::Call(self.h.box_int));
                    return Ok(());
                }
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.len_raw));
                fx.op(I::I64ExtendI32U);
                fx.op(I::Call(self.h.box_int));
            }
            "head" => {
                nargs(1)?;
                // Type-indexed (5.6): a statically-canonical LIST operand's
                // first element loads straight from its (ptr, len) area — no
                // reboxing the whole list. Matches the oracle: `head` of an
                // empty list traps; otherwise it yields element[0] at its
                // natural boxed repr (exactly what `load_from_mem` produces at
                // the reference seam).
                if let Some(mt) = self.node_mem(fx, items[0])
                    && let WitTy::List(elem) = self.mem_tys[mt as usize].clone()
                {
                    let area = fx.local(ValType::I32);
                    self.expr_mem(fx, items[0], mt, false)?;
                    fx.op(I::LocalSet(area));
                    // trap on the empty list, like the boxed `head_h`
                    fx.op(I::LocalGet(area));
                    fx.op(I::I32Load(ma(4, 2)));
                    fx.op(I::I32Eqz);
                    fx.op(I::If(BlockType::Empty));
                    fx.op(I::Unreachable);
                    fx.op(I::End);
                    let dataptr = fx.local(ValType::I32);
                    fx.op(I::LocalGet(area));
                    fx.op(I::I32Load(ma(0, 2)));
                    fx.op(I::LocalSet(dataptr));
                    self.load_from_mem(fx, &elem, dataptr, 0)?;
                    return Ok(());
                }
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.head_h));
            }
            "tail" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.tail_h));
            }
            "reverse" => {
                // Fresh list with the elements in reverse order (oracle: want_list
                // then reverse). A non-list operand traps.
                nargs(1)?;
                let lst = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(lst));
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n));
                let out = fx.local(ValType::I32);
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(out));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(n));
                fx.op(I::I32Store(ma(4, 2)));
                let i = fx.local(ValType::I32);
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(n));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(1));
                fx.op(I::I32Sub);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Sub);
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(i));
                fx.op(I::Br(0));
                fx.op(I::End);
                fx.op(I::End);
                fx.op(I::LocalGet(out));
            }
            "range" => {
                // The half-open list [lo, hi) of ints (oracle: (lo..hi).map(Int)).
                // Empty when hi <= lo.
                nargs(2)?;
                let lo = fx.local(ValType::I64);
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::LocalSet(lo));
                let hi = fx.local(ValType::I64);
                self.expr(fx, items[1], false)?;
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::LocalSet(hi));
                let cnt = fx.local(ValType::I32);
                fx.op(I::LocalGet(hi));
                fx.op(I::LocalGet(lo));
                fx.op(I::I64GtS);
                fx.op(I::If(BlockType::Result(ValType::I32)));
                fx.op(I::LocalGet(hi));
                fx.op(I::LocalGet(lo));
                fx.op(I::I64Sub);
                fx.op(I::I32WrapI64);
                fx.op(I::Else);
                fx.op(I::I32Const(0));
                fx.op(I::End);
                fx.op(I::LocalSet(cnt));
                let out = fx.local(ValType::I32);
                fx.op(I::LocalGet(cnt));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(out));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(cnt));
                fx.op(I::I32Store(ma(4, 2)));
                let i = fx.local(ValType::I32);
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(cnt));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::LocalGet(lo));
                fx.op(I::LocalGet(i));
                fx.op(I::I64ExtendI32S);
                fx.op(I::I64Add);
                fx.op(I::Call(self.h.box_int));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(i));
                fx.op(I::Br(0));
                fx.op(I::End);
                fx.op(I::End);
                fx.op(I::LocalGet(out));
            }
            "empty" => {
                // true iff a string/list/tuple has length 0 (the len word @4);
                // any other tag traps, like the oracle's non-sequence error.
                nargs(1)?;
                let b = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(b));
                let tg = fx.local(ValType::I32);
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::LocalTee(tg));
                fx.op(I::I32Const(TAG_STR));
                fx.op(I::I32Eq);
                fx.op(I::LocalGet(tg));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Eq);
                fx.op(I::I32Or);
                fx.op(I::LocalGet(tg));
                fx.op(I::I32Const(TAG_TUP));
                fx.op(I::I32Eq);
                fx.op(I::I32Or);
                fx.op(I::I32Eqz);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::I32Eqz);
                fx.op(I::Call(self.h.box_bool));
            }
            "drop" => {
                // Evaluates its operand(s) for effect and yields unit — the
                // interpreter's `drop`. Unit is `Value::Rec(vec![])` (prints as
                // "{}"), so return a static empty-record box rather than the
                // `unit_addr()` false-box (which `to-string` would render
                // "false"); at a no-result WIT boundary the box is discarded.
                for &x in items {
                    self.expr(fx, x, false)?;
                    fx.op(I::Drop);
                }
                let addr = self.intern_unit_rec();
                fx.op(I::I32Const(addr as i32));
            }
            _ => unreachable!("builtin_seq routed unexpected name `{name}`"),
        }
        Ok(())
    }

    /// Builtins that take a callable or combine whole sequences: `map`,
    /// `fold`, `filter`, `zip`, and `apply`.
    fn builtin_higher_order(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` expects {want} argument(s), got {}",
                    items.len()
                ))
            }
        };
        match name {
            "map" => {
                // map(f, list) -> list: apply the function value `f` to each
                // element, collecting results in source order (oracle:
                // `for v in lst { out.push(interp.apply(&f, v)) }`). Length is
                // preserved, so the result TAG_LIST box is pre-sized and filled.
                // `f` is applied through the boxed-closure convention
                // (`closure_call`): call_indirect(env=closure, payload=elem,
                // slot=closure[4]) — a single argument passes the value itself as
                // the payload, exactly as `payload_box`'s one-arg case does.
                nargs(2)?;
                let fp = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?; // closure box for f
                fx.op(I::LocalSet(fp));
                let lp = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?; // TAG_LIST box
                fx.op(I::LocalSet(lp));
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(lp));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n));
                // out = alloc(8 + 4*n); [TAG_LIST, n, _ …]
                let out = fx.local(ValType::I32);
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(out));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(n));
                fx.op(I::I32Store(ma(4, 2)));
                // for i in 0..n: out[8+4i] = apply(f, lp[8+4i])
                let i = fx.local(ValType::I32);
                let apply_ty = self.ty_idx(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(n));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));
                // dst = out + 8 + 4*i
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                // env = f
                fx.op(I::LocalGet(fp));
                // payload = lp[8+4*i]
                fx.op(I::LocalGet(lp));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));
                // slot = f[4]
                fx.op(I::LocalGet(fp));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::CallIndirect {
                    type_index: apply_ty,
                    table_index: 0,
                });
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(i));
                fx.op(I::Br(0));
                fx.op(I::End); // loop
                fx.op(I::End); // block
                fx.op(I::LocalGet(out));
            }
            "fold" => {
                // fold(f, acc, list) -> acc: left fold. Each step applies `f`
                // to the two-element tuple (acc, elem) — the bundle shape the
                // interpreter uses (`apply(f, Tup([acc, elem]))`) — via the
                // boxed-closure convention; the closure wrapper unpacks the
                // TAG_TUP payload into the function's two params.
                nargs(3)?;
                let fp = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?; // closure box for f
                fx.op(I::LocalSet(fp));
                let acc = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?; // initial accumulator box
                fx.op(I::LocalSet(acc));
                let lp = fx.local(ValType::I32);
                self.expr(fx, items[2], false)?; // TAG_LIST box
                fx.op(I::LocalSet(lp));
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(lp));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n));
                let i = fx.local(ValType::I32);
                let pay = fx.local(ValType::I32);
                let apply_ty = self.ty_idx(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(n));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));
                // pay = [TAG_TUP, 2, acc, lp[8+4*i]]
                fx.op(I::I32Const(16));
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(pay));
                fx.op(I::LocalGet(pay));
                fx.op(I::I32Const(TAG_TUP));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(pay));
                fx.op(I::I32Const(2));
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::LocalGet(pay));
                fx.op(I::LocalGet(acc));
                fx.op(I::I32Store(ma(8, 2)));
                fx.op(I::LocalGet(pay));
                fx.op(I::LocalGet(lp));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Store(ma(12, 2)));
                // acc = apply(f, pay)
                fx.op(I::LocalGet(fp));
                fx.op(I::LocalGet(pay));
                fx.op(I::LocalGet(fp));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::CallIndirect {
                    type_index: apply_ty,
                    table_index: 0,
                });
                fx.op(I::LocalSet(acc));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(i));
                fx.op(I::Br(0));
                fx.op(I::End); // loop
                fx.op(I::End); // block
                fx.op(I::LocalGet(acc));
            }
            "filter" => {
                // filter(f, list) -> list: keep the elements for which the
                // predicate `f` returns true, in order (oracle: a non-bool
                // result is an error). The result is at most as long as the
                // input, so a full-size TAG_LIST is over-allocated, the kept
                // elements packed in, and the length set to the kept count.
                nargs(2)?;
                let fp = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?; // closure box
                fx.op(I::LocalSet(fp));
                let lp = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?; // TAG_LIST box
                fx.op(I::LocalSet(lp));
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(lp));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n));
                // out = alloc(8 + 4*n); [TAG_LIST, <k set at end>, …]
                let out = fx.local(ValType::I32);
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(out));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Store(ma(0, 2)));
                let i = fx.local(ValType::I32);
                let k = fx.local(ValType::I32);
                let elem = fx.local(ValType::I32);
                let r = fx.local(ValType::I32);
                let apply_ty = self.ty_idx(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(k));
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(n));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));
                // elem = lp[8+4*i]
                fx.op(I::LocalGet(lp));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::LocalSet(elem));
                // r = apply(f, elem)
                fx.op(I::LocalGet(fp));
                fx.op(I::LocalGet(elem));
                fx.op(I::LocalGet(fp));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::CallIndirect {
                    type_index: apply_ty,
                    table_index: 0,
                });
                fx.op(I::LocalSet(r));
                // predicate must be a bool box, else trap (oracle: error)
                fx.op(I::LocalGet(r));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_BOOL));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                // if r's value != 0: out[8+4*k] = elem; k += 1
                fx.op(I::LocalGet(r));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::If(BlockType::Empty));
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(k));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(elem));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(k));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(k));
                fx.op(I::End);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(i));
                fx.op(I::Br(0));
                fx.op(I::End); // loop
                fx.op(I::End); // block
                // out[4] = k
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(k));
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::LocalGet(out));
            }
            "zip" => {
                // Pair two lists element-wise into a list of 2-tuples, stopping at
                // the shorter (oracle: x.into_iter().zip(b)). Non-list traps.
                nargs(2)?;
                let a = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(a));
                let b = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::LocalSet(b));
                fx.op(I::LocalGet(a));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                let n1 = fx.local(ValType::I32);
                fx.op(I::LocalGet(a));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n1));
                let n2 = fx.local(ValType::I32);
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n2));
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(n1));
                fx.op(I::LocalGet(n2));
                fx.op(I::I32LtU);
                fx.op(I::If(BlockType::Result(ValType::I32)));
                fx.op(I::LocalGet(n1));
                fx.op(I::Else);
                fx.op(I::LocalGet(n2));
                fx.op(I::End);
                fx.op(I::LocalSet(n));
                let out = fx.local(ValType::I32);
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(out));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(n));
                fx.op(I::I32Store(ma(4, 2)));
                let i = fx.local(ValType::I32);
                let tup = fx.local(ValType::I32);
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(n));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));
                // tup = [TAG_TUP, 2, a[8+4i], b[8+4i]]
                fx.op(I::I32Const(16));
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(tup));
                fx.op(I::LocalGet(tup));
                fx.op(I::I32Const(TAG_TUP));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(tup));
                fx.op(I::I32Const(2));
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::LocalGet(tup));
                fx.op(I::LocalGet(a));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Store(ma(8, 2)));
                fx.op(I::LocalGet(tup));
                fx.op(I::LocalGet(b));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Store(ma(12, 2)));
                // out[8+4i] = tup
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::LocalGet(tup));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(i));
                fx.op(I::Br(0));
                fx.op(I::End);
                fx.op(I::End);
                fx.op(I::LocalGet(out));
            }
            "apply" => {
                // apply(f, payload): call the closure value `f` with `payload` as
                // its argument bundle, via the boxed-closure convention
                // (call_indirect(env=f, payload, slot=f[4])) — the same seam
                // `map`/`fold`/`filter` use. The interpreter's `apply` binds the
                // payload per the function's arity (a single value to a 1-param
                // fn, a TAG_TUP bundle to an n-param fn), which is exactly what
                // the closure wrapper unpacks.
                nargs(2)?;
                let fp = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(fp));
                let pay = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::LocalSet(pay));
                let apply_ty = self.ty_idx(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
                fx.op(I::LocalGet(fp));
                fx.op(I::LocalGet(pay));
                fx.op(I::LocalGet(fp));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::CallIndirect {
                    type_index: apply_ty,
                    table_index: 0,
                });
            }
            _ => unreachable!("builtin_higher_order routed unexpected name `{name}`"),
        }
        Ok(())
    }

    /// Element access and growth: `get`/`put` (indexed read/replace),
    /// `push`, and `concat`.
    fn builtin_index(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` expects {want} argument(s), got {}",
                    items.len()
                ))
            }
        };
        match name {
            "get" => {
                // Index a list OR tuple; out-of-range (idx >= len, unsigned so a
                // negative index also) traps, like the oracle's range error. A
                // record or other tag traps (the oracle errors on those too).
                nargs(2)?;
                let lst = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(lst));
                let idx = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::I32WrapI64);
                fx.op(I::LocalSet(idx));
                let tg = fx.local(ValType::I32);
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::LocalTee(tg));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Eq);
                fx.op(I::LocalGet(tg));
                fx.op(I::I32Const(TAG_TUP));
                fx.op(I::I32Eq);
                fx.op(I::I32Or);
                fx.op(I::I32Eqz);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(idx));
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::I32GeU);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(idx));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));
            }
            "put" => {
                // Copy the list and overwrite element `idx` with `v`, returning
                // the new list; idx >= len traps (oracle range error). `put`
                // requires a list (not a tuple), matching the oracle's want_list.
                nargs(3)?;
                let lst = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(lst));
                let idx = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::I32WrapI64);
                fx.op(I::LocalSet(idx));
                let v = fx.local(ValType::I32);
                self.expr(fx, items[2], false)?;
                fx.op(I::LocalSet(v));
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n));
                fx.op(I::LocalGet(idx));
                fx.op(I::LocalGet(n));
                fx.op(I::I32GeU);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                let sz = fx.local(ValType::I32);
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(sz));
                let out = fx.local(ValType::I32);
                fx.op(I::LocalGet(sz));
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(out));
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(lst));
                fx.op(I::LocalGet(sz));
                fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(idx));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::LocalGet(v));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(out));
            }
            "push" => {
                // Copy the list and append `v` at the end, returning the new list.
                nargs(2)?;
                let lst = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(lst));
                let v = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::LocalSet(v));
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(lst));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n));
                let out = fx.local(ValType::I32);
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(out));
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(lst));
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(n));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::LocalGet(v));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(out));
            }
            "concat" => {
                // Concatenate two lists into a fresh list (oracle: want_list ++
                // want_list). A non-list operand traps.
                nargs(2)?;
                let a = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(a));
                let b = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::LocalSet(b));
                fx.op(I::LocalGet(a));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                let n1 = fx.local(ValType::I32);
                fx.op(I::LocalGet(a));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n1));
                let n2 = fx.local(ValType::I32);
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n2));
                let out = fx.local(ValType::I32);
                fx.op(I::LocalGet(n1));
                fx.op(I::LocalGet(n2));
                fx.op(I::I32Add);
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(out));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(n1));
                fx.op(I::LocalGet(n2));
                fx.op(I::I32Add);
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(a));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(n1));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
                fx.op(I::LocalGet(out));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(n1));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::LocalGet(b));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(n2));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
                fx.op(I::LocalGet(out));
            }
            _ => unreachable!("builtin_index routed unexpected name `{name}`"),
        }
        Ok(())
    }

    /// Search and partition over strings/lists: `contains`, `join`, and
    /// `split` (which delegates to `builtin_split`).
    fn builtin_search(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` expects {want} argument(s), got {}",
                    items.len()
                ))
            }
        };
        match name {
            "join" => {
                // Concatenate the list's string elements with `sep` between them
                // (oracle: want_list of parts, want_str each, want_str sep).
                // `strcat2` tag-checks its operands, so a non-string part/sep
                // traps exactly where the oracle errors; the empty-list result
                // is "" but `sep` is still validated (oracle pops it first).
                nargs(2)?;
                let parts = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(parts));
                let sep = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::LocalSet(sep));
                fx.op(I::LocalGet(parts));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(sep));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_STR));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(parts));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(n));
                let acc = fx.local(ValType::I32);
                fx.op(I::I32Const(self.intern_str("") as i32));
                fx.op(I::LocalSet(acc));
                let i = fx.local(ValType::I32);
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(n));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));
                // acc = strcat2(acc, sep) when i > 0
                fx.op(I::LocalGet(i));
                fx.op(I::If(BlockType::Empty));
                fx.op(I::LocalGet(acc));
                fx.op(I::LocalGet(sep));
                fx.op(I::Call(self.h.strcat2));
                fx.op(I::LocalSet(acc));
                fx.op(I::End);
                // acc = strcat2(acc, parts[8+4i])
                fx.op(I::LocalGet(acc));
                fx.op(I::LocalGet(parts));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::Call(self.h.strcat2));
                fx.op(I::LocalSet(acc));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(i));
                fx.op(I::Br(0));
                fx.op(I::End);
                fx.op(I::End);
                fx.op(I::LocalGet(acc));
            }
            "contains" => {
                // Substring test over UTF-8 bytes (oracle: s.contains(sub), both
                // strings). The empty substring is contained in every string,
                // matching Rust. Non-string operands trap (want_str).
                nargs(2)?;
                let s = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(s));
                let sub = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::LocalSet(sub));
                for str_box in [s, sub] {
                    fx.op(I::LocalGet(str_box));
                    fx.op(I::I32Load(ma(0, 2)));
                    fx.op(I::I32Const(TAG_STR));
                    fx.op(I::I32Ne);
                    fx.op(I::If(BlockType::Empty));
                    fx.op(I::Unreachable);
                    fx.op(I::End);
                }
                let slen = fx.local(ValType::I32);
                fx.op(I::LocalGet(s));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(slen));
                let sublen = fx.local(ValType::I32);
                fx.op(I::LocalGet(sub));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::LocalSet(sublen));
                let found = fx.local(ValType::I32);
                let matched = fx.local(ValType::I32);
                let i = fx.local(ValType::I32);
                let k = fx.local(ValType::I32);
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(found));
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::Block(BlockType::Empty)); // outer
                fx.op(I::Loop(BlockType::Empty)); // iloop
                fx.op(I::LocalGet(found));
                fx.op(I::BrIf(1));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(sublen));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(slen));
                fx.op(I::I32GtU);
                fx.op(I::BrIf(1));
                fx.op(I::I32Const(1));
                fx.op(I::LocalSet(matched));
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(k));
                fx.op(I::Block(BlockType::Empty)); // jbreak
                fx.op(I::Loop(BlockType::Empty)); // jloop
                fx.op(I::LocalGet(k));
                fx.op(I::LocalGet(sublen));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));
                fx.op(I::LocalGet(s));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(k));
                fx.op(I::I32Add);
                fx.op(I::I32Load8U(ma(0, 0)));
                fx.op(I::LocalGet(sub));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(k));
                fx.op(I::I32Add);
                fx.op(I::I32Load8U(ma(0, 0)));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(matched));
                fx.op(I::Br(2));
                fx.op(I::End);
                fx.op(I::LocalGet(k));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(k));
                fx.op(I::Br(0));
                fx.op(I::End); // jloop
                fx.op(I::End); // jbreak
                fx.op(I::LocalGet(matched));
                fx.op(I::If(BlockType::Empty));
                fx.op(I::I32Const(1));
                fx.op(I::LocalSet(found));
                fx.op(I::End);
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(1));
                fx.op(I::I32Add);
                fx.op(I::LocalSet(i));
                fx.op(I::Br(0));
                fx.op(I::End); // iloop
                fx.op(I::End); // outer
                fx.op(I::LocalGet(found));
                fx.op(I::Call(self.h.box_bool));
            }
            "split" => return self.builtin_split(fx, items),
            _ => unreachable!("builtin_search routed unexpected name `{name}`"),
        }
        Ok(())
    }

    /// `s.split(sep)` — factored out of `builtin_search` because its byte
    /// scanner is by far the largest single builtin body.
    fn builtin_split(&mut self, fx: &mut FnCtx, items: &[NodeId]) -> Result<(), String> {
        // s.split(sep) over UTF-8 bytes (oracle: want_str s, want_str
        // sep). Non-empty sep: scan left-to-right for non-overlapping
        // byte matches, emitting the run before each match and the final
        // tail (at most slen+1 pieces, so the list is over-allocated and
        // its length fixed at the end). Empty sep replicates Rust's
        // char-boundary split: a leading "", one piece per char, a
        // trailing "".
        if items.len() != 2 {
            return Err(format!("`split` expects 2 argument(s), got {}", items.len()));
        }
        let s = fx.local(ValType::I32);
        self.expr(fx, items[0], false)?;
        fx.op(I::LocalSet(s));
        let sep = fx.local(ValType::I32);
        self.expr(fx, items[1], false)?;
        fx.op(I::LocalSet(sep));
        for str_box in [s, sep] {
            fx.op(I::LocalGet(str_box));
            fx.op(I::I32Load(ma(0, 2)));
            fx.op(I::I32Const(TAG_STR));
            fx.op(I::I32Ne);
            fx.op(I::If(BlockType::Empty));
            fx.op(I::Unreachable);
            fx.op(I::End);
        }
        let slen = fx.local(ValType::I32);
        fx.op(I::LocalGet(s));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(slen));
        let seplen = fx.local(ValType::I32);
        fx.op(I::LocalGet(sep));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(seplen));
        let out = fx.local(ValType::I32);
        let oi = fx.local(ValType::I32);
        let start = fx.local(ValType::I32);
        let i = fx.local(ValType::I32);
        let k = fx.local(ValType::I32);
        let matched = fx.local(ValType::I32);
        let plen = fx.local(ValType::I32);
        let pbox = fx.local(ValType::I32);
        let j = fx.local(ValType::I32);
        let empty = self.intern_str("") as i32;
        fx.op(I::LocalGet(seplen));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        // ---- empty-sep path (Rust char-boundary split) ----
        // out over-allocated to slen+2 pieces; length fixed at the end.
        fx.op(I::LocalGet(slen));
        fx.op(I::I32Const(2));
        fx.op(I::I32Add);
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(out));
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Store(ma(0, 2)));
        // out[8] = ""  (leading empty piece)
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(empty));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::I32Const(1));
        fx.op(I::LocalSet(oi));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i)); // byte cursor
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(slen));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalSet(start)); // char start
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i)); // consume lead byte
        // advance over continuation bytes (b & 0xC0 == 0x80)
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(slen));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(s));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(0, 0)));
        fx.op(I::I32Const(0xC0));
        fx.op(I::I32And);
        fx.op(I::I32Const(0x80));
        fx.op(I::I32Ne);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        // plen = i - start ; piece = substr(s, start, plen)
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(start));
        fx.op(I::I32Sub);
        fx.op(I::LocalSet(plen));
        emit_substr(self, fx, s, start, plen, pbox, j);
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(pbox));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(oi));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        // trailing empty piece
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Const(empty));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(oi));
        fx.op(I::Else);
        // ---- non-empty-sep path ----
        fx.op(I::LocalGet(slen));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalSet(out));
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(oi));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(start));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(seplen));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(slen));
        fx.op(I::I32GtU);
        fx.op(I::BrIf(1));
        fx.op(I::I32Const(1));
        fx.op(I::LocalSet(matched));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(k));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(k));
        fx.op(I::LocalGet(seplen));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(s));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(k));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(0, 0)));
        fx.op(I::LocalGet(sep));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(k));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(0, 0)));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(matched));
        fx.op(I::Br(2));
        fx.op(I::End);
        fx.op(I::LocalGet(k));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(k));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(matched));
        fx.op(I::If(BlockType::Empty));
        // emit piece s[start..i]
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(start));
        fx.op(I::I32Sub);
        fx.op(I::LocalSet(plen));
        emit_substr(self, fx, s, start, plen, pbox, j);
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(pbox));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(oi));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(seplen));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalSet(start));
        fx.op(I::Else);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::End);
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        // final tail piece s[start..slen]
        fx.op(I::LocalGet(slen));
        fx.op(I::LocalGet(start));
        fx.op(I::I32Sub);
        fx.op(I::LocalSet(plen));
        emit_substr(self, fx, s, start, plen, pbox, j);
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(pbox));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(oi));
        fx.op(I::End);
        // fix the list length to the actual piece count
        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(out));
        Ok(())
    }

    /// String builtins: `str-cat`, `to-string`, `to-char`, and
    /// `upper`/`lower`.
    fn builtin_string(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` expects {want} argument(s), got {}",
                    items.len()
                ))
            }
        };
        match name {
            "str-cat" => {
                if items.is_empty() {
                    let a = self.intern_str("");
                    fx.op(I::I32Const(a as i32));
                    return Ok(());
                }
                self.expr(fx, items[0], false)?;
                for &x in &items[1..] {
                    self.expr(fx, x, false)?;
                    fx.op(I::Call(self.h.strcat2));
                }
            }
            "to-string" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.to_str));
            }
            "to-char" => {
                // A char passes through; an int must be a Unicode scalar value
                // (traps otherwise, like the interpreter's range error). Anything
                // else traps in unbox_int, matching `to-char expects an int`.
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                let b = fx.local(ValType::I32);
                fx.op(I::LocalSet(b));
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_CHAR));
                fx.op(I::I32Eq);
                fx.op(I::If(BlockType::Result(ValType::I32)));
                fx.op(I::LocalGet(b));
                fx.op(I::Else);
                let n = fx.local(ValType::I64);
                fx.op(I::LocalGet(b));
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::LocalSet(n));
                // invalid if n > 0x10FFFF (unsigned, catches negatives) or a
                // surrogate (0xD800..=0xDFFF)
                fx.op(I::LocalGet(n));
                fx.op(I::I64Const(0x10FFFF));
                fx.op(I::I64GtU);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(n));
                fx.op(I::I64Const(0xD800));
                fx.op(I::I64GeU);
                fx.op(I::LocalGet(n));
                fx.op(I::I64Const(0xDFFF));
                fx.op(I::I64LeU);
                fx.op(I::I32And);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);
                fx.op(I::LocalGet(n));
                self.box_char(fx);
                fx.op(I::End);
            }
            "upper" | "lower" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::I32Const(if name == "upper" { 1 } else { 0 }));
                fx.op(I::Call(self.h.case_h));
            }
            _ => unreachable!("builtin_string routed unexpected name `{name}`"),
        }
        Ok(())
    }

    /// Variant constructors: `some`, `ok`, and `err`.
    fn builtin_variant(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        match name {
            "some" | "ok" | "err" => {
                // the argument(s) bundle into the variant payload, exactly as
                // the interpreter binds it. `ok()`/`err()` with no arguments
                // construct the payload-less case (4.2), like the interpreter.
                if items.is_empty() && name != "some" {
                    let addr = self.none_like_box(name);
                    fx.op(I::I32Const(addr as i32));
                    return Ok(());
                }
                self.var_box(fx, name, items)
            }
            _ => unreachable!("builtin_variant routed unexpected name `{name}`"),
        }
    }

    /// Mutable cell builtins: `cell-new`, `cell-get`, and `cell-set`.
    fn builtin_cell(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` expects {want} argument(s), got {}",
                    items.len()
                ))
            }
        };
        match name {
            "cell-new" => {
                // A mutable cell holding one boxed value; its heap pointer is
                // its identity (interp: `Value::Cell(Rc<RefCell<Value>>)`).
                // In a resource/functor component a cell is resource state (a
                // `DefResource` `New`), so the cell AND its value live in the
                // persistent region and survive the per-call arena reset (5.1).
                nargs(1)?;
                let persist = self.has_persist();
                let v = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                if persist {
                    fx.op(I::Call(self.h.persist));
                }
                fx.op(I::LocalSet(v));
                let p = fx.local(ValType::I32);
                fx.op(I::I32Const(8));
                fx.op(I::Call(if persist {
                    self.h.persist_alloc
                } else {
                    self.h.alloc
                }));
                fx.op(I::LocalSet(p));
                fx.op(I::LocalGet(p));
                fx.op(I::I32Const(TAG_CELL));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(p));
                fx.op(I::LocalGet(v));
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::LocalGet(p));
            }
            "cell-get" => {
                // Current value box, at cell offset 4.
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::I32Load(ma(4, 2)));
            }
            "cell-set" => {
                // Overwrite the cell's value box; return unit (interp parity).
                // In a resource/functor component the stored value is routed
                // through the persistent write barrier so it outlives the reset
                // (5.1); the cell itself was already allocated persistently.
                nargs(2)?;
                let persist = self.has_persist();
                let c = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(c));
                fx.op(I::LocalGet(c));
                self.expr(fx, items[1], false)?;
                if persist {
                    fx.op(I::Call(self.h.persist));
                }
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::I32Const(self.unit_addr() as i32));
            }
            _ => unreachable!("builtin_cell routed unexpected name `{name}`"),
        }
        Ok(())
    }

    /// Compile-time form machinery used by macro bodies: `form-kind`,
    /// `rec-key`, `rec-val`, `gensym`, and `expand`.
    fn builtin_form(&mut self, fx: &mut FnCtx, name: &str, items: &[NodeId]) -> Result<(), String> {
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` expects {want} argument(s), got {}",
                    items.len()
                ))
            }
        };
        match name {
            "form-kind" => {
                nargs(1)?;
                return self.form_kind(fx, items[0]);
            }
            "rec-key" => {
                // First field's key as a payload-less variant: build
                // `[TAG_VAR, key-str-box, 0]` over the key box at rec offset 8.
                nargs(1)?;
                let rp = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(rp));
                self.rec_guard(fx, rp);
                let p = fx.local(ValType::I32);
                fx.op(I::I32Const(12));
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(p));
                fx.op(I::LocalGet(p));
                fx.op(I::I32Const(TAG_VAR));
                fx.op(I::I32Store(ma(0, 2)));
                fx.op(I::LocalGet(p));
                fx.op(I::LocalGet(rp));
                fx.op(I::I32Load(ma(8, 2)));
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::LocalGet(p));
                fx.op(I::I32Const(0));
                fx.op(I::I32Store(ma(8, 2)));
                fx.op(I::LocalGet(p));
            }
            "rec-val" => {
                // First field's value box, at rec offset 12.
                nargs(1)?;
                let rp = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(rp));
                self.rec_guard(fx, rp);
                fx.op(I::LocalGet(rp));
                fx.op(I::I32Load(ma(12, 2)));
            }
            "gensym" => {
                nargs(0)?;
                return self.gensym(fx);
            }
            "expand" => {
                nargs(1)?;
                match self.macro_expand_idx {
                    // Inside a macro component: one expansion step over the
                    // library's own macros (mirrors `builtins.rs` `expand`).
                    Some(idx) => {
                        self.expr(fx, items[0], false)?;
                        fx.op(I::Call(idx));
                    }
                    None => {
                        return Err("`expand` is only available inside a macro library \
                             (a file whose top level is DefMacros)"
                            .into());
                    }
                }
            }
            _ => unreachable!("builtin_form routed unexpected name `{name}`"),
        }
        Ok(())
    }
}

const BUILTINS: &[&str] = &[
    "eq",
    "not",
    "lt",
    "le",
    "gt",
    "ge",
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
    "drop",
    "get",
    "put",
    "push",
    "concat",
    "reverse",
    "range",
    "zip",
    "split",
    "join",
    "contains",
    "apply",
    "head",
    "tail",
    "map",
    "fold",
    "filter",
    "str-cat",
    "upper",
    "lower",
    "to-string",
    "to-char",
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
    "some",
    "ok",
    "err",
    "cell-new",
    "cell-get",
    "cell-set",
    // compile-time form machinery (macro bodies)
    "form-kind",
    "rec-key",
    "rec-val",
    "gensym",
    "expand",
];

fn param_names(arena: &Arena, params_id: NodeId) -> Result<Vec<String>, String> {
    match arena.node(params_id) {
        Node::Flg(names) => Ok(names.clone()),
        Node::Rec(fields) => Ok(fields.iter().map(|(k, _)| k.clone()).collect()),
        _ => Err("malformed Fn parameters".into()),
    }
}

