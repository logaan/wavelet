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
    ExportKind, ExportSection, Function, FunctionSection, GlobalSection, GlobalType,
    ImportSection, Instruction as I, MemArg, MemorySection, MemoryType, Module, RefType,
    TableSection, TableType, TypeSection, ValType,
};

use crate::form::{Arena, Node, NodeId};
use crate::wit::{type_decl, FileInfo, FuncSig};

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

fn ma(offset: u64, align: u32) -> MemArg {
    MemArg { offset, align, memory_index: 0 }
}

/// Push a zero of the given flat type (variant payload padding).
fn push_zero(fx: &mut FnCtx, vt: ValType) {
    match vt {
        ValType::I64 => fx.op(I::I64Const(0)),
        ValType::F64 => fx.op(I::F64Const(0.0.into())),
        _ => fx.op(I::I32Const(0)),
    }
}

// ---------------------------------------------------------------- WIT types

#[derive(Clone, PartialEq)]
enum WitTy {
    Bool,
    IntS, // s8/s16/s32 — i32 flat, sign-extended into the int box
    IntU, // u8/u16/u32
    S64,  // s64/u64 — i64 flat
    F64,
    Str,
    List(Box<WitTy>),
    Record(Vec<(String, WitTy)>), // named record type, fully expanded
    Option(Box<WitTy>),
    Result(Box<WitTy>, Box<WitTy>),
    /// A resource handle (`own<T>`/`borrow<T>` or a bare wasi resource name).
    /// Opaque to Wavelet: a single i32 handle from the host, carried in an int
    /// box so ordinary code can pass it around without inspecting it.
    Handle,
}

impl WitTy {
    /// option/result viewed as a 2-case discriminated variant: the canonical
    /// case order (none=0/some=1, ok=0/err=1) with each case's payload type.
    fn variant_cases(&self) -> Option<Vec<(&'static str, Option<&WitTy>)>> {
        match self {
            WitTy::Option(t) => Some(vec![("none", None), ("some", Some(t))]),
            WitTy::Result(t, e) => Some(vec![("ok", Some(t)), ("err", Some(e))]),
            _ => None,
        }
    }
}

/// Split the comma-separated args of `ctor<...>`, respecting nested `<>`.
fn split_type_args(inner: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in inner.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                out.push(inner[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = inner[start..].trim();
    if !last.is_empty() {
        out.push(last.to_string());
    }
    out
}

/// Named record types, by name → field (name, type-string), for resolving
/// record references at component boundaries.
type TypeEnv = HashMap<String, Vec<(String, String)>>;

/// The wasi resource types Wavelet understands as opaque handles. Their methods
/// are reached through the `http/*` intrinsics; the values themselves are never
/// inspected, only passed along, so a single membership test is all we need.
fn is_resource_name(s: &str) -> bool {
    matches!(
        s,
        "incoming-request"
            | "outgoing-request"
            | "incoming-response"
            | "outgoing-response"
            | "request-options"
            | "response-outparam"
            | "fields"
            | "headers"
            | "trailers"
            | "incoming-body"
            | "outgoing-body"
            | "future-trailers"
            | "future-incoming-response"
            | "input-stream"
            | "output-stream"
            | "pollable"
            | "io-error"
    )
}

fn wit_ty(s: &str, env: &TypeEnv) -> Result<WitTy, String> {
    // A resource handle: `own<T>` / `borrow<T>`, or a bare wasi resource name.
    if s.starts_with("own<") || s.starts_with("borrow<") || is_resource_name(s) {
        return Ok(WitTy::Handle);
    }
    if let Some(inner) = s.strip_prefix("list<").and_then(|r| r.strip_suffix('>')) {
        return Ok(WitTy::List(Box::new(wit_ty(inner.trim(), env)?)));
    }
    if let Some(inner) = s.strip_prefix("option<").and_then(|r| r.strip_suffix('>')) {
        return Ok(WitTy::Option(Box::new(wit_ty(inner.trim(), env)?)));
    }
    if let Some(inner) = s.strip_prefix("result<").and_then(|r| r.strip_suffix('>')) {
        let args = split_type_args(inner);
        if args.len() != 2 {
            return Err(format!(
                "`{s}`: only `result<T, E>` (both arms typed) is supported by the wasm backend"
            ));
        }
        return Ok(WitTy::Result(
            Box::new(wit_ty(&args[0], env)?),
            Box::new(wit_ty(&args[1], env)?),
        ));
    }
    Ok(match s {
        "bool" => WitTy::Bool,
        "s8" | "s16" | "s32" => WitTy::IntS,
        "u8" | "u16" | "u32" => WitTy::IntU,
        "s64" | "u64" => WitTy::S64,
        "f64" => WitTy::F64,
        "string" => WitTy::Str,
        other => match env.get(other) {
            Some(fields) => {
                let mut resolved = Vec::with_capacity(fields.len());
                for (fname, fty) in fields {
                    resolved.push((fname.clone(), wit_ty(fty, env)?));
                }
                WitTy::Record(resolved)
            }
            None => return Err(format!("type `{other}` not supported by the wasm backend yet")),
        },
    })
}

/// Join two flat representations position-wise (canonical-ABI variant flatten).
/// We only support cases that don't need numeric widening: the shorter list
/// must be a prefix of the longer, and shared positions must match exactly.
fn join_flat(a: &[ValType], b: &[ValType]) -> Result<Vec<ValType>, String> {
    let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    for (i, t) in short.iter().enumerate() {
        if long[i] != *t {
            return Err("option/result arms with differing flat shapes are not \
                        supported by the wasm backend yet (use a record, or keep \
                        the arms' types flat-compatible)"
                .into());
        }
    }
    Ok(long.to_vec())
}

fn flat(ty: &WitTy) -> Vec<ValType> {
    flat_checked(ty).expect("flat() on an unsupported boundary type")
}

/// Number of flat (core) values a type lowers to. Unlike [`flat_checked`] this
/// never needs the variant-join to succeed — it just counts — so it is safe to
/// use when only the count matters (deciding direct return vs retptr).
fn flat_len(ty: &WitTy) -> usize {
    match ty {
        WitTy::Bool | WitTy::IntS | WitTy::IntU | WitTy::S64 | WitTy::F64 | WitTy::Handle => 1,
        WitTy::Str | WitTy::List(_) => 2,
        WitTy::Record(fields) => fields.iter().map(|(_, t)| flat_len(t)).sum(),
        WitTy::Option(_) | WitTy::Result(..) => {
            let payload = ty
                .variant_cases()
                .unwrap()
                .iter()
                .filter_map(|(_, p)| p.map(flat_len))
                .max()
                .unwrap_or(0);
            1 + payload
        }
    }
}

fn flat_checked(ty: &WitTy) -> Result<Vec<ValType>, String> {
    Ok(match ty {
        WitTy::Bool | WitTy::IntS | WitTy::IntU | WitTy::Handle => vec![ValType::I32],
        WitTy::S64 => vec![ValType::I64],
        WitTy::F64 => vec![ValType::F64],
        WitTy::Str | WitTy::List(_) => vec![ValType::I32, ValType::I32],
        WitTy::Record(fields) => {
            let mut v = Vec::new();
            for (_, t) in fields {
                v.extend(flat_checked(t)?);
            }
            v
        }
        WitTy::Option(_) | WitTy::Result(..) => {
            let cases = ty.variant_cases().unwrap();
            let mut joined: Vec<ValType> = Vec::new();
            for (_, pay) in &cases {
                let f = match pay {
                    Some(t) => flat_checked(t)?,
                    None => vec![],
                };
                joined = join_flat(&joined, &f)?;
            }
            let mut v = vec![ValType::I32]; // discriminant
            v.extend(joined);
            v
        }
    })
}

/// Canonical-ABI alignment (bytes) for a type's in-memory representation.
fn align_of(ty: &WitTy) -> u64 {
    match ty {
        WitTy::Bool => 1,
        WitTy::Handle => 4,
        WitTy::IntS | WitTy::IntU => 4, // s8/s16 widen to 4 here (we only box i32)
        WitTy::S64 | WitTy::F64 => 8,
        WitTy::Str | WitTy::List(_) => 4, // (ptr, len), pointer-aligned
        WitTy::Record(fields) => fields.iter().map(|(_, t)| align_of(t)).max().unwrap_or(1),
        WitTy::Option(_) | WitTy::Result(..) => {
            // disc is 1-byte; align is the max of disc and any payload align
            ty.variant_cases()
                .unwrap()
                .iter()
                .filter_map(|(_, p)| p.map(align_of))
                .max()
                .unwrap_or(1)
                .max(1)
        }
    }
}

/// Offset of a variant's payload (after the 1-byte discriminant, padded to the
/// payload alignment).
fn variant_payload_offset(ty: &WitTy) -> u64 {
    align_up(1, align_of(ty))
}

/// Canonical-ABI size (bytes) in memory.
fn size_of(ty: &WitTy) -> u64 {
    match ty {
        WitTy::Bool => 1,
        WitTy::Handle => 4,
        WitTy::IntS | WitTy::IntU => 4,
        WitTy::S64 | WitTy::F64 => 8,
        WitTy::Str | WitTy::List(_) => 8,
        WitTy::Record(_) => {
            let a = align_of(ty);
            let mut off = 0u64;
            for (o, t) in record_field_offsets(ty) {
                off = o + size_of(&t);
            }
            align_up(off, a)
        }
        WitTy::Option(_) | WitTy::Result(..) => {
            let payload = ty
                .variant_cases()
                .unwrap()
                .iter()
                .filter_map(|(_, p)| p.map(size_of))
                .max()
                .unwrap_or(0);
            align_up(variant_payload_offset(ty) + payload, align_of(ty))
        }
    }
}

fn align_up(off: u64, align: u64) -> u64 {
    (off + align - 1) / align * align
}

/// (offset, field-type) for each field of a record, in declaration order.
fn record_field_offsets(ty: &WitTy) -> Vec<(u64, WitTy)> {
    let WitTy::Record(fields) = ty else { return vec![] };
    let mut off = 0u64;
    let mut out = Vec::with_capacity(fields.len());
    for (_, ft) in fields {
        off = align_up(off, align_of(ft));
        out.push((off, ft.clone()));
        off += size_of(ft);
    }
    out
}

/// canonical-ABI element size for list payloads
fn elem_size(ty: &WitTy) -> u64 {
    match ty {
        WitTy::Bool => 1,
        WitTy::IntS | WitTy::IntU | WitTy::Handle => 4,
        WitTy::S64 | WitTy::F64 | WitTy::Str | WitTy::List(_) => 8,
        WitTy::Record(_) | WitTy::Option(_) | WitTy::Result(..) => size_of(ty),
    }
}

enum FlatRes {
    None,
    One(WitTy),
    Retptr, // flattened result > 1 value (string/list/record): pass/return a pointer
}

fn flat_result(sig: &FuncSig, env: &TypeEnv) -> Result<FlatRes, String> {
    match &sig.result {
        None => Ok(FlatRes::None),
        Some(t) => {
            let ty = wit_ty(t, env)?;
            // count flats (always defined); retptr never needs the variant-join
            if flat_len(&ty) > 1 { Ok(FlatRes::Retptr) } else { Ok(FlatRes::One(ty)) }
        }
    }
}

// ------------------------------------------------------------ feature scan

#[derive(Default)]
struct Features {
    needs_stdout: bool,
    needs_env: bool,
    /// unique (alias, func) cross-component calls, in first-use order
    dep_calls: Vec<(String, String)>,
}

fn scan(arena: &Arena, id: NodeId, feats: &mut Features) {
    match arena.node(id) {
        Node::Call(head, payload) => {
            match arena.node(*head) {
                Node::Sym(s) if s == "print" || s == "println" => feats.needs_stdout = true,
                Node::Sym(s) if s == "args" => feats.needs_env = true,
                Node::Qsym(alias, name) => {
                    let key = (alias.clone(), name.clone());
                    if !feats.dep_calls.contains(&key) {
                        feats.dep_calls.push(key);
                    }
                }
                _ => {}
            }
            scan(arena, *head, feats);
            scan(arena, *payload, feats);
        }
        Node::Tup(xs) | Node::Lst(xs) => {
            for &x in xs {
                scan(arena, x, feats);
            }
        }
        Node::Rec(fields) => {
            for (_, v) in fields {
                scan(arena, *v, feats);
            }
        }
        _ => {}
    }
}

// ------------------------------------------------------- function building

struct FnCtx {
    instrs: Vec<I<'static>>,
    n_params: u32,
    extra_locals: Vec<ValType>,
    scopes: Vec<HashMap<String, u32>>,
}

impl FnCtx {
    fn new(n_params: u32) -> Self {
        FnCtx { instrs: Vec::new(), n_params, extra_locals: Vec::new(), scopes: vec![] }
    }
    fn local(&mut self, ty: ValType) -> u32 {
        let idx = self.n_params + self.extra_locals.len() as u32;
        self.extra_locals.push(ty);
        idx
    }
    fn op(&mut self, i: I<'static>) {
        self.instrs.push(i);
    }
    fn lookup(&self, name: &str) -> Option<u32> {
        for scope in self.scopes.iter().rev() {
            if let Some(&i) = scope.get(name) {
                return Some(i);
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

struct Helpers {
    alloc: u32,
    realloc: u32,
    box_int: u32,
    box_bool: u32,
    box_dec: u32,
    box_str: u32,
    truthy: u32,
    unbox_int: u32,
    unbox_dec: u32,
    eq_raw: u32,
    len_raw: u32,
    head_h: u32,
    strcat2: u32,
    case_h: u32,
    to_str: u32,
    rec_get: u32,
    print_str: Option<u32>,
    println_h: Option<u32>,
    get_args: Option<u32>,
}

// ---------------------------------------------------------------- emitter

pub fn emit_component(
    arena: &Arena,
    roots: &[NodeId],
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> Result<Vec<u8>, String> {
    let mut module = emit_core_module(arena, roots, info, deps)?;
    let wit = synthesize_world_wit(arena, info, deps, &features_of(arena, info))?;

    let mut resolve = wit_parser::Resolve::default();
    let pkg = resolve
        .push_str("wavelet-synthesized.wit", &wit)
        .map_err(|e| format!("internal: synthesized WIT did not parse: {e:#}\n--- WIT ---\n{wit}"))?;
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

fn features_of(arena: &Arena, info: &FileInfo) -> Features {
    let mut feats = Features::default();
    for (_, (params, body)) in &info.defs {
        let _ = params;
        scan(arena, *body, &mut feats);
    }
    for (_, expr) in &info.value_defs {
        scan(arena, *expr, &mut feats);
    }
    feats
}

/// Record types from a file's `DefType` forms: name → field (name, type-string).
/// Only record-shaped types are collected (variants/flags/aliases are skipped;
/// the boundary ABI for those is not implemented in the wasm backend yet).
fn record_types(arena: &Arena, types: &[(String, NodeId)]) -> Vec<(String, Vec<(String, String)>)> {
    let mut out = Vec::new();
    for (name, node) in types {
        if let Node::Rec(fields) = arena.node(*node) {
            let mut fs = Vec::with_capacity(fields.len());
            let mut ok = true;
            for (fname, fnode) in fields {
                match crate::wit::type_text(arena, *fnode) {
                    Ok(t) => fs.push((fname.clone(), t)),
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                out.push((name.clone(), fs));
            }
        }
    }
    out
}

/// Public: record types a dependency file defines, for the build driver to put
/// on its [`Dep`].
pub fn dep_record_types(arena: &Arena, info: &FileInfo) -> Vec<(String, Vec<(String, String)>)> {
    record_types(arena, &info.types)
}

/// `"demo:shout/render"` → `"render"`; a bare package path means `api`.
fn import_iface(path: &str) -> String {
    match path.split_once('/') {
        Some((_, iface)) => iface.to_string(),
        None => "api".to_string(),
    }
}

/// `("demo:shout@0.1.0", "api")` → `"demo:shout/api@0.1.0"`
fn versioned_iface(pkg: &str, iface: &str) -> String {
    match pkg.split_once('@') {
        Some((base, ver)) => format!("{base}/{iface}@{ver}"),
        None => format!("{pkg}/{iface}"),
    }
}

struct Emitter<'a> {
    arena: &'a Arena,
    info: &'a FileInfo,
    deps: &'a HashMap<String, Dep>,
    type_env: TypeEnv, // record types in scope (local + deps), for boundary ABI
    data: Vec<u8>, // segment contents, lives at DATA_BASE
    str_cache: HashMap<String, u32>,
    types: Vec<(Vec<ValType>, Vec<ValType>)>,
    imports: Vec<(String, String, u32)>, // module, field, type idx
    import_fn: HashMap<(String, String), u32>,
    h: Helpers,
    funcs: HashMap<String, (u32, Vec<String>)>, // internal defs
    value_globals: HashMap<String, u32>,        // module-level value defs → global idx
    compiling_values: Vec<String>,              // cycle guard for value-def inits
    bodies: Vec<(u32, Function)>,               // (type idx, body) for defined funcs
    /// uniform `(env, payload) -> box` functions reachable through the
    /// funcref table; slot k = function index `imports + bodies + k`
    closure_bodies: Vec<(u32, Function)>,
    fn_wrappers: HashMap<String, u32>, // def name → table slot of its wrapper
    fn_box_cache: HashMap<String, u32>, // def name → static closure box addr
    var_box_cache: HashMap<String, u32>, // payload-less variant case → static box addr
    false_addr: u32,
    true_addr: u32,
    nl_addr: u32,
}

impl<'a> Emitter<'a> {
    /// v0 has no record boxes; the unit value `{}` shares the static false box.
    fn unit_addr(&self) -> u32 {
        self.false_addr
    }

    fn ty_idx(&mut self, params: Vec<ValType>, results: Vec<ValType>) -> u32 {
        if let Some(i) = self.types.iter().position(|t| t.0 == params && t.1 == results) {
            return i as u32;
        }
        self.types.push((params, results));
        (self.types.len() - 1) as u32
    }

    fn align8(&mut self) {
        while (DATA_BASE as usize + self.data.len()) % 8 != 0 {
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
            Node::Char(_) => return Err("char values not supported by the wasm backend yet".into()),
            Node::Sym(name) => match fx.lookup(&name) {
                Some(idx) => fx.op(I::LocalGet(idx)),
                None => return self.value_def_ref(fx, &name),
            },
            Node::Call(head, payload) => return self.call(fx, head, payload, tail),
            Node::Lst(items) => return self.list_box(fx, &items),
            Node::Rec(fields) => return self.rec_box(fx, &fields),
            Node::Tup(items) => return self.seq_box(fx, &items, TAG_TUP),
            Node::Flg(_) => {
                return Err("flag literals not supported by the wasm backend yet".into());
            }
            Node::Qsym(a, n) => {
                return Err(format!("`{a}/{n}` used as a value (only calls are supported)"));
            }
        }
        Ok(())
    }

    /// A name that is no local binding: a module-level value `Def` (lazily
    /// initialized global; 0 = uncomputed, no box lives at 0) or a named
    /// function used as a value (static closure box over a uniform wrapper).
    fn value_def_ref(&mut self, fx: &mut FnCtx, name: &str) -> Result<(), String> {
        if name == "none" {
            let addr = self.none_like_box("none");
            fx.op(I::I32Const(addr as i32));
            return Ok(());
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
            return Err(format!("module-level value `{name}` is defined in terms of itself"));
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
    fn var_box(&mut self, fx: &mut FnCtx, case: &str, payload: NodeId) -> Result<(), String> {
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
        self.expr(fx, payload, false)?;
        fx.op(I::I32Store(ma(8, 2)));
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
        let (fidx, params) = self.funcs[name].clone();
        let mut fx = FnCtx::new(2);
        match params.len() {
            0 => {}
            1 => fx.op(I::LocalGet(1)),
            n => {
                for i in 0..n {
                    fx.op(I::LocalGet(1));
                    fx.op(I::I32Load(ma(8 + 4 * i as u64, 2)));
                }
            }
        }
        fx.op(I::ReturnCall(fidx));
        let t = self.ty_idx(vec![ValType::I32; 2], vec![ValType::I32]);
        self.closure_bodies.push((t, fx.finish()));
        let slot = (self.closure_bodies.len() - 1) as u32;
        self.fn_wrappers.insert(name.to_string(), slot);
        Ok(slot)
    }

    /// `Fn {params} body` as an expression: compile the body to a uniform
    /// `(env, payload) -> box` table function capturing every visible local,
    /// and allocate a closure box `[TAG_FN, slot, k, captures…]` at the site.
    fn fn_form(&mut self, fx: &mut FnCtx, payload: NodeId) -> Result<(), String> {
        let Node::Tup(items) = self.arena.node(payload).clone() else {
            return Err("malformed Fn".into());
        };
        let params = param_names(self.arena, items[0])?;
        let body = items[1];

        // captures: every visible local by name (later scopes shadow earlier),
        // sorted so the layout is deterministic
        let mut cap_map: HashMap<String, u32> = HashMap::new();
        for scope in &fx.scopes {
            for (k, &v) in scope {
                cap_map.insert(k.clone(), v);
            }
        }
        let mut caps: Vec<(String, u32)> = cap_map.into_iter().collect();
        caps.sort();

        let mut cf = FnCtx::new(2);
        let mut scope = HashMap::new();
        for (j, (cname, _)) in caps.iter().enumerate() {
            let l = cf.local(ValType::I32);
            cf.op(I::LocalGet(0));
            cf.op(I::I32Load(ma(12 + 4 * j as u64, 2)));
            cf.op(I::LocalSet(l));
            scope.insert(cname.clone(), l);
        }
        match params.len() {
            0 => {}
            1 => {
                scope.insert(params[0].clone(), 1);
            }
            n => {
                // payload must be a list box of exactly n elements
                cf.op(I::LocalGet(1));
                cf.op(I::I32Load(ma(0, 2)));
                cf.op(I::I32Const(TAG_LIST));
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
                    scope.insert(p.clone(), l);
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
        for (j, (_, lidx)) in caps.iter().enumerate() {
            fx.op(I::LocalGet(p));
            fx.op(I::LocalGet(*lidx));
            fx.op(I::I32Store(ma(12 + 4 * j as u64, 2)));
        }
        fx.op(I::LocalGet(p));
        Ok(())
    }

    /// Indirect call through a closure box: `(box, payload-box)` via the
    /// funcref table slot stored in the box at offset 4.
    fn closure_call(
        &mut self,
        fx: &mut FnCtx,
        head: NodeId,
        payload: NodeId,
        tail: bool,
    ) -> Result<(), String> {
        self.expr(fx, head, false)?;
        let c = fx.local(ValType::I32);
        fx.op(I::LocalSet(c));
        fx.op(I::LocalGet(c)); // env argument = the closure box itself
        match self.arena.node(payload).clone() {
            Node::Lst(items) | Node::Tup(items) => self.list_box(fx, &items)?,
            _ => self.expr(fx, payload, false)?,
        }
        fx.op(I::LocalGet(c));
        fx.op(I::I32Load(ma(4, 2))); // table slot
        let t = self.ty_idx(vec![ValType::I32; 2], vec![ValType::I32]);
        fx.op(if tail {
            I::ReturnCallIndirect { type_index: t, table_index: 0 }
        } else {
            I::CallIndirect { type_index: t, table_index: 0 }
        });
        Ok(())
    }

    fn payload_items(&self, payload: NodeId) -> Vec<NodeId> {
        match self.arena.node(payload) {
            Node::Lst(xs) | Node::Tup(xs) => xs.clone(),
            _ => vec![payload],
        }
    }

    fn call(
        &mut self,
        fx: &mut FnCtx,
        head: NodeId,
        payload: NodeId,
        tail: bool,
    ) -> Result<(), String> {
        let head_node = self.arena.node(head).clone();
        match head_node {
            Node::Qsym(alias, fname) => self.dep_call(fx, &alias, &fname, payload),
            Node::Sym(name) => match name.as_str() {
                "if-MACRO" => self.if_form(fx, payload, tail),
                "do-MACRO" => self.do_form(fx, payload, tail),
                "let-MACRO" => self.let_form(fx, payload, tail),
                "the-MACRO" => {
                    let Node::Tup(items) = self.arena.node(payload) else {
                        return Err("malformed The".into());
                    };
                    let expr = items[1];
                    self.expr(fx, expr, tail)
                }
                "match-MACRO" => self.match_form(fx, payload, tail),
                "fn-MACRO" => self.fn_form(fx, payload),
                "quote-MACRO" | "quasi-MACRO" | "def-MACRO" | "def-macro-MACRO" => {
                    Err(format!("`{name}` not supported by the wasm backend yet"))
                }
                _ if fx.lookup(&name).is_some() => self.closure_call(fx, head, payload, tail),
                _ if BUILTINS.contains(&name.as_str()) => self.builtin(fx, &name, payload),
                _ => {
                    if self.funcs.contains_key(&name) {
                        self.internal_call(fx, &name, payload, tail)
                    } else if self.value_globals.contains_key(&name) {
                        self.closure_call(fx, head, payload, tail)
                    } else {
                        Err(format!("unknown function `{name}` (wasm backend)"))
                    }
                }
            },
            // any other head evaluates to a closure box
            _ => self.closure_call(fx, head, payload, tail),
        }
    }

    fn if_form(&mut self, fx: &mut FnCtx, payload: NodeId, tail: bool) -> Result<(), String> {
        let Node::Tup(items) = self.arena.node(payload).clone() else {
            return Err("malformed If".into());
        };
        let (c, t, e) = (items[0], items[1], items[2]);
        self.expr(fx, c, false)?;
        fx.op(I::Call(self.h.truthy));
        fx.op(I::If(BlockType::Result(ValType::I32)));
        self.expr(fx, t, tail)?;
        fx.op(I::Else);
        self.expr(fx, e, tail)?;
        fx.op(I::End);
        Ok(())
    }

    fn do_form(&mut self, fx: &mut FnCtx, payload: NodeId, tail: bool) -> Result<(), String> {
        let items = self.payload_items(payload);
        if items.is_empty() {
            fx.op(I::I32Const(self.unit_addr() as i32));
            return Ok(());
        }
        for &x in &items[..items.len() - 1] {
            self.expr(fx, x, false)?;
            fx.op(I::Drop);
        }
        self.expr(fx, items[items.len() - 1], tail)
    }

    fn let_form(&mut self, fx: &mut FnCtx, payload: NodeId, tail: bool) -> Result<(), String> {
        let Node::Tup(items) = self.arena.node(payload).clone() else {
            return Err("malformed Let".into());
        };
        let Node::Rec(fields) = self.arena.node(items[0]).clone() else {
            return Err("Let bindings must be a record".into());
        };
        fx.scopes.push(HashMap::new());
        for (k, v) in &fields {
            self.expr(fx, *v, false)?;
            let l = fx.local(ValType::I32);
            fx.op(I::LocalSet(l));
            fx.scopes.last_mut().unwrap().insert(k.clone(), l);
        }
        let r = self.expr(fx, items[1], tail);
        fx.scopes.pop();
        r
    }

    /// Each clause is a block: a failed test branches past the clause; a
    /// matched clause leaves its result and branches to the end. No clause
    /// matching traps (the interpreter raises "no Match clause" instead).
    fn match_form(&mut self, fx: &mut FnCtx, payload: NodeId, tail: bool) -> Result<(), String> {
        let Node::Tup(items) = self.arena.node(payload).clone() else {
            return Err("malformed Match".into());
        };
        let Node::Lst(clauses) = self.arena.node(items[1]).clone() else {
            return Err("Match expects a list of (pattern result) clauses".into());
        };
        self.expr(fx, items[0], false)?;
        let scrut = fx.local(ValType::I32);
        fx.op(I::LocalSet(scrut));
        fx.op(I::Block(BlockType::Result(ValType::I32)));
        for &clause in &clauses {
            let pair = match self.arena.node(clause).clone() {
                Node::Tup(pair) if pair.len() == 2 => pair,
                _ => return Err("each Match clause must be a (pattern result) tuple".into()),
            };
            fx.op(I::Block(BlockType::Empty));
            fx.scopes.push(HashMap::new());
            let r = self
                .pattern(fx, pair[0], scrut, 0)
                .and_then(|()| self.expr(fx, pair[1], tail));
            fx.scopes.pop();
            r?;
            fx.op(I::Br(1));
            fx.op(I::End);
        }
        fx.op(I::Unreachable);
        fx.op(I::End);
        Ok(())
    }

    /// Compile a pattern test against the box in local `v`; on mismatch branch
    /// `fail` levels out (the enclosing clause block). Names bind into the
    /// current scope. Nested patterns keep `fail` because no blocks are opened.
    fn pattern(&mut self, fx: &mut FnCtx, pat: NodeId, v: u32, fail: u32) -> Result<(), String> {
        match self.arena.node(pat).clone() {
            // `none` (the only builtin payload-less variant in v0) matches by
            // equality; every other bare name binds. Mirrors the interpreter,
            // which keys this off names bound to a payload-less variant.
            Node::Sym(name) if name == "none" => {
                let naddr = self.intern_str("none");
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
                fx.op(I::I32Const(naddr as i32));
                fx.op(I::Call(self.h.eq_raw));
                fx.op(I::I32Eqz);
                fx.op(I::BrIf(fail));
                Ok(())
            }
            Node::Sym(name) => {
                let l = fx.local(ValType::I32);
                fx.op(I::LocalGet(v));
                fx.op(I::LocalSet(l));
                fx.scopes.last_mut().unwrap().insert(name, l);
                Ok(())
            }
            // variant case with payload: `ok(x)`, `some(x)`, `err(e)`, …
            Node::Call(head, vpayload) => {
                let Node::Sym(case) = self.arena.node(head).clone() else {
                    return Err("pattern call head must be a name".into());
                };
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
                let inner = fx.local(ValType::I32);
                fx.op(I::LocalGet(v));
                fx.op(I::I32Load(ma(8, 2)));
                fx.op(I::LocalTee(inner));
                fx.op(I::I32Eqz);
                fx.op(I::BrIf(fail));
                self.pattern(fx, vpayload, inner, fail)
            }
            Node::Int(_) | Node::Dec(_) | Node::Bool(_) | Node::Str(_) => {
                fx.op(I::LocalGet(v));
                self.expr(fx, pat, false)?;
                fx.op(I::Call(self.h.eq_raw));
                fx.op(I::I32Eqz);
                fx.op(I::BrIf(fail));
                Ok(())
            }
            Node::Lst(pats) => self.seq_pattern(fx, &pats, v, fail, TAG_LIST),
            Node::Tup(pats) => self.seq_pattern(fx, &pats, v, fail, TAG_TUP),
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
            _ => Err("pattern not supported by the wasm backend yet \
                      (literals, names, list/tuple, record, and variant patterns)"
                .into()),
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

    /// Mirror of the interpreter's §4.2 payload-binding rule, at compile time.
    fn bind_args(&self, payload: NodeId, params: &[String]) -> Result<Vec<NodeId>, String> {
        if let Node::Rec(fields) = self.arena.node(payload) {
            let mut keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
            let mut want: Vec<&str> = params.iter().map(|s| s.as_str()).collect();
            keys.sort();
            want.sort();
            if keys == want {
                let map: HashMap<&str, NodeId> =
                    fields.iter().map(|(k, v)| (k.as_str(), *v)).collect();
                return Ok(params.iter().map(|p| map[p.as_str()]).collect());
            }
        }
        if params.len() == 1 {
            return Ok(vec![payload]);
        }
        match self.arena.node(payload) {
            Node::Lst(xs) | Node::Tup(xs) if xs.len() == params.len() => Ok(xs.clone()),
            _ => Err(format!(
                "payload does not match parameters ({})",
                params.join(", ")
            )),
        }
    }

    fn internal_call(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        payload: NodeId,
        tail: bool,
    ) -> Result<(), String> {
        let (idx, params) = self.funcs[name].clone();
        let args = self.bind_args(payload, &params)?;
        for a in args {
            self.expr(fx, a, false)?;
        }
        fx.op(if tail { I::ReturnCall(idx) } else { I::Call(idx) });
        Ok(())
    }

    fn dep_call(
        &mut self,
        fx: &mut FnCtx,
        alias: &str,
        fname: &str,
        payload: NodeId,
    ) -> Result<(), String> {
        let imp = self
            .info
            .imports
            .iter()
            .find(|i| i.alias == alias)
            .ok_or(format!("unknown import alias `{alias}`"))?;
        let dep = self
            .deps
            .get(&imp.package)
            .ok_or(format!("dependency `{}` is not in the build set", imp.package))?;
        let iface = import_iface(&imp.path);
        let sig = dep
            .funcs
            .iter()
            .find(|f| f.name == fname && f.iface == iface)
            .ok_or(format!("`{}` does not export `{fname}` in `{iface}`", imp.package))?
            .clone();
        let module = versioned_iface(&dep.package, &iface);
        let fidx = self.import_idx(&module, fname);

        let param_names: Vec<String> = sig.params.iter().map(|(n, _)| n.clone()).collect();
        let args = self.bind_args(payload, &param_names)?;
        for (a, (_, t)) in args.iter().zip(&sig.params) {
            self.expr(fx, *a, false)?;
            let pty = wit_ty(t, &self.type_env)?;
            self.lower(fx, &pty)?;
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
                if matches!(rty, WitTy::Record(_) | WitTy::Option(_) | WitTy::Result(..)) {
                    // allocate a result area sized to the value, pass it as the
                    // canonical retptr, then read the value back out of it
                    let area = fx.local(ValType::I32);
                    fx.op(I::I32Const(size_of(&rty) as i32));
                    fx.op(I::Call(self.h.alloc));
                    fx.op(I::LocalTee(area));
                    fx.op(I::Call(fidx));
                    self.load_from_mem(fx, &rty, area, 0)?;
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
            WitTy::IntS | WitTy::IntU | WitTy::Handle => {
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::I32WrapI64);
            }
            WitTy::S64 => fx.op(I::Call(self.h.unbox_int)),
            WitTy::F64 => fx.op(I::Call(self.h.unbox_dec)),
            WitTy::Str => {
                let t = fx.local(ValType::I32);
                fx.op(I::LocalTee(t));
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::LocalGet(t));
                fx.op(I::I32Load(ma(4, 2)));
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
            WitTy::Option(_) | WitTy::Result(..) => {
                // variant box → [disc i32] ++ joined payload flats; both arms
                // produce the same flat shape (zero-padded where shorter)
                let cases = ty.variant_cases().unwrap();
                let full = flat(ty);
                let joined: Vec<ValType> = full[1..].to_vec();
                let resty = self.ty_idx(vec![], full);
                let b = fx.local(ValType::I32);
                fx.op(I::LocalSet(b));
                let n0 = self.intern_str(cases[0].0);
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::I32Const(n0 as i32));
                fx.op(I::Call(self.h.eq_raw));
                fx.op(I::If(BlockType::FunctionType(resty)));
                self.lower_variant_case(fx, b, 0, cases[0].1, &joined)?;
                fx.op(I::Else);
                self.lower_variant_case(fx, b, 1, cases[1].1, &joined)?;
                fx.op(I::End);
            }
        }
        Ok(())
    }

    /// One arm of a lowered option/result: push the discriminant, the payload's
    /// flats (if any), then zero-pad the remaining joined positions.
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
                fx.op(I::LocalGet(b));
                fx.op(I::I32Load(ma(8, 2)));
                self.lower(fx, pt)?;
                flat(pt).len()
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
    fn lift_list(&mut self, fx: &mut FnCtx, ptr: u32, len: u32, elem: &WitTy) -> Result<(), String> {
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
            WitTy::IntS => {
                fx.op(I::I64ExtendI32S);
                fx.op(I::Call(self.h.box_int));
            }
            WitTy::IntU | WitTy::Handle => {
                fx.op(I::I64ExtendI32U);
                fx.op(I::Call(self.h.box_int));
            }
            WitTy::S64 => fx.op(I::Call(self.h.box_int)),
            WitTy::F64 => fx.op(I::Call(self.h.box_dec)),
            WitTy::Str | WitTy::List(_) | WitTy::Record(_) | WitTy::Option(_) | WitTy::Result(..) => {
                unreachable!("never a single flat value")
            }
        }
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
            WitTy::Option(_) | WitTy::Result(..) => {
                // disc at `base`, payload union starting at `base + 1`
                let cases = ty.variant_cases().unwrap();
                fx.op(I::LocalGet(base));
                fx.op(I::If(BlockType::Result(ValType::I32)));
                self.lift_variant_case(fx, cases[1].0, cases[1].1, base + 1)?;
                fx.op(I::Else);
                self.lift_variant_case(fx, cases[0].0, cases[0].1, base + 1)?;
                fx.op(I::End);
            }
            _ => {
                fx.op(I::LocalGet(base));
                self.lift(fx, ty);
            }
        }
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
    ) -> Result<(), String> {
        match pay {
            Some(pt) => {
                self.lift_flat(fx, pt, payload_base)?;
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
            WitTy::IntS | WitTy::IntU | WitTy::Handle => {
                fx.op(I::LocalGet(dst));
                fx.op(I::LocalGet(src));
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::I32WrapI64);
                fx.op(I::I32Store(ma(off, 2)));
            }
            WitTy::S64 => {
                fx.op(I::LocalGet(dst));
                fx.op(I::LocalGet(src));
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::I64Store(ma(off, 3)));
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
            WitTy::Option(_) | WitTy::Result(..) => {
                let cases = ty.variant_cases().unwrap();
                let poff = variant_payload_offset(ty);
                let n0 = self.intern_str(cases[0].0);
                fx.op(I::LocalGet(src));
                fx.op(I::I32Load(ma(4, 2))); // TAG_VAR case-name box
                fx.op(I::I32Const(n0 as i32));
                fx.op(I::Call(self.h.eq_raw));
                fx.op(I::If(BlockType::Empty));
                self.store_variant_case(fx, src, 0, cases[0].1, dst, off, poff)?;
                fx.op(I::Else);
                self.store_variant_case(fx, src, 1, cases[1].1, dst, off, poff)?;
                fx.op(I::End);
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
            WitTy::IntS => {
                fx.op(I::LocalGet(src));
                fx.op(I::I32Load(ma(off, 2)));
                fx.op(I::I64ExtendI32S);
                fx.op(I::Call(self.h.box_int));
            }
            WitTy::IntU | WitTy::Handle => {
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
            WitTy::Option(_) | WitTy::Result(..) => {
                let cases = ty.variant_cases().unwrap();
                let poff = variant_payload_offset(ty);
                fx.op(I::LocalGet(src));
                fx.op(I::I32Load8U(ma(off, 0))); // discriminant
                fx.op(I::If(BlockType::Result(ValType::I32)));
                self.load_variant_case(fx, cases[1].0, cases[1].1, src, off + poff)?;
                fx.op(I::Else);
                self.load_variant_case(fx, cases[0].0, cases[0].1, src, off + poff)?;
                fx.op(I::End);
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

    fn builtin(&mut self, fx: &mut FnCtx, name: &str, payload: NodeId) -> Result<(), String> {
        let items = self.payload_items(payload);
        let nargs = |want: usize| -> Result<(), String> {
            if items.len() == want {
                Ok(())
            } else {
                Err(format!("`{name}` expects {want} argument(s), got {}", items.len()))
            }
        };
        match name {
            "eq" => {
                nargs(2)?;
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
                nargs(2)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.unbox_int));
                self.expr(fx, items[1], false)?;
                fx.op(I::Call(self.h.unbox_int));
                fx.op(match name {
                    "lt" => I::I64LtS,
                    "le" => I::I64LeS,
                    "gt" => I::I64GtS,
                    _ => I::I64GeS,
                });
                fx.op(I::Call(self.h.box_bool));
            }
            "add" | "sub" | "mul" | "div" | "rem" => {
                if items.is_empty() {
                    return Err(format!("`{name}` needs at least one argument"));
                }
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.unbox_int));
                for &x in &items[1..] {
                    self.expr(fx, x, false)?;
                    fx.op(I::Call(self.h.unbox_int));
                    fx.op(match name {
                        "add" => I::I64Add,
                        "sub" => I::I64Sub,
                        "mul" => I::I64Mul,
                        "div" => I::I64DivS,
                        _ => I::I64RemS,
                    });
                }
                fx.op(I::Call(self.h.box_int));
            }
            "neg" => {
                nargs(1)?;
                fx.op(I::I64Const(0));
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.unbox_int));
                fx.op(I::I64Sub);
                fx.op(I::Call(self.h.box_int));
            }
            "len" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.len_raw));
                fx.op(I::I64ExtendI32U);
                fx.op(I::Call(self.h.box_int));
            }
            "head" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.head_h));
            }
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
            "upper" | "lower" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::I32Const(if name == "upper" { 1 } else { 0 }));
                fx.op(I::Call(self.h.case_h));
            }
            "print" | "println" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                let h = if name == "print" { self.h.print_str } else { self.h.println_h };
                fx.op(I::Call(h.expect("print helpers emitted when used")));
                fx.op(I::I32Const(self.unit_addr() as i32));
            }
            "args" => {
                nargs(0)?;
                fx.op(I::Call(self.h.get_args.expect("args helper emitted when used")));
            }
            "some" | "ok" | "err" => {
                // the single argument is the whole payload (which may itself be
                // a list/tuple/record), exactly as the interpreter binds it
                return self.var_box(fx, name, payload);
            }
            other => return Err(format!("builtin `{other}` not supported by the wasm backend yet")),
        }
        Ok(())
    }
}

const BUILTINS: &[&str] = &[
    "eq", "not", "lt", "le", "gt", "ge", "add", "sub", "mul", "div", "rem", "neg", "len",
    "head", "str-cat", "upper", "lower", "to-string", "print", "println", "args",
    "some", "ok", "err",
];

// --------------------------------------------------------- helper bodies

fn emit_core_module(
    arena: &Arena,
    roots: &[NodeId],
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> Result<Vec<u8>, String> {
    let feats = features_of(arena, info);
    let is_command = info.target.as_deref() == Some("wasi:cli/command");

    // record types in scope: this file's own DefTypes, plus those of every dep
    let mut type_env: TypeEnv = HashMap::new();
    for (name, fields) in record_types(arena, &info.types) {
        type_env.insert(name, fields);
    }
    for dep in deps.values() {
        for (name, fields) in &dep.types {
            type_env.entry(name.clone()).or_insert_with(|| fields.clone());
        }
    }

    let mut em = Emitter {
        arena,
        info,
        deps,
        type_env,
        data: Vec::new(),
        str_cache: HashMap::new(),
        types: Vec::new(),
        imports: Vec::new(),
        import_fn: HashMap::new(),
        h: Helpers {
            alloc: 0,
            realloc: 0,
            box_int: 0,
            box_bool: 0,
            box_dec: 0,
            box_str: 0,
            truthy: 0,
            unbox_int: 0,
            unbox_dec: 0,
            eq_raw: 0,
            len_raw: 0,
            head_h: 0,
            strcat2: 0,
            case_h: 0,
            to_str: 0,
            rec_get: 0,
            print_str: None,
            println_h: None,
            get_args: None,
        },
        funcs: HashMap::new(),
        value_globals: HashMap::new(),
        compiling_values: Vec::new(),
        bodies: Vec::new(),
        closure_bodies: Vec::new(),
        fn_wrappers: HashMap::new(),
        fn_box_cache: HashMap::new(),
        var_box_cache: HashMap::new(),
        false_addr: 0,
        true_addr: 0,
        nl_addr: 0,
    };

    // static boxes: false @16, true @24, "\n" box
    em.false_addr = DATA_BASE;
    em.put_i32(TAG_BOOL);
    em.put_i32(0);
    em.true_addr = DATA_BASE + 8;
    em.put_i32(TAG_BOOL);
    em.put_i32(1);
    em.nl_addr = em.intern_str("\n");

    // ---- imports (function index space starts here)
    let mut n_imports = 0u32;
    let mut add_import = |em: &mut Emitter, module: &str, field: &str, p: Vec<ValType>, r: Vec<ValType>| {
        let t = em.ty_idx(p, r);
        em.imports.push((module.to_string(), field.to_string(), t));
        em.import_fn
            .insert((module.to_string(), field.to_string()), n_imports);
        n_imports += 1;
    };

    use ValType::{F64, I32, I64};
    let _ = (I64, F64);
    if feats.needs_stdout {
        add_import(&mut em, "wasi:cli/stdout@0.2.0", "get-stdout", vec![], vec![I32]);
        add_import(
            &mut em,
            "wasi:io/streams@0.2.0",
            "[method]output-stream.blocking-write-and-flush",
            vec![I32, I32, I32, I32],
            vec![],
        );
    }
    if feats.needs_env {
        add_import(&mut em, "wasi:cli/environment@0.2.0", "get-arguments", vec![I32], vec![]);
    }
    for (alias, fname) in &feats.dep_calls {
        let imp = info
            .imports
            .iter()
            .find(|i| &i.alias == alias)
            .ok_or(format!("unknown import alias `{alias}`"))?;
        let dep = deps
            .get(&imp.package)
            .ok_or(format!("dependency `{}` is not in the build set", imp.package))?;
        let iface = import_iface(&imp.path);
        let sig = dep
            .funcs
            .iter()
            .find(|f| &f.name == fname && f.iface == iface)
            .ok_or(format!("`{}` does not export `{fname}` in `{iface}`", imp.package))?;
        let mut p = Vec::new();
        for (_, t) in &sig.params {
            p.extend_from_slice(&flat_checked(&wit_ty(t, &em.type_env)?)?);
        }
        let r = match flat_result(sig, &em.type_env)? {
            FlatRes::None => vec![],
            FlatRes::One(t) => flat(&t),
            FlatRes::Retptr => {
                p.push(I32);
                vec![]
            }
        };
        let module = versioned_iface(&dep.package, &iface);
        add_import(&mut em, &module, fname, p, r);
    }

    // ---- assign helper indices
    let mut next = n_imports;
    let mut take = || {
        let i = next;
        next += 1;
        i
    };
    em.h.alloc = take();
    em.h.realloc = take();
    em.h.box_int = take();
    em.h.box_bool = take();
    em.h.box_dec = take();
    em.h.box_str = take();
    em.h.truthy = take();
    em.h.unbox_int = take();
    em.h.unbox_dec = take();
    em.h.eq_raw = take();
    em.h.len_raw = take();
    em.h.head_h = take();
    em.h.strcat2 = take();
    em.h.case_h = take();
    em.h.to_str = take();
    em.h.rec_get = take();
    if feats.needs_stdout {
        em.h.print_str = Some(take());
        em.h.println_h = Some(take());
    }
    if feats.needs_env {
        em.h.get_args = Some(take());
    }

    // ---- assign internal function indices (file order)
    let mut internal_order: Vec<String> = Vec::new();
    for &root in roots {
        if let Node::Call(h, p) = arena.node(root) {
            if matches!(arena.node(*h), Node::Sym(s) if s == "def-MACRO") {
                if let Node::Tup(items) = arena.node(*p) {
                    if let Node::Sym(name) = arena.node(items[0]) {
                        if info.defs.contains_key(name) && !internal_order.contains(name) {
                            internal_order.push(name.clone());
                        }
                    }
                }
            }
        }
    }
    for (i, (name, _)) in info.value_defs.iter().enumerate() {
        em.value_globals.insert(name.clone(), 1 + i as u32); // global 0 = heap ptr
    }
    for name in &internal_order {
        let (params_id, _) = info.defs[name];
        let params = param_names(arena, params_id)?;
        em.funcs.insert(name.clone(), (take(), params));
    }

    // ---- helper bodies (order must match index assignment above)
    emit_helpers(&mut em, &feats)?;

    // ---- internal function bodies
    for name in &internal_order {
        let (_, body) = info.defs[name];
        let params = em.funcs[name].1.clone();
        let n = params.len();
        let mut fx = FnCtx::new(n as u32);
        let mut scope = HashMap::new();
        for (i, p) in params.iter().enumerate() {
            scope.insert(p.clone(), i as u32);
        }
        fx.scopes.push(scope);
        em.expr(&mut fx, body, true)
            .map_err(|e| format!("in `{name}`: {e}"))?;
        let t = em.ty_idx(vec![I32; n], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // ---- export wrappers
    let mut exports: Vec<(String, u32)> = Vec::new(); // (export name, fn idx)
    for sig in &info.exports {
        let (fidx, _) = *em
            .funcs
            .get(&sig.name)
            .ok_or(format!("export `{}` has no Def Fn", sig.name))?;
        if is_command && sig.name == "run" {
            // wasi:cli/run@0.2.0#run: func() -> result
            let mut fx = FnCtx::new(0);
            fx.op(I::Call(fidx));
            fx.op(I::Drop);
            fx.op(I::I32Const(0)); // ok
            let t = em.ty_idx(vec![], vec![I32]);
            em.bodies.push((t, fx.finish()));
            exports.push(("wasi:cli/run@0.2.0#run".to_string(), take()));
            continue;
        }
        let mut fparams = Vec::new();
        let mut lifted: Vec<(WitTy, u32)> = Vec::new(); // (ty, first flat local)
        for (_, t) in &sig.params {
            let ty = wit_ty(t, &em.type_env)?;
            lifted.push((ty.clone(), fparams.len() as u32));
            fparams.extend_from_slice(&flat_checked(&ty)?);
        }
        if fparams.len() > 16 {
            return Err(format!(
                "`{}` flattens to {} parameters; spilling >16 params to memory \
                 is not supported by the wasm backend yet",
                sig.name,
                fparams.len()
            ));
        }
        let mut fx = FnCtx::new(fparams.len() as u32);
        for (ty, base) in &lifted {
            em.lift_flat(&mut fx, ty, *base)?;
        }
        fx.op(I::Call(fidx));
        let fresults = match flat_result(sig, &em.type_env)? {
            FlatRes::None => {
                fx.op(I::Drop);
                vec![]
            }
            FlatRes::One(t) => {
                em.lower(&mut fx, &t)?;
                flat(&t)
            }
            FlatRes::Retptr => {
                let ty = wit_ty(sig.result.as_deref().unwrap(), &em.type_env)?;
                let area = fx.local(I32);
                if matches!(ty, WitTy::Record(_) | WitTy::Option(_) | WitTy::Result(..)) {
                    // store the value's canonical layout into a callee-owned area
                    let rbox = fx.local(I32);
                    fx.op(I::LocalSet(rbox));
                    fx.op(I::I32Const(size_of(&ty) as i32));
                    fx.op(I::Call(em.h.alloc));
                    fx.op(I::LocalSet(area));
                    em.store_to_mem(&mut fx, &ty, rbox, area, 0)?;
                    fx.op(I::LocalGet(area));
                } else {
                    // string/list: lower to (ptr, len) parked in an 8-byte area
                    em.lower(&mut fx, &ty)?;
                    let lp = fx.local(I32);
                    let ll = fx.local(I32);
                    fx.op(I::LocalSet(ll));
                    fx.op(I::LocalSet(lp));
                    fx.op(I::I32Const(8));
                    fx.op(I::Call(em.h.alloc));
                    fx.op(I::LocalTee(area));
                    fx.op(I::LocalGet(lp));
                    fx.op(I::I32Store(ma(0, 2)));
                    fx.op(I::LocalGet(area));
                    fx.op(I::LocalGet(ll));
                    fx.op(I::I32Store(ma(4, 2)));
                    fx.op(I::LocalGet(area));
                }
                vec![I32]
            }
        };
        let t = em.ty_idx(fparams, fresults);
        em.bodies.push((t, fx.finish()));
        let own_iface = versioned_iface(&info.package, &sig.iface);
        exports.push((format!("{own_iface}#{}", sig.name), take()));
    }

    // ---- assemble
    let heap_base = {
        em.align8();
        DATA_BASE + em.data.len() as u32
    };
    let pages = (heap_base as u64 >> 16) + 1;

    let mut module = Module::new();
    let mut ts = TypeSection::new();
    for (p, r) in &em.types {
        ts.ty().function(p.iter().copied(), r.iter().copied());
    }
    module.section(&ts);

    let mut is = ImportSection::new();
    for (m, f, t) in &em.imports {
        is.import(m, f, EntityType::Function(*t));
    }
    module.section(&is);

    // closure/wrapper functions live after every directly-indexed function;
    // table slot k = function index closure_base + k
    let closure_base = n_imports + em.bodies.len() as u32;

    let mut fs = FunctionSection::new();
    for (t, _) in &em.bodies {
        fs.function(*t);
    }
    for (t, _) in &em.closure_bodies {
        fs.function(*t);
    }
    module.section(&fs);

    if !em.closure_bodies.is_empty() {
        let mut tbl = TableSection::new();
        tbl.table(TableType {
            element_type: RefType::FUNCREF,
            minimum: em.closure_bodies.len() as u64,
            maximum: Some(em.closure_bodies.len() as u64),
            table64: false,
            shared: false,
        });
        module.section(&tbl);
    }

    let mut ms = MemorySection::new();
    ms.memory(MemoryType {
        minimum: pages,
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    module.section(&ms);

    let mut gs = GlobalSection::new();
    gs.global(
        GlobalType { val_type: I32, mutable: true, shared: false },
        &ConstExpr::i32_const(heap_base as i32),
    );
    for _ in &info.value_defs {
        gs.global(
            GlobalType { val_type: I32, mutable: true, shared: false },
            &ConstExpr::i32_const(0),
        );
    }
    module.section(&gs);

    let mut es = ExportSection::new();
    es.export("memory", ExportKind::Memory, 0);
    es.export("cabi_realloc", ExportKind::Func, em.h.realloc);
    for (name, idx) in &exports {
        es.export(name, ExportKind::Func, *idx);
    }
    module.section(&es);

    if !em.closure_bodies.is_empty() {
        let idxs: Vec<u32> =
            (0..em.closure_bodies.len() as u32).map(|k| closure_base + k).collect();
        let mut els = ElementSection::new();
        els.active(
            Some(0),
            &ConstExpr::i32_const(0),
            Elements::Functions(idxs.into()),
        );
        module.section(&els);
    }

    let mut cs = CodeSection::new();
    for (_, f) in &em.bodies {
        cs.function(f);
    }
    for (_, f) in &em.closure_bodies {
        cs.function(f);
    }
    module.section(&cs);

    let mut ds = DataSection::new();
    ds.active(0, &ConstExpr::i32_const(DATA_BASE as i32), em.data.iter().copied());
    module.section(&ds);

    Ok(module.finish())
}

fn param_names(arena: &Arena, params_id: NodeId) -> Result<Vec<String>, String> {
    match arena.node(params_id) {
        Node::Flg(names) => Ok(names.clone()),
        Node::Rec(fields) => Ok(fields.iter().map(|(k, _)| k.clone()).collect()),
        _ => Err("malformed Fn parameters".into()),
    }
}

fn emit_helpers(em: &mut Emitter, feats: &Features) -> Result<(), String> {
    use ValType::{F64, I32, I64};

    // alloc(n) -> ptr   [locals: r=1, end=2]
    {
        let mut fx = FnCtx::new(1);
        let r = fx.local(I32);
        let end = fx.local(I32);
        fx.op(I::GlobalGet(0));
        fx.op(I::LocalSet(r));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Const(7));
        fx.op(I::I32Add);
        fx.op(I::I32Const(-8));
        fx.op(I::I32And);
        fx.op(I::LocalSet(0));
        fx.op(I::LocalGet(r));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(end));
        fx.op(I::LocalGet(end));
        fx.op(I::MemorySize(0));
        fx.op(I::I32Const(16));
        fx.op(I::I32Shl);
        fx.op(I::I32GtU);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(end));
        fx.op(I::MemorySize(0));
        fx.op(I::I32Const(16));
        fx.op(I::I32Shl);
        fx.op(I::I32Sub);
        fx.op(I::I32Const(0xffff));
        fx.op(I::I32Add);
        fx.op(I::I32Const(16));
        fx.op(I::I32ShrU);
        fx.op(I::MemoryGrow(0));
        fx.op(I::I32Const(-1));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(end));
        fx.op(I::GlobalSet(0));
        fx.op(I::LocalGet(r));
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // cabi_realloc(old, old_size, align, new_size) -> ptr
    {
        let mut fx = FnCtx::new(4);
        let p = fx.local(I32);
        fx.op(I::LocalGet(3));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(p));
        fx.op(I::LocalGet(1));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(1));
        fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
        fx.op(I::End);
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![I32, I32, I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // box_int(i64) -> ptr
    {
        let mut fx = FnCtx::new(1);
        let p = fx.local(I32);
        fx.op(I::I32Const(16));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalTee(p));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(0));
        fx.op(I::I64Store(ma(8, 3)));
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![I64], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // box_bool(i32) -> ptr (static boxes)
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::I32Const(em.true_addr as i32));
        fx.op(I::Else);
        fx.op(I::I32Const(em.false_addr as i32));
        fx.op(I::End);
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // box_dec(f64) -> ptr
    {
        let mut fx = FnCtx::new(1);
        let p = fx.local(I32);
        fx.op(I::I32Const(16));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalTee(p));
        fx.op(I::I32Const(TAG_DEC));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(0));
        fx.op(I::F64Store(ma(8, 3)));
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![F64], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // box_str(ptr, len) -> box
    {
        let mut fx = FnCtx::new(2);
        let p = fx.local(I32);
        fx.op(I::I32Const(8));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalTee(p));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(1));
        fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // truthy(box) -> i32
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::I32Const(1));
        fx.op(I::Else);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::I32Ne);
        fx.op(I::End);
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // unbox_int(box) -> i64 (traps unless tag int)
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        let t = em.ty_idx(vec![I32], vec![I64]);
        em.bodies.push((t, fx.finish()));
    }

    // unbox_dec(box) -> f64
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_DEC));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::F64Load(ma(8, 3)));
        let t = em.ty_idx(vec![I32], vec![F64]);
        em.bodies.push((t, fx.finish()));
    }

    // eq_raw(a, b) -> i32   [locals: ta=2, la=3, i=4]
    {
        let mut fx = FnCtx::new(2);
        let ta = fx.local(I32);
        let la = fx.local(I32);
        let i = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalTee(ta));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        // bool
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Eq);
        fx.op(I::Return);
        fx.op(I::End);
        // int
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::LocalGet(1));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::I64Eq);
        fx.op(I::Return);
        fx.op(I::End);
        // dec
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_DEC));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::F64Load(ma(8, 3)));
        fx.op(I::LocalGet(1));
        fx.op(I::F64Load(ma(8, 3)));
        fx.op(I::F64Eq);
        fx.op(I::Return);
        fx.op(I::End);
        // str
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalTee(la));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(la));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(8, 0)));
        fx.op(I::LocalGet(1));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(8, 0)));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::I32Const(1));
        fx.op(I::Return);
        fx.op(I::End);
        // lists & anything else: identity
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Eq);
        let t = em.ty_idx(vec![I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // len_raw(box) -> i32 (str or list)
    {
        let mut fx = FnCtx::new(1);
        let tg = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalTee(tg));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Eq);
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Eq);
        fx.op(I::I32Or);
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // head_h(list box) -> box
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(8, 2)));
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // strcat2(a, b) -> box   [locals la, lb, p]
    {
        let mut fx = FnCtx::new(2);
        let la = fx.local(I32);
        let lb = fx.local(I32);
        let p = fx.local(I32);
        for arg in [0u32, 1u32] {
            fx.op(I::LocalGet(arg));
            fx.op(I::I32Load(ma(0, 2)));
            fx.op(I::I32Const(TAG_STR));
            fx.op(I::I32Ne);
            fx.op(I::If(BlockType::Empty));
            fx.op(I::Unreachable);
            fx.op(I::End);
        }
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(la));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(lb));
        fx.op(I::I32Const(8));
        fx.op(I::LocalGet(la));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(lb));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalTee(p));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(la));
        fx.op(I::LocalGet(lb));
        fx.op(I::I32Add);
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(la));
        fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(la));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(1));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(lb));
        fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // case_h(s, up) -> box   [locals l, p, i, c]
    {
        let mut fx = FnCtx::new(2);
        let l = fx.local(I32);
        let p = fx.local(I32);
        let i = fx.local(I32);
        let c = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(l));
        fx.op(I::I32Const(8));
        fx.op(I::LocalGet(l));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalTee(p));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(l));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(l));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(8, 0)));
        fx.op(I::LocalSet(c));
        fx.op(I::LocalGet(1));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(b'a' as i32));
        fx.op(I::I32GeU);
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(b'z' as i32));
        fx.op(I::I32LeU);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(32));
        fx.op(I::I32Sub);
        fx.op(I::LocalSet(c));
        fx.op(I::End);
        fx.op(I::Else);
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(b'A' as i32));
        fx.op(I::I32GeU);
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(b'Z' as i32));
        fx.op(I::I32LeU);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(32));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(c));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(c));
        fx.op(I::I32Store8(ma(8, 0)));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // to_str(box) -> str box   [locals tag, n(i64), neg, buf, i]
    {
        let true_s = em.intern_str("true");
        let false_s = em.intern_str("false");
        let mut fx = FnCtx::new(1);
        let tag = fx.local(I32);
        let n = fx.local(I64);
        let neg = fx.local(I32);
        let buf = fx.local(I32);
        let i = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(tag));
        // string: identity
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::Return);
        fx.op(I::End);
        // bool: static "true"/"false"
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_BOOL));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(true_s as i32));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::I32Const(false_s as i32));
        fx.op(I::Return);
        fx.op(I::End);
        // anything but int from here traps
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::LocalSet(n));
        fx.op(I::I32Const(32));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(buf));
        fx.op(I::I32Const(32));
        fx.op(I::LocalSet(i));
        fx.op(I::LocalGet(n));
        fx.op(I::I64Const(0));
        fx.op(I::I64LtS);
        fx.op(I::LocalSet(neg));
        fx.op(I::LocalGet(neg));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I64Const(0));
        fx.op(I::LocalGet(n));
        fx.op(I::I64Sub);
        fx.op(I::LocalSet(n));
        fx.op(I::End);
        // digits, least significant first (unsigned ops so |i64::MIN| works)
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Sub);
        fx.op(I::LocalSet(i));
        fx.op(I::LocalGet(buf));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(n));
        fx.op(I::I64Const(10));
        fx.op(I::I64RemU);
        fx.op(I::I32WrapI64);
        fx.op(I::I32Const(b'0' as i32));
        fx.op(I::I32Add);
        fx.op(I::I32Store8(ma(0, 0)));
        fx.op(I::LocalGet(n));
        fx.op(I::I64Const(10));
        fx.op(I::I64DivU);
        fx.op(I::LocalSet(n));
        fx.op(I::LocalGet(n));
        fx.op(I::I64Const(0));
        fx.op(I::I64Ne);
        fx.op(I::BrIf(0));
        fx.op(I::End);
        fx.op(I::LocalGet(neg));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Sub);
        fx.op(I::LocalSet(i));
        fx.op(I::LocalGet(buf));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Const(b'-' as i32));
        fx.op(I::I32Store8(ma(0, 0)));
        fx.op(I::End);
        fx.op(I::LocalGet(buf));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Const(32));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Sub);
        fx.op(I::Call(em.h.box_str));
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // rec_get(rec, key) -> box   returns the value box for `key`, or 0 if the
    // record has no such field.   [locals n=2, i=3, base=4]
    {
        let mut fx = FnCtx::new(2);
        let n = fx.local(I32);
        let i = fx.local(I32);
        let base = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_REC));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(n));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(n));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // base = rec + 8*i ; field key @ ma(8), value @ ma(12)
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalSet(base));
        fx.op(I::LocalGet(base));
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalGet(1));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(base));
        fx.op(I::I32Load(ma(12, 2)));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::I32Const(0));
        let t = em.ty_idx(vec![I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    if feats.needs_stdout {
        let get_stdout = em.import_idx("wasi:cli/stdout@0.2.0", "get-stdout");
        let write = em.import_idx(
            "wasi:io/streams@0.2.0",
            "[method]output-stream.blocking-write-and-flush",
        );
        // print_str(box)
        {
            let mut fx = FnCtx::new(1);
            fx.op(I::Call(get_stdout));
            fx.op(I::LocalGet(0));
            fx.op(I::I32Const(8));
            fx.op(I::I32Add);
            fx.op(I::LocalGet(0));
            fx.op(I::I32Load(ma(4, 2)));
            fx.op(I::I32Const(SCRATCH));
            fx.op(I::Call(write));
            let t = em.ty_idx(vec![I32], vec![]);
            em.bodies.push((t, fx.finish()));
        }
        // println_h(box)
        {
            let mut fx = FnCtx::new(1);
            fx.op(I::LocalGet(0));
            fx.op(I::Call(em.h.print_str.unwrap()));
            fx.op(I::I32Const(em.nl_addr as i32));
            fx.op(I::Call(em.h.print_str.unwrap()));
            let t = em.ty_idx(vec![I32], vec![]);
            em.bodies.push((t, fx.finish()));
        }
    }

    if feats.needs_env {
        let get_arguments = em.import_idx("wasi:cli/environment@0.2.0", "get-arguments");
        // get_args() -> list box, dropping argv[0]
        // locals: base, n, m, lst, i, tmp, bx
        let mut fx = FnCtx::new(0);
        let base = fx.local(I32);
        let n = fx.local(I32);
        let m = fx.local(I32);
        let lst = fx.local(I32);
        let i = fx.local(I32);
        let tmp = fx.local(I32);
        let bx = fx.local(I32);
        fx.op(I::I32Const(SCRATCH));
        fx.op(I::Call(get_arguments));
        fx.op(I::I32Const(SCRATCH));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(base));
        fx.op(I::I32Const(SCRATCH));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(n));
        fx.op(I::LocalGet(n));
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::LocalGet(n));
        fx.op(I::I32Const(1));
        fx.op(I::I32Sub);
        fx.op(I::Else);
        fx.op(I::I32Const(0));
        fx.op(I::End);
        fx.op(I::LocalSet(m));
        fx.op(I::I32Const(8));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Const(2));
        fx.op(I::I32Shl);
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalTee(lst));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(lst));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(m));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // tmp = base + 8*(i+1)
        fx.op(I::LocalGet(base));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::I32Const(3));
        fx.op(I::I32Shl);
        fx.op(I::I32Add);
        fx.op(I::LocalSet(tmp));
        fx.op(I::LocalGet(tmp));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalGet(tmp));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::Call(em.h.box_str));
        fx.op(I::LocalSet(bx));
        fx.op(I::LocalGet(lst));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(2));
        fx.op(I::I32Shl);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(bx));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(lst));
        let t = em.ty_idx(vec![], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    Ok(())
}

// ----------------------------------------------------------- WIT synthesis

const WASI_PACKAGES: &str = r#"
package wasi:io@0.2.0 {
  interface error {
    resource error;
  }
  interface streams {
    use error.{error};
    variant stream-error {
      last-operation-failed(error),
      closed,
    }
    resource output-stream {
      blocking-write-and-flush: func(contents: list<u8>) -> result<_, stream-error>;
    }
  }
}
package wasi:cli@0.2.0 {
  interface stdout {
    use wasi:io/streams@0.2.0.{output-stream};
    get-stdout: func() -> output-stream;
  }
  interface environment {
    get-arguments: func() -> list<string>;
  }
  interface run {
    run: func() -> result;
  }
}
"#;

/// Render a dependency's nested-package WIT from its parsed surface.
pub fn dep_package_wit(arena: &Arena, info: &FileInfo) -> Result<String, String> {
    let mut out = format!("package {} {{\n", info.package);
    for iface in crate::wit::iface_order(&info.exports, !info.types.is_empty()) {
        out.push_str(&format!("  interface {iface} {{\n"));
        if iface == "api" {
            for (name, ty) in &info.types {
                out.push_str(&format!("    {}\n", type_decl(arena, name, *ty)?));
            }
        }
        for sig in info.exports.iter().filter(|s| s.iface == iface) {
            out.push_str(&format!("    {}\n", sig.to_wit()));
        }
        out.push_str("  }\n");
    }
    out.push_str("}\n");
    Ok(out)
}

fn synthesize_world_wit(
    arena: &Arena,
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
    feats: &Features,
) -> Result<String, String> {
    let is_command = info.target.as_deref() == Some("wasi:cli/command");
    let mut out = format!("package {};\n\n", info.package);

    let api_exports: Vec<&FuncSig> = info
        .exports
        .iter()
        .filter(|s| !(is_command && s.name == "run"))
        .collect();
    let ifaces = crate::wit::iface_order(
        &api_exports.iter().map(|s| (*s).clone()).collect::<Vec<_>>(),
        !info.types.is_empty(),
    );
    for iface in &ifaces {
        out.push_str(&format!("interface {iface} {{\n"));
        if iface == "api" {
            for (name, ty) in &info.types {
                out.push_str(&format!("  {}\n", type_decl(arena, name, *ty)?));
            }
        }
        for sig in api_exports.iter().filter(|s| &s.iface == iface) {
            out.push_str(&format!("  {}\n", sig.to_wit()));
        }
        out.push_str("}\n\n");
    }

    out.push_str(&format!("world {} {{\n", info.world));
    if feats.needs_stdout {
        out.push_str("  import wasi:cli/stdout@0.2.0;\n");
    }
    if feats.needs_env {
        out.push_str("  import wasi:cli/environment@0.2.0;\n");
    }
    for imp in &info.imports {
        let dep = deps
            .get(&imp.package)
            .ok_or(format!("dependency `{}` is not in the build set", imp.package))?;
        let iface = import_iface(&imp.path);
        out.push_str(&format!("  import {};\n", versioned_iface(&dep.package, &iface)));
    }
    for iface in &ifaces {
        out.push_str(&format!("  export {iface};\n"));
    }
    if is_command {
        out.push_str("  export wasi:cli/run@0.2.0;\n");
    }
    out.push_str("}\n");

    if feats.needs_stdout || feats.needs_env || is_command {
        out.push_str(WASI_PACKAGES);
    }
    for dep in deps.values() {
        out.push_str(&dep.package_wit);
    }
    Ok(out)
}
