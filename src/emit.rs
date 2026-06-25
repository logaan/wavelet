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
    /// non-record named types the dep defines (enum/variant/flags), so the
    /// generic bridge can lower/lift values of those kinds at the boundary.
    /// Defaulted empty for Wavelet deps, which only define records today.
    pub type_defs: Vec<(String, TypeDef)>,
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
    Char, // a Unicode scalar — i32 flat (u32 codepoint), carried in an int box
    IntS, // s8/s16/s32 — i32 flat, sign-extended into the int box
    IntU, // u8/u16/u32
    S64,  // s64/u64 — i64 flat
    F64,
    Str,
    List(Box<WitTy>),
    Record(Vec<(String, WitTy)>), // named record type, fully expanded
    /// An anonymous positional tuple (`tuple<a, b, …>`). Laid out in memory like
    /// a record with fields `0`, `1`, …; carried at the value level as a
    /// `TAG_TUP` box (element boxes at `@8+4i`).
    Tuple(Vec<WitTy>),
    Option(Box<WitTy>),
    Result(Box<WitTy>, Box<WitTy>),
    /// A resource handle (`own<T>`/`borrow<T>` or a bare wasi resource name).
    /// Opaque to Wavelet: a single i32 handle from the host, carried in an int
    /// box so ordinary code can pass it around without inspecting it.
    Handle,
    /// A WIT `enum` — a set of named, payload-less cases. A single i32 flat
    /// discriminant; carried at the value level as a payload-less `TAG_VAR` box
    /// (case name, no payload), the same box an option's `none` uses.
    Enum(Vec<String>),
    /// A WIT `variant` — named cases, each optionally carrying a payload. The
    /// general form of which option/result are the canonical 2-case specials:
    /// an i32 discriminant followed by the join of every case's payload flats.
    /// Carried at the value level as a `TAG_VAR` box (case name + payload box).
    Variant(Vec<(String, Option<WitTy>)>),
    /// A WIT `flags` — a set of named bit flags. For ≤32 flags this is a single
    /// i32 bitset; carried at the value level as a record box whose fields are
    /// the flag names mapped to bool boxes (set/clear).
    Flags(Vec<String>),
}

impl WitTy {
    /// A discriminated-union view: the canonical case order with each case's
    /// payload type. Covers option/result (the 2-case specials), explicit WIT
    /// `variant`s, and `enum`s (every case payload-less). Returns `None` for
    /// non-variant types.
    fn variant_cases(&self) -> Option<Vec<(&str, Option<&WitTy>)>> {
        match self {
            WitTy::Option(t) => Some(vec![("none", None), ("some", Some(t))]),
            WitTy::Result(t, e) => Some(vec![("ok", Some(t)), ("err", Some(e))]),
            WitTy::Variant(cases) => {
                Some(cases.iter().map(|(n, p)| (n.as_str(), p.as_ref())).collect())
            }
            WitTy::Enum(cases) => Some(cases.iter().map(|n| (n.as_str(), None)).collect()),
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

/// A named non-record WIT type definition (resource/enum/variant/flags), carried
/// as the type *strings* parsed from WIT so `wit_ty` can resolve a reference to
/// it. Records keep their own (legacy) map; everything else lands here.
#[derive(Clone)]
pub enum TypeDef {
    /// `resource` — an opaque host-owned type. Wavelet never inspects it; a
    /// reference to the bare name (or `own<name>`/`borrow<name>`) is a handle.
    Resource,
    /// `enum` — ordered, payload-less case names.
    Enum(Vec<String>),
    /// `variant` — ordered cases, each with an optional payload type-string.
    Variant(Vec<(String, Option<String>)>),
    /// `flags` — ordered flag names.
    Flags(Vec<String>),
}

/// Named WIT types in scope at a component boundary. Records resolve through
/// `records` (name → field (name, type-string)); enum/variant/flags through
/// `defs` (name → [`TypeDef`]). Split so the long-standing record path stays
/// byte-for-byte unchanged while the richer kinds are added alongside.
#[derive(Default)]
struct TypeEnv {
    records: HashMap<String, Vec<(String, String)>>,
    defs: HashMap<String, TypeDef>,
}

fn wit_ty(s: &str, env: &TypeEnv) -> Result<WitTy, String> {
    // A resource handle. `own<T>` / `borrow<T>` are always handles; a bare name
    // is a handle when the boundary `TypeEnv` declares it a `resource` (fed from
    // parsed WIT under `wit/deps`).
    if s.starts_with("own<")
        || s.starts_with("borrow<")
        || matches!(env.defs.get(s), Some(TypeDef::Resource))
    {
        return Ok(WitTy::Handle);
    }
    if let Some(inner) = s.strip_prefix("list<").and_then(|r| r.strip_suffix('>')) {
        return Ok(WitTy::List(Box::new(wit_ty(inner.trim(), env)?)));
    }
    if let Some(inner) = s.strip_prefix("tuple<").and_then(|r| r.strip_suffix('>')) {
        let mut elems = Vec::new();
        for arg in split_type_args(inner) {
            elems.push(wit_ty(&arg, env)?);
        }
        return Ok(WitTy::Tuple(elems));
    }
    if let Some(inner) = s.strip_prefix("option<").and_then(|r| r.strip_suffix('>')) {
        return Ok(WitTy::Option(Box::new(wit_ty(inner.trim(), env)?)));
    }
    if let Some(inner) = s.strip_prefix("result<").and_then(|r| r.strip_suffix('>')) {
        let args = split_type_args(inner);
        // Both arms typed keeps the existing `WitTy::Result` path byte-for-byte.
        // The single-arm and `_`-elided forms (`result<T>`, `result<_, E>`,
        // `result<T, _>`) become a 2-case `ok`/`err` variant where a missing or
        // `_` arm is payload-less — reusing the general variant lower/lift/store/
        // load machinery, with the same case names so `Match [(ok …)(err …)]`
        // still resolves. The canonical-ABI flattening is identical.
        let arm = |a: &str| -> Result<Option<WitTy>, String> {
            let a = a.trim();
            if a.is_empty() || a == "_" { Ok(None) } else { Ok(Some(wit_ty(a, env)?)) }
        };
        let (ok, err) = match args.len() {
            1 => (arm(&args[0])?, None),
            2 => {
                let ok = arm(&args[0])?;
                let err = arm(&args[1])?;
                if let (Some(o), Some(e)) = (&ok, &err) {
                    // Both arms typed → the legacy `WitTy::Result` representation.
                    return Ok(WitTy::Result(Box::new(o.clone()), Box::new(e.clone())));
                }
                (ok, err)
            }
            _ => {
                return Err(format!(
                    "`{s}`: a result takes at most two type arguments"
                ));
            }
        };
        return Ok(WitTy::Variant(vec![
            ("ok".to_string(), ok),
            ("err".to_string(), err),
        ]));
    }
    // A bare `result` (no arms) — both sides unit. Used by `wasi:cli/run`'s
    // `func() -> result`. Same `ok`/`err` 2-case variant, both payload-less.
    if s == "result" {
        return Ok(WitTy::Variant(vec![
            ("ok".to_string(), None),
            ("err".to_string(), None),
        ]));
    }
    Ok(match s {
        "bool" => WitTy::Bool,
        "char" => WitTy::Char,
        "s8" | "s16" | "s32" => WitTy::IntS,
        "u8" | "u16" | "u32" => WitTy::IntU,
        "s64" | "u64" => WitTy::S64,
        "f64" => WitTy::F64,
        "string" => WitTy::Str,
        other => {
            if let Some(fields) = env.records.get(other) {
                let mut resolved = Vec::with_capacity(fields.len());
                for (fname, fty) in fields {
                    resolved.push((fname.clone(), wit_ty(fty, env)?));
                }
                WitTy::Record(resolved)
            } else if let Some(def) = env.defs.get(other) {
                match def.clone() {
                    // Unreachable: a bare resource name is caught by the handle
                    // check at the top of `wit_ty`. Mapped for exhaustiveness.
                    TypeDef::Resource => WitTy::Handle,
                    TypeDef::Enum(cases) => WitTy::Enum(cases),
                    TypeDef::Flags(names) => WitTy::Flags(names),
                    TypeDef::Variant(cases) => {
                        let mut resolved = Vec::with_capacity(cases.len());
                        for (name, pay) in cases {
                            let pty = match pay {
                                Some(t) => Some(wit_ty(&t, env)?),
                                None => None,
                            };
                            resolved.push((name, pty));
                        }
                        WitTy::Variant(resolved)
                    }
                }
            } else {
                return Err(format!("type `{other}` not supported by the wasm backend yet"));
            }
        }
    })
}

/// Canonical-ABI `join` of two core value types for variant flattening: equal
/// types stay; `{i32, f32}` collapse to `i32` (same width, reinterpretable);
/// anything else widens to `i64` (the canonical "everything fits in 64 bits"
/// rule). See the Component Model canonical ABI `join`.
fn join_vt(a: ValType, b: ValType) -> ValType {
    use ValType::{F32, I32, I64};
    if a == b {
        a
    } else if matches!((a, b), (I32, F32) | (F32, I32)) {
        I32
    } else {
        I64
    }
}

/// Join two flat representations position-wise (canonical-ABI variant flatten),
/// widening per [`join_vt`]. Shared positions are widened to a common type;
/// trailing positions of the longer arm are kept as-is.
fn join_flat(a: &[ValType], b: &[ValType]) -> Result<Vec<ValType>, String> {
    let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    let mut out = long.to_vec();
    for (i, t) in short.iter().enumerate() {
        out[i] = join_vt(out[i], *t);
    }
    Ok(out)
}

/// Coerce a value of core type `have` (on the stack) into the joined slot type
/// `want`, per the canonical ABI's variant payload widening. Used when *lowering*
/// a variant arm whose payload flat is narrower than the joined union slot.
fn coerce_flat_to(fx: &mut FnCtx, have: ValType, want: ValType) {
    use ValType::{F32, F64, I32, I64};
    match (have, want) {
        _ if have == want => {}
        // i32 → i64 (zero-extend; the canonical ABI treats the lane as a bag of
        // bits, and the lifting side narrows it back).
        (I32, I64) => fx.op(I::I64ExtendI32U),
        // f32 → i32 (reinterpret bits), then possibly widen to i64.
        (F32, I32) => fx.op(I::I32ReinterpretF32),
        (F32, I64) => {
            fx.op(I::I32ReinterpretF32);
            fx.op(I::I64ExtendI32U);
        }
        // f64 → i64 (reinterpret bits).
        (F64, I64) => fx.op(I::I64ReinterpretF64),
        // Any remaining combination is unreachable for canonical `join` outputs
        // (`want` is only ever the original type, `i32`, or `i64`). Leave the
        // value untouched — a real mismatch then fails wasm validation loudly
        // rather than silently corrupting the stack.
        _ => {}
    }
}

/// Reverse of [`coerce_flat_to`]: a value read from a joined slot of type `from`
/// (on the stack) is narrowed back to the arm payload's core type `to`. Used
/// when *lifting* a variant arm from flat locals.
fn coerce_flat_from(fx: &mut FnCtx, from: ValType, to: ValType) {
    use ValType::{F32, F64, I32, I64};
    match (from, to) {
        _ if from == to => {}
        (I64, I32) => fx.op(I::I32WrapI64),
        (I32, F32) => fx.op(I::F32ReinterpretI32),
        (I64, F32) => {
            fx.op(I::I32WrapI64);
            fx.op(I::F32ReinterpretI32);
        }
        (I64, F64) => fx.op(I::F64ReinterpretI64),
        _ => {}
    }
}

fn flat(ty: &WitTy) -> Vec<ValType> {
    flat_checked(ty).expect("flat() on an unsupported boundary type")
}

/// Number of flat (core) values a type lowers to. Unlike [`flat_checked`] this
/// never needs the variant-join to succeed — it just counts — so it is safe to
/// use when only the count matters (deciding direct return vs retptr).
fn flat_len(ty: &WitTy) -> usize {
    match ty {
        WitTy::Bool
        | WitTy::Char
        | WitTy::IntS
        | WitTy::IntU
        | WitTy::S64
        | WitTy::F64
        | WitTy::Handle
        | WitTy::Enum(_)
        | WitTy::Flags(_) => 1,
        WitTy::Str | WitTy::List(_) => 2,
        WitTy::Record(fields) => fields.iter().map(|(_, t)| flat_len(t)).sum(),
        WitTy::Tuple(elems) => elems.iter().map(flat_len).sum(),
        WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_) => {
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
        WitTy::Bool
        | WitTy::Char
        | WitTy::IntS
        | WitTy::IntU
        | WitTy::Handle
        | WitTy::Enum(_)
        | WitTy::Flags(_) => vec![ValType::I32],
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
        WitTy::Tuple(elems) => {
            let mut v = Vec::new();
            for t in elems {
                v.extend(flat_checked(t)?);
            }
            v
        }
        WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_) => {
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
        WitTy::Char | WitTy::Handle => 4,
        WitTy::IntS | WitTy::IntU => 4, // s8/s16 widen to 4 here (we only box i32)
        WitTy::S64 | WitTy::F64 => 8,
        WitTy::Str | WitTy::List(_) => 4, // (ptr, len), pointer-aligned
        WitTy::Record(fields) => fields.iter().map(|(_, t)| align_of(t)).max().unwrap_or(1),
        WitTy::Tuple(elems) => elems.iter().map(align_of).max().unwrap_or(1),
        // enum: just the discriminant; flags: the bitset word(s).
        WitTy::Enum(cases) => disc_size(cases.len()),
        WitTy::Flags(names) => flags_align(names.len()),
        WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_) => {
            // align is the max of the discriminant's own size and any payload align
            let cases = ty.variant_cases().unwrap();
            cases
                .iter()
                .filter_map(|(_, p)| p.map(align_of))
                .max()
                .unwrap_or(1)
                .max(disc_size(cases.len()))
        }
    }
}

/// Canonical-ABI discriminant size (bytes) for a tag with `n` cases: the
/// smallest of 1/2/4 that can hold the case index.
fn disc_size(n: usize) -> u64 {
    if n <= 0x100 {
        1
    } else if n <= 0x10000 {
        2
    } else {
        4
    }
}

/// Canonical-ABI alignment of a `flags` with `n` members: 1 word (i32) for
/// ≤32 flags, then 4-byte alignment for the multi-word bitset.
fn flags_align(n: usize) -> u64 {
    let _ = n;
    4
}

/// Offset of a variant's payload (after the discriminant, padded to the
/// variant's alignment).
fn variant_payload_offset(ty: &WitTy) -> u64 {
    let n = ty.variant_cases().map(|c| c.len()).unwrap_or(0);
    align_up(disc_size(n), align_of(ty))
}

/// Canonical-ABI size (bytes) in memory.
fn size_of(ty: &WitTy) -> u64 {
    match ty {
        WitTy::Bool => 1,
        WitTy::Char | WitTy::Handle => 4,
        WitTy::IntS | WitTy::IntU => 4,
        WitTy::S64 | WitTy::F64 => 8,
        WitTy::Str | WitTy::List(_) => 8,
        WitTy::Enum(cases) => disc_size(cases.len()),
        WitTy::Flags(names) => flags_size(names.len()),
        WitTy::Record(_) | WitTy::Tuple(_) => {
            let a = align_of(ty);
            let mut off = 0u64;
            for (o, t) in record_field_offsets(ty) {
                off = o + size_of(&t);
            }
            align_up(off, a)
        }
        WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_) => {
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

/// Canonical-ABI size (bytes) of a `flags` with `n` members: 1/2/4 bytes for
/// ≤8/≤16/≤32 flags, then a 4-byte word per 32 flags.
fn flags_size(n: usize) -> u64 {
    if n <= 8 {
        1
    } else if n <= 16 {
        2
    } else {
        (n as u64).div_ceil(32) * 4
    }
}

fn align_up(off: u64, align: u64) -> u64 {
    (off + align - 1) / align * align
}

/// (offset, field-type) for each field of a record or element of a tuple, in
/// declaration order. Tuples lay out exactly like records (canonical-ABI treats
/// them identically — positional fields with the same alignment rules).
fn record_field_offsets(ty: &WitTy) -> Vec<(u64, WitTy)> {
    let fts: Vec<&WitTy> = match ty {
        WitTy::Record(fields) => fields.iter().map(|(_, ft)| ft).collect(),
        WitTy::Tuple(elems) => elems.iter().collect(),
        _ => return vec![],
    };
    let mut off = 0u64;
    let mut out = Vec::with_capacity(fts.len());
    for ft in fts {
        off = align_up(off, align_of(ft));
        out.push((off, ft.clone()));
        off += size_of(ft);
    }
    out
}

/// canonical-ABI element size for list payloads
/// Whether a `list<elem>` may be supplied from a Wavelet string (its bytes used
/// directly). True for the integer element kinds — in practice `list<u8>`, the
/// canonical byte-buffer type (`wasi:io` write, http bodies). The actual choice
/// is made at runtime on the value's box tag, so a real list still builds
/// element-by-element; only a string value takes the zero-copy bytes path.
fn is_byte_elem(ty: &WitTy) -> bool {
    matches!(ty, WitTy::IntU | WitTy::IntS)
}

fn elem_size(ty: &WitTy) -> u64 {
    match ty {
        WitTy::Bool => 1,
        WitTy::Char | WitTy::IntS | WitTy::IntU | WitTy::Handle => 4,
        WitTy::S64 | WitTy::F64 | WitTy::Str | WitTy::List(_) => 8,
        WitTy::Enum(_) | WitTy::Flags(_) => size_of(ty),
        WitTy::Record(_)
        | WitTy::Tuple(_)
        | WitTy::Option(_)
        | WitTy::Result(..)
        | WitTy::Variant(_) => size_of(ty),
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
    /// unique (alias, func) cross-component calls, in first-use order
    dep_calls: Vec<(String, String)>,
}

/// Result of binding a call's argument forms to a callee's parameters.
enum BoundArgs {
    /// one argument form per parameter, in parameter order
    PerParam(Vec<NodeId>),
    /// the sole parameter receives every argument bundled as one tuple
    Bundle,
}

fn scan(arena: &Arena, id: NodeId, feats: &mut Features) {
    match arena.node(id) {
        // A call is a tuple whose head (items[0]) may be a cross-component
        // (Qsym) dependency; recurse over every element either way.
        Node::Tup(items) => {
            if let Some(&head) = items.first() {
                if let Node::Qsym(alias, name) = arena.node(head) {
                    let key = (alias.clone(), name.clone());
                    if !feats.dep_calls.contains(&key) {
                        feats.dep_calls.push(key);
                    }
                }
            }
            for &x in items {
                scan(arena, x, feats);
            }
        }
        Node::Lst(xs) => {
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
    tail_h: u32,
    strcat2: u32,
    case_h: u32,
    to_str: u32,
    rec_get: u32,
    as_f64: u32,
    arith_raw: u32,
    cmp_raw: u32,
    neg_raw: u32,
}

// ---------------------------------------------------------------- emitter

pub fn emit_component(
    arena: &Arena,
    roots: &[NodeId],
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> Result<Vec<u8>, String> {
    // The wasm backend does not yet emit functor components. `wavelet wit` and
    // `wavelet run` support functors (the interpreter is the oracle, see
    // `builtins`'s `set-*` ops), but `build` does not: the synthesized world
    // *exports* a `set` resource whose `new`/`add`/`contains`/`size` methods have
    // no source bodies, so emit would have to generate a resource implementation
    // and hand out canonical-ABI `own<set>` handles — machinery this backend does
    // not have. Rather than emit a component that validates yet diverges from the
    // interpreter (the one thing the project treats as a hard bug), fail with a
    // clear, honest error. See `dev-notes/dd-type-system.typ` (functors are an
    // open question for the binary/emit path) and the CHANGELOG.
    if let Some(f) = info.functors.first() {
        return Err(format!(
            "the wasm backend cannot yet build functor components \
             (`Import {{pkg: \"…coll/set\" elem: … as: {alias}}}`): the synthesized \
             world exports a `{iface}` resource with no emittable method bodies. \
             `wavelet wit` and `wavelet run` support this functor; `wavelet build` \
             does not yet. Track this in the type-system design notes.",
            alias = f.alias,
            iface = f.iface,
        ));
    }
    let mut module = emit_core_module(arena, roots, info, deps)?;
    let wit = synthesize_world_wit(arena, info, deps)?;

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

/// A macro definition collected from a macro-library file: the unsuffixed name,
/// its parameter names (bound to argument *forms*), the body form, and arity.
struct MacroDef {
    name: String,
    params: Vec<String>,
    body: NodeId,
}

/// The WIT for a produced macro component: the `wavelet:macro-guest` world
/// (exporting `wavelet:meta/macros`) plus the canonical `wavelet:meta` package
/// (`code` + `macros`), as a nested package block. Mirrors
/// `tools/macro-guest/wit/{world,deps/wavelet-meta/code}.wit`.
fn macro_component_wit() -> String {
    // Kept in sync with `wit/meta/code.wit` (pinned); the nested form lets a
    // single `push_str` carry both packages, as dep WIT does.
    "package wavelet:macro-guest@0.1.0;\n\
\n\
world macro-lib {\n\
  export wavelet:meta/macros@0.1.0;\n\
}\n\
\n\
package wavelet:meta@0.1.0 {\n\
  interface code {\n\
    type node-id = u32;\n\
    variant node {\n\
      bool-val(bool),\n\
      int-val(s64),\n\
      dec-val(f64),\n\
      char-val(char),\n\
      str-val(string),\n\
      sym(string),\n\
      qsym(tuple<string, string>),\n\
      tup(list<node-id>),\n\
      lst(list<node-id>),\n\
      rec(list<tuple<string, node-id>>),\n\
      flg(list<string>),\n\
    }\n\
    record tree {\n\
      nodes: list<node>,\n\
      root: node-id,\n\
      spans: list<tuple<u32, u32>>,\n\
    }\n\
  }\n\
  interface macros {\n\
    use code.{tree};\n\
    manifest: func() -> list<tuple<string, u32>>;\n\
    expand: func(name: string, args: tree) -> result<tree, string>;\n\
  }\n\
}\n"
        .to_string()
}

/// The `wavelet:meta` `node` variant as a backend [`WitTy`], for lifting an
/// incoming `tree` and lowering an outgoing one through the generic boundary
/// bridge. Mirrors `wit/meta/code.wit` exactly.
fn meta_node_wit_ty() -> WitTy {
    let nid = WitTy::IntU; // node-id = u32
    WitTy::Variant(vec![
        ("bool-val".into(), Some(WitTy::Bool)),
        ("int-val".into(), Some(WitTy::S64)),
        ("dec-val".into(), Some(WitTy::F64)),
        ("char-val".into(), Some(WitTy::Char)),
        ("str-val".into(), Some(WitTy::Str)),
        ("sym".into(), Some(WitTy::Str)),
        ("qsym".into(), Some(WitTy::Tuple(vec![WitTy::Str, WitTy::Str]))),
        ("tup".into(), Some(WitTy::List(Box::new(nid.clone())))),
        ("lst".into(), Some(WitTy::List(Box::new(nid.clone())))),
        (
            "rec".into(),
            Some(WitTy::List(Box::new(WitTy::Tuple(vec![WitTy::Str, nid.clone()])))),
        ),
        ("flg".into(), Some(WitTy::List(Box::new(WitTy::Str)))),
    ])
}

/// The `wavelet:meta` `tree` record as a backend [`WitTy`].
fn meta_tree_wit_ty() -> WitTy {
    WitTy::Record(vec![
        ("nodes".into(), WitTy::List(Box::new(meta_node_wit_ty()))),
        ("root".into(), WitTy::IntU),
        (
            "spans".into(),
            WitTy::List(Box::new(WitTy::Tuple(vec![WitTy::IntU, WitTy::IntU]))),
        ),
    ])
}

/// Build a `wavelet:meta/macros` component from a macro-library file's forms
/// (design.md §6.3; **strategy B: compile the bodies**). The result is an
/// ordinary compiled component whose `manifest`/`expand` are compiled wasm —
/// no interpreter in the guest. Each macro body compiles like any function
/// (params bound to argument *forms* as boxes); `expand` converts the incoming
/// `tree` to box forms, dispatches to the compiled body, and converts the
/// result form back to a `tree`.
pub fn emit_macro_component(arena: &Arena, roots: &[NodeId]) -> Result<Vec<u8>, String> {
    let mut module = emit_macro_core_module(arena, roots)?;
    let wit = macro_component_wit();

    let mut resolve = wit_parser::Resolve::default();
    let pkg = resolve
        .push_str("wavelet-macro.wit", &wit)
        .map_err(|e| format!("internal: macro WIT did not parse: {e:#}\n--- WIT ---\n{wit}"))?;
    let world = resolve
        .select_world(&[pkg], Some("macro-lib"))
        .map_err(|e| format!("internal: macro world selection failed: {e:#}"))?;
    wit_component::embed_component_metadata(
        &mut module,
        &resolve,
        world,
        wit_component::StringEncoding::UTF8,
    )
    .map_err(|e| format!("embedding macro component metadata failed: {e:#}"))?;

    wit_component::ComponentEncoder::default()
        .validate(true)
        .module(&module)
        .map_err(|e| format!("componentizing the macro library failed: {e:#}"))?
        .encode()
        .map_err(|e| format!("encoding the macro-library component failed: {e:#}"))
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

/// The default version for an external interface whose package isn't resolved
/// to a [`Dep`] (so its pinned version is unknown). External WIT now comes from
/// `wit/deps`, so [`external_versioned_in`] supplies the real version; this is
/// only the fallback.
const WASI_VERSION: &str = "0.2.0";

/// An export/import that names an external WIT interface directly — e.g.
/// `wasi:http/incoming-handler` — rather than a local interface like `api`.
fn is_external_iface(iface: &str) -> bool {
    iface.contains(':')
}

/// Version an external interface path to the version we vendor:
/// `wasi:http/incoming-handler` → `wasi:http/incoming-handler@0.2.0`.
fn external_versioned(path: &str) -> String {
    format!("{path}@{WASI_VERSION}")
}

/// Version an external interface path (`ns:pkg/iface`) using the version of the
/// resolved [`Dep`] for its package, when one is in scope — the generic export
/// path, whose WIT comes from `wit/deps` at whatever version `wkg` pinned. Falls
/// back to [`external_versioned`] (the hardcoded WASI version) for the magic
/// http/cli path, which has no `Dep` for its vendored interfaces.
///
/// `ns:greet/greeter` with a dep `greet` at `acme:greet@0.1.0` → `…@0.1.0`.
fn external_versioned_in(path: &str, deps: &HashMap<String, Dep>) -> String {
    if let Some((pkg, _iface)) = path.split_once('/')
        && let Some(dep) = deps.get(pkg)
        && let Some((_base, ver)) = dep.package.split_once('@')
    {
        return format!("{path}@{ver}");
    }
    external_versioned(path)
}

/// `("demo:shout@0.1.0", "api")` → `"demo:shout/api@0.1.0"`
fn versioned_iface(pkg: &str, iface: &str) -> String {
    match pkg.split_once('@') {
        Some((base, ver)) => format!("{base}/{iface}@{ver}"),
        None => format!("{pkg}/{iface}"),
    }
}

/// The source-visible operation name a (possibly mangled) WIT function name is
/// reached by. A freestanding `f` is called as `f`; a resource operation is
/// called by its *bare op name*:
///
/// - `[constructor]res`      → `res`
/// - `[method]res.op`        → `op`
/// - `[static]res.op`        → `op`
/// - `[resource-drop]res`    → `drop-res`  (synthetic, see [`crate::witdep`])
///
/// So `r/body` resolves to `[method]outgoing-response.body`, `r/fields` to
/// `[constructor]fields`, and `r/drop-output-stream` to
/// `[resource-drop]output-stream`. Drop is spelled `drop-<res>` (not the bare
/// `<res>`) so it never collides with the resource's own constructor.
fn dep_func_op(name: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    if let Some(rest) = name.strip_prefix("[constructor]") {
        return Cow::Borrowed(rest);
    }
    if let Some(rest) = name.strip_prefix("[resource-drop]") {
        return Cow::Owned(format!("drop-{rest}"));
    }
    for prefix in ["[method]", "[static]"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            // `res.op` → `op`
            return Cow::Borrowed(rest.rsplit_once('.').map(|(_, op)| op).unwrap_or(rest));
        }
    }
    Cow::Borrowed(name)
}

/// The *resource-qualified* source name for a resource operation, used to
/// disambiguate when several resources in one interface share a bare op name
/// (e.g. `wasi:http/types` has both `outgoing-request.body` and
/// `outgoing-response.body`). Since a Wavelet qualified name is kebab-only (no
/// `.`), the qualifier joins with `-`:
///
/// - `[method]outgoing-response.body` → `outgoing-response-body`
/// - `[static]response-outparam.set`  → `response-outparam-set`
/// - `[constructor]fields`            → `fields` (same as the bare op)
///
/// A freestanding function or a drop has no qualified form (`None`).
fn dep_func_qualified(name: &str) -> Option<String> {
    if let Some(rest) = name.strip_prefix("[constructor]") {
        return Some(rest.to_string());
    }
    for prefix in ["[method]", "[static]"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            // `res.op` → `res-op`
            return Some(rest.replacen('.', "-", 1));
        }
    }
    None
}

/// Resolve a source-visible op name to the dep's [`FuncSig`] in `iface`.
///
/// Matching is two-tier so that the common bare-op spelling stays terse while
/// collisions stay resolvable:
/// 1. An *exact* match — the mangled WIT name, the resource-qualified
///    `res-op` form ([`dep_func_qualified`]), or a freestanding name — wins
///    outright. This is unique by construction (WIT names are unique per
///    interface), so `outgoing-response-body` selects exactly that method.
/// 2. Otherwise the *bare* op name ([`dep_func_op`]) is tried. If two resources
///    share it, the call is ambiguous and the source must use the qualified
///    form instead.
fn resolve_dep_func<'a>(
    dep: &'a Dep,
    iface: &str,
    fname: &str,
) -> Result<&'a crate::wit::FuncSig, String> {
    let in_iface = || dep.funcs.iter().filter(|f| f.iface == iface);

    // Tier 1: an exact mangled-name / qualified-name / freestanding match.
    if let Some(f) = in_iface()
        .find(|f| f.name == fname || dep_func_qualified(&f.name).as_deref() == Some(fname))
    {
        return Ok(f);
    }

    // Tier 2: the bare op name, rejecting genuine collisions.
    let mut bare = in_iface().filter(|f| dep_func_op(&f.name) == *fname);
    let first = bare.next().ok_or(format!(
        "`{}` does not export `{fname}` in `{iface}`",
        dep.package
    ))?;
    if let Some(second) = bare.next() {
        return Err(format!(
            "`{fname}` is ambiguous in `{}/{iface}`: matches both `{}` and `{}`; \
             use the resource-qualified name (e.g. `{}`)",
            dep.package,
            first.name,
            second.name,
            dep_func_qualified(&first.name).unwrap_or_else(|| first.name.clone()),
        ));
    }
    Ok(first)
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
    /// In a macro component, the function index of the guest-internal one-step
    /// expander, so the `expand` builtin (used *inside* a macro body) can call
    /// it. `None` in an ordinary module, where `expand` is unsupported.
    macro_expand_idx: Option<u32>,
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
            Node::Char(c) => {
                fx.op(I::I64Const(c as u32 as i64));
                self.box_char(fx);
            }
            Node::Sym(name) => match fx.lookup(&name) {
                Some(idx) => fx.op(I::LocalGet(idx)),
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
            Node::Flg(names) => return Ok(self.flg_box(fx, &names)),
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
                if items.len() == 2 {
                    if let Node::Sym(name) = self.arena.node(items[0]).clone() {
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
            if depth == 1 {
                if let Node::Tup(t) = self.arena.node(item).clone() {
                    if t.len() == 2 {
                        if let Node::Sym(s) = self.arena.node(t[0]).clone() {
                            if s == "splice-MACRO" {
                                segs.push((true, t[1]));
                                continue;
                            }
                        }
                    }
                }
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
        let (fidx, params) = self.funcs[name].clone();
        let mut fx = FnCtx::new(2);
        match params.len() {
            0 => {}
            1 => fx.op(I::LocalGet(1)),
            n => {
                // payload must be a list box of exactly n elements, same guard
                // `fn_form` emits, so a malformed indirect call traps rather
                // than reading garbage past the box.
                fx.op(I::LocalGet(1));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_LIST));
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
    fn fn_form(&mut self, fx: &mut FnCtx, args: &[NodeId]) -> Result<(), String> {
        let [params_id, body] = *args else {
            return Err("malformed Fn".into());
        };
        let params = param_names(self.arena, params_id)?;

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
            I::ReturnCallIndirect { type_index: t, table_index: 0 }
        } else {
            I::CallIndirect { type_index: t, table_index: 0 }
        });
        Ok(())
    }

    /// Bundle a call's evaluated arguments into one payload box, mirroring the
    /// interpreter: 0 args ⇒ an empty list box, 1 arg ⇒ the value itself, ≥2 ⇒ a
    /// list box. (A record arg binds by name, a list/tuple by order, a scalar to
    /// the sole parameter — matching `bind_args`.)
    fn payload_box(&mut self, fx: &mut FnCtx, args: &[NodeId]) -> Result<(), String> {
        match args {
            [] => self.list_box(fx, &[]),
            [one] => self.expr(fx, *one, false),
            many => self.list_box(fx, many),
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
                // sibling `.wvl` or a `wit/deps` package — host `wasi:*`
                // packages included).
                self.dep_call(fx, &alias, &fname, args)
            }
            Node::Sym(name) => match name.as_str() {
                "if-MACRO" => self.if_form(fx, args, tail),
                "do-MACRO" => self.do_form(fx, args, tail),
                "let-MACRO" => self.let_form(fx, args, tail),
                "the-MACRO" => {
                    // args = [ty, expr]
                    let [_ty, expr] = *args else {
                        return Err("malformed The".into());
                    };
                    self.expr(fx, expr, tail)
                }
                "match-MACRO" => self.match_form(fx, args, tail),
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
                _ if fx.lookup(&name).is_some() => self.closure_call(fx, head, args, tail),
                _ if BUILTINS.contains(&name.as_str()) => self.builtin(fx, &name, args),
                _ => {
                    if self.funcs.contains_key(&name) {
                        self.internal_call(fx, &name, args, tail)
                    } else if self.value_globals.contains_key(&name) {
                        self.closure_call(fx, head, args, tail)
                    } else {
                        Err(format!("unknown function `{name}` (wasm backend)"))
                    }
                }
            },
            // any other head evaluates to a closure box
            _ => self.closure_call(fx, head, args, tail),
        }
    }

    fn if_form(&mut self, fx: &mut FnCtx, args: &[NodeId], tail: bool) -> Result<(), String> {
        let [c, t, e] = *args else {
            return Err("malformed If".into());
        };
        self.expr(fx, c, false)?;
        fx.op(I::Call(self.h.truthy));
        fx.op(I::If(BlockType::Result(ValType::I32)));
        self.expr(fx, t, tail)?;
        fx.op(I::Else);
        self.expr(fx, e, tail)?;
        fx.op(I::End);
        Ok(())
    }

    fn do_form(&mut self, fx: &mut FnCtx, args: &[NodeId], tail: bool) -> Result<(), String> {
        let [list] = *args else {
            return Err("malformed Do".into());
        };
        let Node::Lst(items) = self.arena.node(list).clone() else {
            return Err("Do expects a list of expressions".into());
        };
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

    fn let_form(&mut self, fx: &mut FnCtx, args: &[NodeId], tail: bool) -> Result<(), String> {
        let [bindings, body] = *args else {
            return Err("malformed Let".into());
        };
        let Node::Rec(fields) = self.arena.node(bindings).clone() else {
            return Err("Let bindings must be a record".into());
        };
        fx.scopes.push(HashMap::new());
        for (k, v) in &fields {
            self.expr(fx, *v, false)?;
            let l = fx.local(ValType::I32);
            fx.op(I::LocalSet(l));
            fx.scopes.last_mut().unwrap().insert(k.clone(), l);
        }
        let r = self.expr(fx, body, tail);
        fx.scopes.pop();
        r
    }

    /// Each clause is a block: a failed test branches past the clause; a
    /// matched clause leaves its result and branches to the end. No clause
    /// matching traps (the interpreter raises "no Match clause" instead).
    fn match_form(&mut self, fx: &mut FnCtx, args: &[NodeId], tail: bool) -> Result<(), String> {
        let [scrut_form, clauses_form] = *args else {
            return Err("malformed Match".into());
        };
        let Node::Lst(clauses) = self.arena.node(clauses_form).clone() else {
            return Err("Match expects a list of (pattern result) clauses".into());
        };
        self.expr(fx, scrut_form, false)?;
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
            Node::Int(_) | Node::Dec(_) | Node::Bool(_) | Node::Str(_) => {
                fx.op(I::LocalGet(v));
                self.expr(fx, pat, false)?;
                fx.op(I::Call(self.h.eq_raw));
                fx.op(I::I32Eqz);
                fx.op(I::BrIf(fail));
                Ok(())
            }
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

    /// Mirror of the interpreter's §4.2 argument-binding rule, at compile time.
    /// `args` are the call's argument forms (`Tup[head, …args]`).
    fn bind_args(&self, args: &[NodeId], params: &[String]) -> Result<BoundArgs, String> {
        // named: a single record arg whose keys are exactly the parameters
        if let [only] = args {
            if let Node::Rec(fields) = self.arena.node(*only) {
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

    fn internal_call(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        args: &[NodeId],
        tail: bool,
    ) -> Result<(), String> {
        let (idx, params) = self.funcs[name].clone();
        match self.bind_args(args, &params)? {
            BoundArgs::PerParam(nodes) => {
                for a in nodes {
                    self.expr(fx, a, false)?;
                }
            }
            BoundArgs::Bundle => self.seq_box(fx, args, TAG_TUP)?,
        }
        fx.op(if tail { I::ReturnCall(idx) } else { I::Call(idx) });
        Ok(())
    }

    fn dep_call(
        &mut self,
        fx: &mut FnCtx,
        alias: &str,
        fname: &str,
        args: &[NodeId],
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
        // Resolve freestanding names directly, and resource operations
        // (`[method]`/`[static]`/`[constructor]`/`[resource-drop]`) by their
        // bare op name.
        let sig = resolve_dep_func(dep, &iface, fname)?.clone();
        let module = versioned_iface(&dep.package, &iface);
        // The host import is keyed by the *mangled* WIT name (`sig.name`), which
        // is what the import-signature loop declares and what `wit-component`
        // re-validates against the WIT.
        let fidx = self.import_idx(&module, &sig.name);

        let param_names: Vec<String> = sig.params.iter().map(|(n, _)| n.clone()).collect();
        let arg_nodes = match self.bind_args(args, &param_names)? {
            BoundArgs::PerParam(nodes) => nodes,
            BoundArgs::Bundle => {
                return Err(format!(
                    "imported `{alias}/{fname}`: bundling multiple arguments into a single \
                     tuple parameter is not supported by the wasm backend"
                ));
            }
        };
        for (a, (_, t)) in arg_nodes.iter().zip(&sig.params) {
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
                if matches!(
                    rty,
                    WitTy::Record(_) | WitTy::Tuple(_) | WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_)
                ) {
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
            WitTy::Char | WitTy::IntS | WitTy::IntU | WitTy::Handle => {
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
                // flags record box → bitset i32: OR `1<<i` for each set flag.
                let b = fx.local(ValType::I32);
                let acc = fx.local(ValType::I32);
                fx.op(I::LocalSet(b));
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(acc));
                for (i, name) in names.iter().enumerate() {
                    let kaddr = self.intern_str(name);
                    fx.op(I::LocalGet(acc));
                    fx.op(I::LocalGet(b));
                    fx.op(I::I32Const(kaddr as i32));
                    fx.op(I::Call(self.h.rec_get));
                    fx.op(I::Call(self.h.truthy));
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
            WitTy::Char | WitTy::IntU | WitTy::Handle => {
                fx.op(I::I64ExtendI32U);
                fx.op(I::Call(self.h.box_int));
            }
            WitTy::S64 => fx.op(I::Call(self.h.box_int)),
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

    /// bitset in local `v` → record box `{name: bool …}`, one field per flag set
    /// to `(v >> i) & 1`.
    fn lift_flags(&mut self, fx: &mut FnCtx, v: u32, names: &[String]) {
        let n = names.len();
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
        for (i, name) in names.iter().enumerate() {
            let kaddr = self.intern_str(name);
            fx.op(I::LocalGet(p));
            fx.op(I::I32Const(kaddr as i32));
            fx.op(I::I32Store(ma(8 + 8 * i as u64, 2)));
            fx.op(I::LocalGet(p));
            fx.op(I::LocalGet(v));
            fx.op(I::I32Const(i as i32));
            fx.op(I::I32ShrU);
            fx.op(I::I32Const(1));
            fx.op(I::I32And);
            fx.op(I::Call(self.h.box_bool));
            fx.op(I::I32Store(ma(12 + 8 * i as u64, 2)));
        }
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
                let needs_narrowing =
                    pflat.iter().zip(joined).any(|(have, want)| have != want);
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
            WitTy::Char | WitTy::IntS | WitTy::IntU | WitTy::Handle => {
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
                // OR the set flags into a bitset word, then store it
                fx.op(I::LocalGet(dst));
                fx.op(I::LocalGet(src));
                self.lower(fx, &WitTy::Flags(names.clone()))?;
                fx.op(I::I32Store(ma(off, 2)));
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
            WitTy::IntS => {
                fx.op(I::LocalGet(src));
                fx.op(I::I32Load(ma(off, 2)));
                fx.op(I::I64ExtendI32S);
                fx.op(I::Call(self.h.box_int));
            }
            WitTy::Char | WitTy::IntU | WitTy::Handle => {
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

    fn builtin(&mut self, fx: &mut FnCtx, name: &str, args: &[NodeId]) -> Result<(), String> {
        let items = args;
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
                // cmp_raw yields -1/0/1 over ints, decs and strings (chars ride
                // in int boxes), matching the interpreter's `compare`.
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
            "tail" => {
                nargs(1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.tail_h));
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
            "some" | "ok" | "err" => {
                // the argument(s) bundle into the variant payload, exactly as
                // the interpreter binds it
                return self.var_box(fx, name, args);
            }
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
                        return Err(
                            "`expand` is only available inside a macro library \
                             (a file whose top level is DefMacros)"
                                .into(),
                        );
                    }
                }
            }
            other => return Err(format!("builtin `{other}` not supported by the wasm backend yet")),
        }
        Ok(())
    }
}

const BUILTINS: &[&str] = &[
    "eq", "not", "lt", "le", "gt", "ge", "add", "sub", "mul", "div", "rem", "neg", "len",
    "head", "tail", "str-cat", "upper", "lower", "to-string",
    "some", "ok", "err",
    // compile-time form machinery (macro bodies)
    "form-kind", "rec-key", "rec-val", "gensym", "expand",
];

// --------------------------------------------------------- helper bodies

fn emit_core_module(
    arena: &Arena,
    roots: &[NodeId],
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> Result<Vec<u8>, String> {
    let feats = features_of(arena, info);

    // named types in scope: this file's own DefTypes, plus those of every dep.
    // Records resolve via `records`; enum/variant/flags (dep-only today) via
    // `defs`.
    let mut type_env = TypeEnv::default();
    for (name, fields) in record_types(arena, &info.types) {
        type_env.records.insert(name, fields);
    }
    for dep in deps.values() {
        for (name, fields) in &dep.types {
            type_env
                .records
                .entry(name.clone())
                .or_insert_with(|| fields.clone());
        }
        for (name, def) in &dep.type_defs {
            type_env
                .defs
                .entry(name.clone())
                .or_insert_with(|| def.clone());
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
            tail_h: 0,
            strcat2: 0,
            case_h: 0,
            to_str: 0,
            rec_get: 0,
            as_f64: 0,
            arith_raw: 0,
            cmp_raw: 0,
            neg_raw: 0,
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
        macro_expand_idx: None,
    };

    // static boxes: false @16, true @24
    em.false_addr = DATA_BASE;
    em.put_i32(TAG_BOOL);
    em.put_i32(0);
    em.true_addr = DATA_BASE + 8;
    em.put_i32(TAG_BOOL);
    em.put_i32(1);

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
    let _ = (I32, I64, F64);
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
        // Same op-name resolution as `dep_call`, so a resource operation's
        // core import is declared under its mangled WIT name (`sig.name`).
        let sig = resolve_dep_func(dep, &iface, fname)?;
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
        // Declare the import once per mangled name; a method shared across ops
        // (none today) would otherwise be added twice.
        if !em
            .import_fn
            .contains_key(&(module.clone(), sig.name.clone()))
        {
            add_import(&mut em, &module, &sig.name, p, r);
        }
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
    em.h.tail_h = take();
    em.h.strcat2 = take();
    em.h.case_h = take();
    em.h.to_str = take();
    em.h.rec_get = take();
    em.h.as_f64 = take();
    em.h.arith_raw = take();
    em.h.cmp_raw = take();
    em.h.neg_raw = take();

    // An *exported overload set* (≥2 same-named `Def Fn`s, or a curated-op name)
    // is lowered by `wit::collect` to one mangled WIT export per member
    // (`eq-point`, `eq-string`, …), recorded in `info.overload_bodies` as
    // mangled-name -> (params, body). The underlying `Def`s share one original
    // name (`eq`) which collapses last-wins in `info.defs`, so the export
    // wrappers — which look bodies up by the *mangled* name — would otherwise
    // find nothing (`export `eq-point` has no Def Fn`). Register and emit one
    // internal function per mangled member instead, keyed on identity. Skip the
    // original collapsed name in the normal pass below so we don't emit a bogus,
    // unreferenced `eq` (and avoid any clash). Keep `info.fn_defs`/overload sets
    // intact — internal-call resolution and type-checking land in a later step.
    let mut overload_order: Vec<String> = info.overload_bodies.keys().cloned().collect();
    overload_order.sort(); // deterministic index assignment
    // Original def names whose every member was consumed by overload mangling.
    let overloaded_origins: std::collections::HashSet<&String> = info
        .fn_defs
        .iter()
        .filter(|(_, members)| {
            members
                .iter()
                .all(|m| info.overload_bodies.values().any(|ob| ob == m))
        })
        .map(|(name, _)| name)
        .collect();

    // ---- assign internal function indices (file order)
    let mut internal_order: Vec<String> = Vec::new();
    for &root in roots {
        if let Node::Tup(items) = arena.node(root) {
            if items.len() >= 2
                && matches!(arena.node(items[0]), Node::Sym(s) if s == "def-MACRO")
            {
                if let Node::Sym(name) = arena.node(items[1]) {
                    if info.defs.contains_key(name)
                        && !overloaded_origins.contains(name)
                        && !internal_order.contains(name)
                    {
                        internal_order.push(name.clone());
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
    // Mangled overload members get their own internal-function indices.
    for mangled in &overload_order {
        let (params_id, _) = info.overload_bodies[mangled];
        let params = param_names(arena, params_id)?;
        em.funcs.insert(mangled.clone(), (take(), params));
    }

    // ---- helper bodies (order must match index assignment above)
    emit_helpers(&mut em)?;

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
    // ---- mangled overload member bodies (same paired order as their indices)
    for mangled in &overload_order {
        let (_, body) = info.overload_bodies[mangled];
        let params = em.funcs[mangled].1.clone();
        let n = params.len();
        let mut fx = FnCtx::new(n as u32);
        let mut scope = HashMap::new();
        for (i, p) in params.iter().enumerate() {
            scope.insert(p.clone(), i as u32);
        }
        fx.scopes.push(scope);
        em.expr(&mut fx, body, true)
            .map_err(|e| format!("in `{mangled}`: {e}"))?;
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
                if matches!(
                    ty,
                    WitTy::Record(_) | WitTy::Tuple(_) | WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_)
                ) {
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
        // An external interface (wasi:http/incoming-handler, wasi:cli/run) is
        // exported under its own versioned name — at the version of its resolved
        // `wit/deps` package on the generic path, or the vendored WASI version
        // for the magic path; a local one lands in this package.
        let own_iface = if is_external_iface(&sig.iface) {
            external_versioned_in(&sig.iface, deps)
        } else {
            versioned_iface(&info.package, &sig.iface)
        };
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
    // The `gensym` counter (always present): an i64 incremented once per
    // `gensym` call, so fresh symbols are unique and deterministic across every
    // expansion in one component instance. Index = 1 + value_defs.len() (global
    // 0 is the heap pointer, then one i32 per value def).
    gs.global(
        GlobalType { val_type: ValType::I64, mutable: true, shared: false },
        &ConstExpr::i64_const(0),
    );
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

// ------------------------------------------------- functor `set` resource bodies
//
// Step 02 (see `dev-notes/functor/plan/02-rep-and-bodies.typ` and the verified
// ABI in `summaries/01-abi.typ`): emit the core wasm functions that implement a
// `set` resource for ONE instantiation at element type `elem`. This is the
// "guest implements an exported resource" case; the bodies mirror the
// interpreter's `Value::Cell(Rc<RefCell<Value::Lst>>)` set, with structural
// `eq_raw` membership (the project's one hard rule).
//
// REP LAYOUT (mirrors the interpreter):
//   * A `set` rep is a pointer to a one-word mutable CELL: `[i32 list-ptr]`.
//     The mutable cell gives the resource a stable identity so a later
//     `contains`/`size` observes earlier `add`s — exactly `RefCell` semantics.
//   * The cell's word points at the existing boxed-list layout
//     `[TAG_LIST, len, elem-ptr…]` (TAG_LIST=3; `len` is the i32 word @4).
//   * Elements are stored as boxed values (the same heap boxes the rest of the
//     backend uses), so `eq_raw`/list iteration operate uniformly across any
//     element type (record / string / primitive).
//
// ABI (from summary 01, the THING TO GET RIGHT):
//   * constructor `() -> i32`: mint an OWN handle with `resource.new(cell)`.
//   * every method's param 0 (`self`, a `borrow`) arrives as the REP DIRECTLY —
//     i.e. the cell ptr we passed to `resource.new`. Do NOT call `resource.rep`
//     on it (that traps "unknown handle index"; `resource.rep` is for the
//     opposite direction). Use param 0 as the cell ptr verbatim.
//   * dtor `(i32 rep) -> ()`: safe no-op (bump allocator never frees).
//   * `contains`/`size` return a bare core i32 (the encoder's `canon lift` does
//     i32→bool / i32→u32), so no value-`lower` is needed on the result.

/// The five core functions implementing one `set` instantiation, by core
/// function index, plus the resource-intrinsic import indices their bodies
/// reference. Step 03 wires these into the export/import sections.
#[derive(Clone, Copy, Debug)]
pub struct ResourceFns {
    /// `[constructor]set` — core sig `() -> i32` (returns an OWN handle).
    pub ctor: u32,
    /// `[method]set.add` — core sig `(i32 self, <flat elem>) -> ()`.
    pub add: u32,
    /// `[method]set.contains` — core sig `(i32 self, <flat elem>) -> i32` (0/1).
    pub contains: u32,
    /// `[method]set.size` — core sig `(i32 self) -> i32` (u32 count).
    pub size: u32,
    /// `[dtor]set` — core sig `(i32 rep) -> ()` (no-op).
    pub dtor: u32,
    /// import idx of `[resource-new]set` `(i32 rep) -> i32 handle` (ctor uses it).
    pub new_import: u32,
    /// import idx of `[resource-rep]set` `(i32 handle) -> i32 rep`. The bodies do
    /// NOT call this (methods already receive the rep), but it is declared and
    /// carried so step 03 can wire the intrinsic table the encoder expects.
    pub rep_import: u32,
    /// import idx of `[resource-drop]set` `(i32 handle) -> ()`. Unused by the
    /// bodies; carried for the intrinsic table.
    pub drop_import: u32,
}

/// Build an empty boxed list `[TAG_LIST, 0]` (8 bytes), leaving its ptr on the
/// stack.
fn emit_empty_list_box(em: &mut Emitter, fx: &mut FnCtx) {
    let p = fx.local(ValType::I32);
    fx.op(I::I32Const(8));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(p));
    fx.op(I::LocalGet(p));
    fx.op(I::I32Const(TAG_LIST));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(p));
    fx.op(I::I32Const(0));
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::LocalGet(p));
}

/// Linear-scan the boxed list in local `list` for a box structurally-equal (via
/// `eq_raw`) to the box in local `needle`. Leaves an i32 0/1 on the stack: 1 if
/// present, else 0. Allocates two fresh i32 locals (`i`, `n`) internally.
fn emit_list_contains(em: &mut Emitter, fx: &mut FnCtx, list: u32, needle: u32) {
    let i = fx.local(ValType::I32);
    let n = fx.local(ValType::I32);
    // n = list.len  (@4)
    fx.op(I::LocalGet(list));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(n));
    // i = 0
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(i));
    // result accumulated as a block that returns i32: default 0, early-return 1
    fx.op(I::Block(BlockType::Result(ValType::I32)));
    fx.op(I::Loop(BlockType::Empty));
    // if i >= n: break out with 0
    fx.op(I::LocalGet(i));
    fx.op(I::LocalGet(n));
    fx.op(I::I32GeU);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::I32Const(0));
    fx.op(I::Br(2)); // br to the result block, yielding 0
    fx.op(I::End);
    // elem = list[8 + 4*i]
    fx.op(I::LocalGet(list));
    fx.op(I::LocalGet(i));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(8, 2)));
    // eq_raw(elem, needle)
    fx.op(I::LocalGet(needle));
    fx.op(I::Call(em.h.eq_raw));
    fx.op(I::If(BlockType::Empty));
    fx.op(I::I32Const(1));
    fx.op(I::Br(2)); // present → result block yields 1
    fx.op(I::End);
    // i += 1; continue
    fx.op(I::LocalGet(i));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(i));
    fx.op(I::Br(0)); // loop
    fx.op(I::End); // loop
    // unreachable fallthrough: the loop only exits via Br(2)
    fx.op(I::I32Const(0));
    fx.op(I::End); // block → i32 on stack
}

/// Build a NEW boxed list whose elements are the old list's elements followed by
/// the box in local `extra`. Leaves the new list-box ptr on the stack. Allocates
/// fresh i32 locals internally.
fn emit_list_append(em: &mut Emitter, fx: &mut FnCtx, old: u32, extra: u32) {
    let n = fx.local(ValType::I32); // old length
    let new = fx.local(ValType::I32); // new list ptr
    let i = fx.local(ValType::I32); // copy cursor
    // n = old.len (@4)
    fx.op(I::LocalGet(old));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(n));
    // new = alloc(8 + 4*(n+1))
    fx.op(I::I32Const(8 + 4));
    fx.op(I::LocalGet(n));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(new));
    // new.tag = TAG_LIST
    fx.op(I::LocalGet(new));
    fx.op(I::I32Const(TAG_LIST));
    fx.op(I::I32Store(ma(0, 2)));
    // new.len = n + 1
    fx.op(I::LocalGet(new));
    fx.op(I::LocalGet(n));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::I32Store(ma(4, 2)));
    // copy old elems: for i in 0..n: new[8+4i] = old[8+4i]
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(i));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(i));
    fx.op(I::LocalGet(n));
    fx.op(I::I32GeU);
    fx.op(I::BrIf(1)); // exit copy loop
    // new[8 + 4*i] = old[8 + 4*i]
    fx.op(I::LocalGet(new));
    fx.op(I::LocalGet(i));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::LocalGet(old));
    fx.op(I::LocalGet(i));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(8, 2)));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(i));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(i));
    fx.op(I::Br(0));
    fx.op(I::End); // loop
    fx.op(I::End); // block
    // new[8 + 4*n] = extra
    fx.op(I::LocalGet(new));
    fx.op(I::LocalGet(n));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::LocalGet(extra));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(new));
}

/// Emit the five `set` bodies for one instantiation at element type `elem`,
/// appending them to `em.bodies` and returning their indices in [`ResourceFns`].
///
/// The three resource-intrinsic import indices (`[resource-new/rep/drop]set`)
/// are passed in: imports must be declared before any helper/body index is
/// assigned (function index space is imports-first), so step 03 declares them in
/// the up-front import loop and hands the indices here. This keeps the function
/// purely additive over `em.bodies` and side-effect-free on the import section.
///
/// `inst` is accepted for symmetry / future per-instantiation specialisation
/// (e.g. distinct rep tags) but is not needed by the current bodies; the only
/// thing that varies is `elem`, which drives the flat param shape and boxing.
fn emit_set_resource(
    em: &mut Emitter,
    _inst: &crate::wit::FunctorInst,
    elem: &WitTy,
    new_import: u32,
    rep_import: u32,
    drop_import: u32,
) -> Result<ResourceFns, String> {
    use ValType::I32;
    // Flat core params of the element value (after `self`), in canonical-ABI flat
    // order. `flat_checked` rejects element types the backend can't flatten.
    let elem_flat = flat_checked(elem)?;
    let n_elem = elem_flat.len() as u32;

    // The function index of the next body we push. Bodies are emitted later (in
    // the same order) at `n_imports + position`; the caller assigns that base.
    let body_base = em.imports.len() as u32;
    let mut next_idx = body_base + em.bodies.len() as u32;
    let mut alloc_idx = || {
        let i = next_idx;
        next_idx += 1;
        i
    };

    // ---- constructor: () -> i32 (own handle)
    let ctor = alloc_idx();
    {
        let mut fx = FnCtx::new(0);
        let cell = fx.local(I32);
        // cell = alloc(4)
        fx.op(I::I32Const(4));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(cell));
        // cell[0] = empty list box
        fx.op(I::LocalGet(cell));
        emit_empty_list_box(em, &mut fx);
        fx.op(I::I32Store(ma(0, 2)));
        // resource.new(cell) -> handle ; return it
        fx.op(I::LocalGet(cell));
        fx.op(I::Call(new_import));
        let t = em.ty_idx(vec![], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // ---- add: (i32 self, <flat elem>) -> ()
    //   self IS the cell ptr (see ABI note). value flats start at local 1.
    let add = alloc_idx();
    {
        let mut params = vec![I32];
        params.extend_from_slice(&elem_flat);
        let mut fx = FnCtx::new(params.len() as u32);
        let list = fx.local(I32);
        let needle = fx.local(I32);
        // list = *self  (the cell's word)
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(list));
        // needle = box the flattened incoming value (param locals 1..1+n_elem)
        em.lift_flat(&mut fx, elem, 1)?;
        fx.op(I::LocalSet(needle));
        // if present → return (dedup-on-add by Value equality)
        emit_list_contains(em, &mut fx, list, needle);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Return);
        fx.op(I::End);
        // *self = old list + needle
        fx.op(I::LocalGet(0));
        emit_list_append(em, &mut fx, list, needle);
        fx.op(I::I32Store(ma(0, 2)));
        let _ = n_elem; // silence if elem flattens to zero (no such WitTy today)
        let t = em.ty_idx(params, vec![]);
        em.bodies.push((t, fx.finish()));
    }

    // ---- contains: (i32 self, <flat elem>) -> i32 (0/1)
    let contains = alloc_idx();
    {
        let mut params = vec![I32];
        params.extend_from_slice(&elem_flat);
        let mut fx = FnCtx::new(params.len() as u32);
        let list = fx.local(I32);
        let needle = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(list));
        em.lift_flat(&mut fx, elem, 1)?;
        fx.op(I::LocalSet(needle));
        emit_list_contains(em, &mut fx, list, needle); // i32 0/1 on stack
        let t = em.ty_idx(params, vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // ---- size: (i32 self) -> i32 (u32 element count)
    let size = alloc_idx();
    {
        let mut fx = FnCtx::new(1);
        // *self -> list ptr ; load len @4
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Load(ma(4, 2)));
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // ---- dtor: (i32 rep) -> ()  — safe no-op (bump allocator never frees)
    let dtor = alloc_idx();
    {
        let fx = FnCtx::new(1);
        let t = em.ty_idx(vec![I32], vec![]);
        em.bodies.push((t, fx.finish()));
    }

    Ok(ResourceFns {
        ctor,
        add,
        contains,
        size,
        dtor,
        new_import,
        rep_import,
        drop_import,
    })
}

// ----------------------------------------------- strategy-B macro component
//
// `emit_macro_core_module` builds the core wasm for a `wavelet:meta/macros`
// component whose `manifest`/`expand` are compiled (no interpreter in the
// guest). It mirrors `emit_core_module`'s assembly but is driven by a file's
// `DefMacro`s rather than its `Def`/`Export`s, and adds the compiled
// `tree`⇄`box` adapters the boundary needs.

fn emit_macro_core_module(arena: &Arena, roots: &[NodeId]) -> Result<Vec<u8>, String> {
    use ValType::I32;

    // Collect the file's DefMacros: `Tup[defmacro-MACRO, name, {params}, body]`.
    let mut macros: Vec<MacroDef> = Vec::new();
    for &root in roots {
        let Node::Tup(items) = arena.node(root) else { continue };
        if items.len() != 4 {
            continue;
        }
        let Node::Sym(h) = arena.node(items[0]) else { continue };
        if h != "defmacro-MACRO" {
            continue;
        }
        let Node::Sym(name) = arena.node(items[1]) else { continue };
        let params = param_names(arena, items[2])?;
        macros.push(MacroDef { name: name.clone(), params, body: items[3] });
    }

    // A minimal FileInfo: a macro library has no runtime defs/exports of its
    // own. `gensym` keys its counter global off `value_defs.len()` (here 0).
    let info = FileInfo {
        package: "wavelet:macro-guest@0.1.0".to_string(),
        package_path: "wavelet:macro-guest".to_string(),
        world: "macro-lib".to_string(),
        imports: Vec::new(),
        functors: Vec::new(),
        exports: Vec::new(),
        types: Vec::new(),
        defs: HashMap::new(),
        fn_defs: HashMap::new(),
        value_defs: Vec::new(),
        overload_bodies: HashMap::new(),
    };
    let deps: HashMap<String, Dep> = HashMap::new();

    let mut em = Emitter {
        arena,
        info: &info,
        deps: &deps,
        type_env: TypeEnv::default(),
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
            tail_h: 0,
            strcat2: 0,
            case_h: 0,
            to_str: 0,
            rec_get: 0,
            as_f64: 0,
            arith_raw: 0,
            cmp_raw: 0,
            neg_raw: 0,
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
        macro_expand_idx: None,
    };

    // static boxes: false @16, true @24
    em.false_addr = DATA_BASE;
    em.put_i32(TAG_BOOL);
    em.put_i32(0);
    em.true_addr = DATA_BASE + 8;
    em.put_i32(TAG_BOOL);
    em.put_i32(1);

    // helper indices (no imports, so function index space starts at 0)
    let mut next = 0u32;
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
    em.h.tail_h = take();
    em.h.strcat2 = take();
    em.h.case_h = take();
    em.h.to_str = take();
    em.h.rec_get = take();
    em.h.as_f64 = take();
    em.h.arith_raw = take();
    em.h.cmp_raw = take();
    em.h.neg_raw = take();
    emit_helpers(&mut em)?;

    // macro body functions (each compiles like a Fn over its param forms)
    for m in &macros {
        let idx = take();
        em.funcs.insert(m.name.clone(), (idx, m.params.clone()));
    }
    let tree_to_form_idx = take();
    let count_idx = take();
    let sym_node_idx = take();
    let fill_idx = take();
    let form_to_tree_idx = take();
    let manifest_idx = take();
    let expand_idx = take();
    let expand_step_idx = take();
    // Make the in-macro `expand` builtin available while compiling the bodies.
    em.macro_expand_idx = Some(expand_step_idx);

    // bodies, in the same order their indices were assigned
    for m in &macros {
        let body = mc_macro_body(&mut em, m)?;
        em.bodies.push(body);
    }
    let b = mc_tree_to_form(&mut em)?;
    em.bodies.push(b);
    let b = mc_count_nodes(&mut em, count_idx)?;
    em.bodies.push(b);
    let b = mc_sym_node(&mut em)?;
    em.bodies.push(b);
    let b = mc_fill(&mut em, fill_idx, sym_node_idx)?;
    em.bodies.push(b);
    let b = mc_form_to_tree(&mut em, count_idx, fill_idx)?;
    em.bodies.push(b);
    let b = mc_manifest(&mut em, &macros)?;
    em.bodies.push(b);
    let b = mc_expand(&mut em, &macros, tree_to_form_idx, form_to_tree_idx)?;
    em.bodies.push(b);
    let b = mc_expand_step(&mut em, &macros)?;
    em.bodies.push(b);

    let exports: Vec<(String, u32)> = vec![
        ("wavelet:meta/macros@0.1.0#manifest".to_string(), manifest_idx),
        ("wavelet:meta/macros@0.1.0#expand".to_string(), expand_idx),
    ];

    // ---- assemble (no imports, no deps)
    let heap_base = {
        em.align8();
        DATA_BASE + em.data.len() as u32
    };
    let pages = (heap_base as u64 >> 16) + 1;
    let closure_base = em.bodies.len() as u32;

    let mut module = Module::new();
    let mut ts = TypeSection::new();
    for (p, r) in &em.types {
        ts.ty().function(p.iter().copied(), r.iter().copied());
    }
    module.section(&ts);

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
    // gensym counter (index 1, since there are no value defs)
    gs.global(
        GlobalType { val_type: ValType::I64, mutable: true, shared: false },
        &ConstExpr::i64_const(0),
    );
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
        els.active(Some(0), &ConstExpr::i32_const(0), Elements::Functions(idxs.into()));
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

/// Compile a macro body to an internal `(box…) -> box` function: its parameters
/// bind to the argument *forms* (as boxes), exactly as `expand_once` binds them.
fn mc_macro_body(em: &mut Emitter, m: &MacroDef) -> Result<(u32, Function), String> {
    use ValType::I32;
    let n = m.params.len();
    let mut fx = FnCtx::new(n as u32);
    let mut scope = HashMap::new();
    for (i, p) in m.params.iter().enumerate() {
        scope.insert(p.clone(), i as u32);
    }
    fx.scopes.push(scope);
    em.expr(&mut fx, m.body, false)
        .map_err(|e| format!("in macro `{}`: {e}", m.name))?;
    let t = em.ty_idx(vec![I32; n], vec![I32]);
    Ok((t, fx.finish()))
}

/// `tree → box`: convert a lifted wire `tree` record box into the root form box
/// (the compile-time analogue of `meta::tree_to_arena` + `form_to_value`). Walks
/// the node table building a per-node index of form boxes; children precede
/// parents (`meta::arena_to_tree` guarantees it), so a `tup`/`lst`/`rec` node's
/// child ids are already built.
fn mc_tree_to_form(em: &mut Emitter) -> Result<(u32, Function), String> {
    use ValType::I32;
    let mut fx = FnCtx::new(1);
    let args = 0u32;
    let nodes = fx.local(I32);
    let n = fx.local(I32);
    let root_id = fx.local(I32);
    let idx = fx.local(I32);
    let k = fx.local(I32);
    let nodevar = fx.local(I32);
    let case = fx.local(I32);
    let payload = fx.local(I32);
    let formbox = fx.local(I32);
    let cs = fx.local(I32);
    let m = fx.local(I32);
    let e = fx.local(I32);
    let cid = fx.local(I32);
    let out = fx.local(I32);
    let tup = fx.local(I32);

    let nodes_key = em.intern_str("nodes") as i32;
    let root_key = em.intern_str("root") as i32;
    fx.op(I::LocalGet(args));
    fx.op(I::I32Const(nodes_key));
    fx.op(I::Call(em.h.rec_get));
    fx.op(I::LocalSet(nodes));
    fx.op(I::LocalGet(nodes));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(n));
    fx.op(I::LocalGet(args));
    fx.op(I::I32Const(root_key));
    fx.op(I::Call(em.h.rec_get));
    fx.op(I::Call(em.h.unbox_int));
    fx.op(I::I32WrapI64);
    fx.op(I::LocalSet(root_id));
    // idx = alloc((n+1)*4)  (+1 so n==0 never asks for a zero-byte block)
    fx.op(I::LocalGet(n));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(idx));

    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(k));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(k));
    fx.op(I::LocalGet(n));
    fx.op(I::I32GeU);
    fx.op(I::BrIf(1));
    // nodevar = nodes[k]
    fx.op(I::LocalGet(nodes));
    fx.op(I::LocalGet(k));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(8, 2)));
    fx.op(I::LocalSet(nodevar));
    fx.op(I::LocalGet(nodevar));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(case));
    fx.op(I::LocalGet(nodevar));
    fx.op(I::I32Load(ma(8, 2)));
    fx.op(I::LocalSet(payload));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(formbox));

    // scalar cases pass the payload box straight through (a bool/int/dec/str
    // wire payload is already exactly the form box).
    for c in ["bool-val", "int-val", "dec-val", "str-val"] {
        let s = em.intern_str(c) as i32;
        fx.op(I::LocalGet(case));
        fx.op(I::I32Const(s));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(payload));
        fx.op(I::LocalSet(formbox));
        fx.op(I::End);
    }
    // char-val: the wire payload is a `char` lifted as an int box; rebuild it as
    // a distinct TAG_CHAR form box so it round-trips as a char (not an int).
    {
        let s = em.intern_str("char-val") as i32;
        fx.op(I::LocalGet(case));
        fx.op(I::I32Const(s));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(payload));
        fx.op(I::I64Load(ma(8, 3)));
        em.box_char(&mut fx);
        fx.op(I::LocalSet(formbox));
        fx.op(I::End);
    }
    // sym → payload-less TAG_VAR [case=str payload-box, 0]
    {
        let s = em.intern_str("sym") as i32;
        fx.op(I::LocalGet(case));
        fx.op(I::I32Const(s));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(12));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(formbox));
        fx.op(I::LocalGet(formbox));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(formbox));
        fx.op(I::LocalGet(payload));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(formbox));
        fx.op(I::I32Const(0));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::End);
    }
    // qsym → payload-less TAG_VAR whose case is "alias/name"
    {
        let s = em.intern_str("qsym") as i32;
        let slash = em.intern_str("/") as i32;
        fx.op(I::LocalGet(case));
        fx.op(I::I32Const(s));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(payload));
        fx.op(I::I32Load(ma(8, 2))); // alias str
        fx.op(I::I32Const(slash));
        fx.op(I::Call(em.h.strcat2));
        fx.op(I::LocalGet(payload));
        fx.op(I::I32Load(ma(12, 2))); // name str
        fx.op(I::Call(em.h.strcat2));
        fx.op(I::LocalSet(cs));
        fx.op(I::I32Const(12));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(formbox));
        fx.op(I::LocalGet(formbox));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(formbox));
        fx.op(I::LocalGet(cs));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(formbox));
        fx.op(I::I32Const(0));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::End);
    }
    // tup / lst → sequence form box of child forms (by id)
    for (c, tag) in [("tup", TAG_TUP), ("lst", TAG_LIST)] {
        let s = em.intern_str(c) as i32;
        fx.op(I::LocalGet(case));
        fx.op(I::I32Const(s));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        // m = payload.len
        fx.op(I::LocalGet(payload));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(m));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(out));
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(tag));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(e));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(e));
        fx.op(I::LocalGet(m));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // cid = unbox(payload[e])
        fx.op(I::LocalGet(payload));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::Call(em.h.unbox_int));
        fx.op(I::I32WrapI64);
        fx.op(I::LocalSet(cid));
        // out[e] = idx[cid]
        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(idx));
        fx.op(I::LocalGet(cid));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(e));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(out));
        fx.op(I::LocalSet(formbox));
        fx.op(I::End);
    }
    // rec → record form box of child value forms (keys carried through)
    {
        let s = em.intern_str("rec") as i32;
        fx.op(I::LocalGet(case));
        fx.op(I::I32Const(s));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(payload));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(m));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(out));
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(TAG_REC));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(e));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(e));
        fx.op(I::LocalGet(m));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // tup = payload[e]  (a TAG_TUP [2, key-str, id-box])
        fx.op(I::LocalGet(payload));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalSet(tup));
        // key → out key slot (8 + 8e)
        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(tup));
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::I32Store(ma(8, 2)));
        // cid = unbox(tup[1]) ; val = idx[cid] → out val slot (12 + 8e)
        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(tup));
        fx.op(I::I32Load(ma(12, 2)));
        fx.op(I::Call(em.h.unbox_int));
        fx.op(I::I32WrapI64);
        fx.op(I::LocalSet(cid));
        fx.op(I::LocalGet(idx));
        fx.op(I::LocalGet(cid));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Store(ma(12, 2)));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(e));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(out));
        fx.op(I::LocalSet(formbox));
        fx.op(I::End);
    }
    // flg → flags form box [TAG_FLG, m, name str boxes…] (payload is a
    // list<string> of names, copied straight through)
    {
        let s = em.intern_str("flg") as i32;
        fx.op(I::LocalGet(case));
        fx.op(I::I32Const(s));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(payload));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(m));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(out));
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(TAG_FLG));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(e));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(e));
        fx.op(I::LocalGet(m));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(payload));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(e));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(out));
        fx.op(I::LocalSet(formbox));
        fx.op(I::End);
    }
    // unmatched → trap
    fx.op(I::LocalGet(formbox));
    fx.op(I::I32Eqz);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::Unreachable);
    fx.op(I::End);
    // idx[k] = formbox
    fx.op(I::LocalGet(idx));
    fx.op(I::LocalGet(k));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::LocalGet(formbox));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(k));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(k));
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
    // return idx[root_id]
    fx.op(I::LocalGet(idx));
    fx.op(I::LocalGet(root_id));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(0, 2)));
    let t = em.ty_idx(vec![I32], vec![I32]);
    Ok((t, fx.finish()))
}

/// Count the nodes a form contributes to a wire `tree` (one per node, plus its
/// children). Must agree with `mc_fill`'s assignment exactly.
fn mc_count_nodes(em: &mut Emitter, count_idx: u32) -> Result<(u32, Function), String> {
    use ValType::I32;
    let mut fx = FnCtx::new(1);
    let form = 0u32;
    let tag = fx.local(I32);
    let total = fx.local(I32);
    let m = fx.local(I32);
    let e = fx.local(I32);
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(0, 2)));
    fx.op(I::LocalSet(tag));
    fx.op(I::I32Const(1));
    fx.op(I::LocalSet(total));
    // TAG_FN, or a payloaded TAG_VAR, cannot appear in code → trap (kept in sync
    // with mc_fill).
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_FN));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::Unreachable);
    fx.op(I::End);
    // A payloaded TAG_VAR `name(p)` becomes a `tup[sym(name), p]` (2 nodes plus
    // the payload's subtree); a payload-less one is a single sym/qsym node.
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_VAR));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(8, 2)));
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(total));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(8, 2)));
    fx.op(I::Call(count_idx));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(total));
    fx.op(I::End);
    fx.op(I::End);
    // tup / lst: children at [form + 8 + 4e]
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_TUP));
    fx.op(I::I32Eq);
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_LIST));
    fx.op(I::I32Eq);
    fx.op(I::I32Or);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(m));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(e));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(e));
    fx.op(I::LocalGet(m));
    fx.op(I::I32GeU);
    fx.op(I::BrIf(1));
    fx.op(I::LocalGet(total));
    fx.op(I::LocalGet(form));
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(8, 2)));
    fx.op(I::Call(count_idx));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(total));
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(e));
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::End);
    // rec: values at [form + 12 + 8e]
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_REC));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(m));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(e));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(e));
    fx.op(I::LocalGet(m));
    fx.op(I::I32GeU);
    fx.op(I::BrIf(1));
    fx.op(I::LocalGet(total));
    fx.op(I::LocalGet(form));
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(8));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(12, 2)));
    fx.op(I::Call(count_idx));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(total));
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(e));
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::LocalGet(total));
    let t = em.ty_idx(vec![I32], vec![I32]);
    Ok((t, fx.finish()))
}

/// Copy `sublen` bytes from `src[8 + start ..]` into a fresh `[TAG_STR, sublen,
/// bytes…]` box left in `out`. `start`/`sublen` are locals; `j` is a scratch
/// loop local.
fn emit_substr(em: &mut Emitter, fx: &mut FnCtx, src: u32, start: u32, sublen: u32, out: u32, j: u32) {
    fx.op(I::LocalGet(sublen));
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(out));
    fx.op(I::LocalGet(out));
    fx.op(I::I32Const(TAG_STR));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(out));
    fx.op(I::LocalGet(sublen));
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(j));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(j));
    fx.op(I::LocalGet(sublen));
    fx.op(I::I32GeU);
    fx.op(I::BrIf(1));
    // dst = out + 8 + j
    fx.op(I::LocalGet(out));
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(j));
    fx.op(I::I32Add);
    // byte = src[8 + start + j]
    fx.op(I::LocalGet(src));
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(start));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(j));
    fx.op(I::I32Add);
    fx.op(I::I32Load8U(ma(0, 0)));
    fx.op(I::I32Store8(ma(0, 0)));
    fx.op(I::LocalGet(j));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(j));
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
}

/// Build the wire node box for a symbol whose name is the string box `case`:
/// a `sym(name)` node, or — when the name contains a `/` — a `qsym((alias,
/// name))` node (mirroring `value::sym_node`, which splits a `Variant` case on
/// `/`). Signature `(case-str) -> node-box`.
fn mc_sym_node(em: &mut Emitter) -> Result<(u32, Function), String> {
    use ValType::I32;
    let mut fx = FnCtx::new(1);
    let case = 0u32;
    let len = fx.local(I32);
    let i = fx.local(I32);
    let slash = fx.local(I32);
    let node = fx.local(I32);
    let alias = fx.local(I32);
    let name = fx.local(I32);
    let tup = fx.local(I32);
    let start = fx.local(I32);
    let sublen = fx.local(I32);
    let j = fx.local(I32);

    fx.op(I::LocalGet(case));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(len));
    fx.op(I::I32Const(-1));
    fx.op(I::LocalSet(slash));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(i));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(i));
    fx.op(I::LocalGet(len));
    fx.op(I::I32GeU);
    fx.op(I::BrIf(1));
    fx.op(I::LocalGet(case));
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(i));
    fx.op(I::I32Add);
    fx.op(I::I32Load8U(ma(0, 0)));
    fx.op(I::I32Const('/' as i32));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(i));
    fx.op(I::LocalSet(slash));
    fx.op(I::Br(2)); // first '/' found → exit the scan
    fx.op(I::End);
    fx.op(I::LocalGet(i));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(i));
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);

    let sym = em.intern_str("sym") as i32;
    let qsym = em.intern_str("qsym") as i32;
    fx.op(I::LocalGet(slash));
    fx.op(I::I32Const(-1));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Result(I32)));
    // sym(name): payload is the whole case string box
    fx.op(I::I32Const(12));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(node));
    fx.op(I::LocalGet(node));
    fx.op(I::I32Const(TAG_VAR));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(node));
    fx.op(I::I32Const(sym));
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::LocalGet(node));
    fx.op(I::LocalGet(case));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(node));
    fx.op(I::Else);
    // qsym((alias, name)): split at the slash
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(start));
    fx.op(I::LocalGet(slash));
    fx.op(I::LocalSet(sublen));
    emit_substr(em, &mut fx, case, start, sublen, alias, j);
    fx.op(I::LocalGet(slash));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(start));
    fx.op(I::LocalGet(len));
    fx.op(I::LocalGet(slash));
    fx.op(I::I32Sub);
    fx.op(I::I32Const(1));
    fx.op(I::I32Sub);
    fx.op(I::LocalSet(sublen));
    emit_substr(em, &mut fx, case, start, sublen, name, j);
    // tup = [TAG_TUP, 2, alias, name]
    fx.op(I::I32Const(16));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(tup));
    fx.op(I::LocalGet(tup));
    fx.op(I::I32Const(TAG_TUP));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(tup));
    fx.op(I::I32Const(2));
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::LocalGet(tup));
    fx.op(I::LocalGet(alias));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(tup));
    fx.op(I::LocalGet(name));
    fx.op(I::I32Store(ma(12, 2)));
    // node = [TAG_VAR, "qsym", tup]
    fx.op(I::I32Const(12));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(node));
    fx.op(I::LocalGet(node));
    fx.op(I::I32Const(TAG_VAR));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(node));
    fx.op(I::I32Const(qsym));
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::LocalGet(node));
    fx.op(I::LocalGet(tup));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(node));
    fx.op(I::End);
    let t = em.ty_idx(vec![I32], vec![I32]);
    Ok((t, fx.finish()))
}

/// `box → wire`, recursively: emit `form`'s subtree into `nodes` (a list box,
/// elements at +8) using a post-order id cursor (`cur`, a 4-byte cell), and
/// return this form's assigned node id. Children are emitted before parents.
fn mc_fill(em: &mut Emitter, fill_idx: u32, sym_node_idx: u32) -> Result<(u32, Function), String> {
    use ValType::I32;
    let mut fx = FnCtx::new(3);
    let form = 0u32;
    let nodes = 1u32;
    let cur = 2u32;
    let tag = fx.local(I32);
    let id = fx.local(I32);
    let headid = fx.local(I32);
    let pid = fx.local(I32);
    let m = fx.local(I32);
    let e = fx.local(I32);
    let plist = fx.local(I32);
    let node = fx.local(I32);
    let tupb = fx.local(I32);

    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(0, 2)));
    fx.op(I::LocalSet(tag));

    // helper to assign & advance the cursor into `id`
    let bump = |fx: &mut FnCtx| {
        fx.op(I::LocalGet(cur));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(id));
        fx.op(I::LocalGet(cur));
        fx.op(I::LocalGet(id));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::I32Store(ma(0, 2)));
    };

    // Build `node` = [TAG_VAR, case-str, payload] then store at nodes[id].
    // Scalars: bool/int/dec/str → wire case with the form box as payload.
    for (tagk, case) in [
        (TAG_BOOL, "bool-val"),
        (TAG_INT, "int-val"),
        (TAG_DEC, "dec-val"),
        (TAG_STR, "str-val"),
    ] {
        let caddr = em.intern_str(case) as i32;
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(tagk));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        bump(&mut fx);
        fx.op(I::I32Const(12));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(node));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(caddr));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::LocalGet(form));
        fx.op(I::I32Store(ma(8, 2)));
        store_node(&mut fx, nodes, id, node);
        fx.op(I::LocalGet(id));
        fx.op(I::Return);
        fx.op(I::End);
    }
    // char (TAG_CHAR): wire `char-val`, payload = the codepoint as an int box so
    // the boundary's `lower(char)` (= unbox_int) reads it.
    {
        let caddr = em.intern_str("char-val") as i32;
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_CHAR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        bump(&mut fx);
        fx.op(I::I32Const(12));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(node));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(caddr));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::LocalGet(form));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::Call(em.h.box_int));
        fx.op(I::I32Store(ma(8, 2)));
        store_node(&mut fx, nodes, id, node);
        fx.op(I::LocalGet(id));
        fx.op(I::Return);
        fx.op(I::End);
    }
    // TAG_VAR: a payload-less variant is a symbol (`sym`/`qsym` via mc_sym_node);
    // a payloaded variant `name(p)` mirrors `value_to_form` as a 1-argument call
    // `tup[sym(name), p]`.
    {
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(form));
        fx.op(I::I32Load(ma(8, 2))); // the variant's payload (0 if none)
        fx.op(I::If(BlockType::Empty));
        // payloaded → tup[ sym-node(name), fill(payload) ]
        bump(&mut fx);
        fx.op(I::LocalGet(id));
        fx.op(I::LocalSet(headid));
        fx.op(I::LocalGet(form));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::Call(sym_node_idx));
        fx.op(I::LocalSet(node));
        store_node(&mut fx, nodes, headid, node);
        fx.op(I::LocalGet(form));
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalGet(nodes));
        fx.op(I::LocalGet(cur));
        fx.op(I::Call(fill_idx));
        fx.op(I::LocalSet(pid));
        bump(&mut fx);
        // plist = [box_int(headid), box_int(pid)]
        fx.op(I::I32Const(16));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(plist));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Const(2));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(plist));
        fx.op(I::LocalGet(headid));
        fx.op(I::I64ExtendI32U);
        fx.op(I::Call(em.h.box_int));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(plist));
        fx.op(I::LocalGet(pid));
        fx.op(I::I64ExtendI32U);
        fx.op(I::Call(em.h.box_int));
        fx.op(I::I32Store(ma(12, 2)));
        fx.op(I::I32Const(12));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(node));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(em.intern_str("tup") as i32));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Store(ma(8, 2)));
        store_node(&mut fx, nodes, id, node);
        fx.op(I::LocalGet(id));
        fx.op(I::Return);
        fx.op(I::Else);
        // payload-less → sym / qsym
        bump(&mut fx);
        fx.op(I::LocalGet(form));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::Call(sym_node_idx));
        fx.op(I::LocalSet(node));
        store_node(&mut fx, nodes, id, node);
        fx.op(I::LocalGet(id));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::End);
    }
    // tup / lst: fill children first, then a wire node whose payload is the
    // list of child ids (as int boxes).
    for (tagk, case) in [(TAG_TUP, "tup"), (TAG_LIST, "lst")] {
        let caddr = em.intern_str(case) as i32;
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(tagk));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(form));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(m));
        // kids = int-box list of child ids
        fx.op(I::LocalGet(m));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(plist));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(plist));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(e));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(e));
        fx.op(I::LocalGet(m));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // plist[e] = box_int(fill(form[e]))
        fx.op(I::LocalGet(plist));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(form));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalGet(nodes));
        fx.op(I::LocalGet(cur));
        fx.op(I::Call(fill_idx));
        fx.op(I::I64ExtendI32U);
        fx.op(I::Call(em.h.box_int));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(e));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        bump(&mut fx);
        fx.op(I::I32Const(12));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(node));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(caddr));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Store(ma(8, 2)));
        store_node(&mut fx, nodes, id, node);
        fx.op(I::LocalGet(id));
        fx.op(I::Return);
        fx.op(I::End);
    }
    // rec: payload = list of (key-str, child-id) tuples
    {
        let caddr = em.intern_str("rec") as i32;
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_REC));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(form));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(m));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(plist));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(plist));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(e));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(e));
        fx.op(I::LocalGet(m));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // tupb = [TAG_TUP, 2, key, box_int(fill(value))]
        fx.op(I::I32Const(16));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(tupb));
        fx.op(I::LocalGet(tupb));
        fx.op(I::I32Const(TAG_TUP));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(tupb));
        fx.op(I::I32Const(2));
        fx.op(I::I32Store(ma(4, 2)));
        // key at [form + 8 + 8e]
        fx.op(I::LocalGet(tupb));
        fx.op(I::LocalGet(form));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::I32Store(ma(8, 2)));
        // value id box at tupb[12]
        fx.op(I::LocalGet(tupb));
        fx.op(I::LocalGet(form));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(12, 2)));
        fx.op(I::LocalGet(nodes));
        fx.op(I::LocalGet(cur));
        fx.op(I::Call(fill_idx));
        fx.op(I::I64ExtendI32U);
        fx.op(I::Call(em.h.box_int));
        fx.op(I::I32Store(ma(12, 2)));
        // plist[e] = tupb
        fx.op(I::LocalGet(plist));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(tupb));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(e));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        bump(&mut fx);
        fx.op(I::I32Const(12));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(node));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(caddr));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Store(ma(8, 2)));
        store_node(&mut fx, nodes, id, node);
        fx.op(I::LocalGet(id));
        fx.op(I::Return);
        fx.op(I::End);
    }
    // flg: a leaf wire node whose payload is the list<string> of names.
    {
        let caddr = em.intern_str("flg") as i32;
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_FLG));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(form));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(m));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(plist));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(plist));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(e));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(e));
        fx.op(I::LocalGet(m));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(plist));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(form));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(e));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        bump(&mut fx);
        fx.op(I::I32Const(12));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(node));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(caddr));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Store(ma(8, 2)));
        store_node(&mut fx, nodes, id, node);
        fx.op(I::LocalGet(id));
        fx.op(I::Return);
        fx.op(I::End);
    }
    // anything else → trap
    fx.op(I::Unreachable);
    let t = em.ty_idx(vec![I32, I32, I32], vec![I32]);
    Ok((t, fx.finish()))
}

/// `nodes[id] = node` (elements at +8 in the list box).
fn store_node(fx: &mut FnCtx, nodes: u32, id: u32, node: u32) {
    fx.op(I::LocalGet(nodes));
    fx.op(I::LocalGet(id));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::LocalGet(node));
    fx.op(I::I32Store(ma(8, 2)));
}

/// `box → tree`: flatten a form box into a wire `tree` record box (the inverse
/// of `mc_tree_to_form`). Sizes the node table via `count_nodes`, then fills it
/// with `fill`.
fn mc_form_to_tree(
    em: &mut Emitter,
    count_idx: u32,
    fill_idx: u32,
) -> Result<(u32, Function), String> {
    use ValType::I32;
    let mut fx = FnCtx::new(1);
    let form = 0u32;
    let count = fx.local(I32);
    let nodes = fx.local(I32);
    let cur = fx.local(I32);
    let root = fx.local(I32);
    let spans = fx.local(I32);
    let zspan = fx.local(I32);
    let e = fx.local(I32);
    let tree = fx.local(I32);

    fx.op(I::LocalGet(form));
    fx.op(I::Call(count_idx));
    fx.op(I::LocalSet(count));
    // nodes list box
    fx.op(I::LocalGet(count));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(nodes));
    fx.op(I::LocalGet(nodes));
    fx.op(I::I32Const(TAG_LIST));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(nodes));
    fx.op(I::LocalGet(count));
    fx.op(I::I32Store(ma(4, 2)));
    // cursor cell
    fx.op(I::I32Const(4));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(cur));
    fx.op(I::LocalGet(cur));
    fx.op(I::I32Const(0));
    fx.op(I::I32Store(ma(0, 2)));
    // root = fill(form, nodes, cur)
    fx.op(I::LocalGet(form));
    fx.op(I::LocalGet(nodes));
    fx.op(I::LocalGet(cur));
    fx.op(I::Call(fill_idx));
    fx.op(I::LocalSet(root));
    // zspan = [TAG_TUP, 2, box_int(0), box_int(0)]
    fx.op(I::I32Const(16));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(zspan));
    fx.op(I::LocalGet(zspan));
    fx.op(I::I32Const(TAG_TUP));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(zspan));
    fx.op(I::I32Const(2));
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::LocalGet(zspan));
    fx.op(I::I64Const(0));
    fx.op(I::Call(em.h.box_int));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(zspan));
    fx.op(I::I64Const(0));
    fx.op(I::Call(em.h.box_int));
    fx.op(I::I32Store(ma(12, 2)));
    // spans list (count copies of zspan)
    fx.op(I::LocalGet(count));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(spans));
    fx.op(I::LocalGet(spans));
    fx.op(I::I32Const(TAG_LIST));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(spans));
    fx.op(I::LocalGet(count));
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(e));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(e));
    fx.op(I::LocalGet(count));
    fx.op(I::I32GeU);
    fx.op(I::BrIf(1));
    fx.op(I::LocalGet(spans));
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::LocalGet(zspan));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(e));
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
    // tree record box {nodes, root, spans}
    let k_nodes = em.intern_str("nodes") as i32;
    let k_root = em.intern_str("root") as i32;
    let k_spans = em.intern_str("spans") as i32;
    fx.op(I::I32Const(8 + 8 * 3));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(tree));
    fx.op(I::LocalGet(tree));
    fx.op(I::I32Const(TAG_REC));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(tree));
    fx.op(I::I32Const(3));
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::LocalGet(tree));
    fx.op(I::I32Const(k_nodes));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(tree));
    fx.op(I::LocalGet(nodes));
    fx.op(I::I32Store(ma(12, 2)));
    fx.op(I::LocalGet(tree));
    fx.op(I::I32Const(k_root));
    fx.op(I::I32Store(ma(16, 2)));
    fx.op(I::LocalGet(tree));
    fx.op(I::LocalGet(root));
    fx.op(I::I64ExtendI32U);
    fx.op(I::Call(em.h.box_int));
    fx.op(I::I32Store(ma(20, 2)));
    fx.op(I::LocalGet(tree));
    fx.op(I::I32Const(k_spans));
    fx.op(I::I32Store(ma(24, 2)));
    fx.op(I::LocalGet(tree));
    fx.op(I::LocalGet(spans));
    fx.op(I::I32Store(ma(28, 2)));
    fx.op(I::LocalGet(tree));
    let t = em.ty_idx(vec![I32], vec![I32]);
    Ok((t, fx.finish()))
}

/// `manifest()` → `list<tuple<string, u32>>`: a constant list built from the
/// file's macros, lowered to a parked `(ptr, len)` (the canonical list return).
fn mc_manifest(em: &mut Emitter, macros: &[MacroDef]) -> Result<(u32, Function), String> {
    use ValType::I32;
    let mut fx = FnCtx::new(0);
    let n = macros.len();
    let lst = fx.local(I32);
    fx.op(I::I32Const(8 + 4 * n as i32));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(lst));
    fx.op(I::LocalGet(lst));
    fx.op(I::I32Const(TAG_LIST));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(lst));
    fx.op(I::I32Const(n as i32));
    fx.op(I::I32Store(ma(4, 2)));
    for (i, m) in macros.iter().enumerate() {
        let name_addr = em.intern_str(&m.name) as i32;
        let tup = fx.local(I32);
        fx.op(I::I32Const(16));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(tup));
        fx.op(I::LocalGet(tup));
        fx.op(I::I32Const(TAG_TUP));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(tup));
        fx.op(I::I32Const(2));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(tup));
        fx.op(I::I32Const(name_addr));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(tup));
        fx.op(I::I64Const(m.params.len() as i64));
        fx.op(I::Call(em.h.box_int));
        fx.op(I::I32Store(ma(12, 2)));
        fx.op(I::LocalGet(lst));
        fx.op(I::LocalGet(tup));
        fx.op(I::I32Store(ma(8 + 4 * i as u64, 2)));
    }
    // lower list<tuple<string,u32>> → (ptr,len) parked in an 8-byte area
    let list_ty = WitTy::List(Box::new(WitTy::Tuple(vec![WitTy::Str, WitTy::IntU])));
    let lp = fx.local(I32);
    let ll = fx.local(I32);
    let area = fx.local(I32);
    fx.op(I::LocalGet(lst));
    em.lower(&mut fx, &list_ty)?;
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
    let t = em.ty_idx(vec![], vec![I32]);
    Ok((t, fx.finish()))
}

/// `expand(name, args)` → `result<tree, string>`: lift the args tree to forms,
/// dispatch to the named compiled macro body (binding the call's argument
/// forms), convert the result form back to a tree, and lower `result<tree,
/// string>` through a return area.
fn mc_expand(
    em: &mut Emitter,
    macros: &[MacroDef],
    tree_to_form_idx: u32,
    form_to_tree_idx: u32,
) -> Result<(u32, Function), String> {
    use ValType::I32;
    let tree_ty = meta_tree_wit_ty();
    let result_ty = WitTy::Result(Box::new(tree_ty.clone()), Box::new(WitTy::Str));

    let mut fparams: Vec<ValType> = Vec::new();
    fparams.extend_from_slice(&flat_checked(&WitTy::Str)?);
    let args_base = fparams.len() as u32;
    fparams.extend_from_slice(&flat_checked(&tree_ty)?);

    let mut fx = FnCtx::new(fparams.len() as u32);
    // lift name (str) and args (tree) into boxes
    em.lift_flat(&mut fx, &WitTy::Str, 0)?;
    let name_box = fx.local(I32);
    fx.op(I::LocalSet(name_box));
    em.lift_flat(&mut fx, &tree_ty, args_base)?;
    let args_box = fx.local(I32);
    fx.op(I::LocalSet(args_box));
    // call form (the whole call tup) and its argument count
    let call = fx.local(I32);
    fx.op(I::LocalGet(args_box));
    fx.op(I::Call(tree_to_form_idx));
    fx.op(I::LocalSet(call));
    let nargs = fx.local(I32);
    fx.op(I::LocalGet(call));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::I32Const(1));
    fx.op(I::I32Sub);
    fx.op(I::LocalSet(nargs));

    let res = fx.local(I32);
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(res));
    let tupb = fx.local(I32);
    let e = fx.local(I32);

    for m in macros {
        let name_addr = em.intern_str(&m.name) as i32;
        let arity = m.params.len();
        let fidx = em.funcs[&m.name].0;
        fx.op(I::LocalGet(name_box));
        fx.op(I::I32Const(name_addr));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        // equal-arity fast path
        fx.op(I::LocalGet(nargs));
        fx.op(I::I32Const(arity as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        for j in 1..=arity {
            fx.op(I::LocalGet(call));
            fx.op(I::I32Load(ma(8 + 4 * j as u64, 2)));
        }
        fx.op(I::Call(fidx));
        fx.op(I::Call(form_to_tree_idx));
        em.wrap_variant(&mut fx, "ok");
        fx.op(I::LocalSet(res));
        fx.op(I::Else);
        if arity == 1 {
            // a 1-param macro given several args binds the whole args tuple
            // (`expand_once`'s rule): build TAG_TUP of call[1..].
            fx.op(I::LocalGet(nargs));
            fx.op(I::I32Const(4));
            fx.op(I::I32Mul);
            fx.op(I::I32Const(8));
            fx.op(I::I32Add);
            fx.op(I::Call(em.h.alloc));
            fx.op(I::LocalSet(tupb));
            fx.op(I::LocalGet(tupb));
            fx.op(I::I32Const(TAG_TUP));
            fx.op(I::I32Store(ma(0, 2)));
            fx.op(I::LocalGet(tupb));
            fx.op(I::LocalGet(nargs));
            fx.op(I::I32Store(ma(4, 2)));
            fx.op(I::I32Const(0));
            fx.op(I::LocalSet(e));
            fx.op(I::Block(BlockType::Empty));
            fx.op(I::Loop(BlockType::Empty));
            fx.op(I::LocalGet(e));
            fx.op(I::LocalGet(nargs));
            fx.op(I::I32GeU);
            fx.op(I::BrIf(1));
            // tupb[e] = call[1+e]
            fx.op(I::LocalGet(tupb));
            fx.op(I::LocalGet(e));
            fx.op(I::I32Const(4));
            fx.op(I::I32Mul);
            fx.op(I::I32Add);
            fx.op(I::LocalGet(call));
            fx.op(I::LocalGet(e));
            fx.op(I::I32Const(1));
            fx.op(I::I32Add);
            fx.op(I::I32Const(4));
            fx.op(I::I32Mul);
            fx.op(I::I32Add);
            fx.op(I::I32Load(ma(8, 2)));
            fx.op(I::I32Store(ma(8, 2)));
            fx.op(I::LocalGet(e));
            fx.op(I::I32Const(1));
            fx.op(I::I32Add);
            fx.op(I::LocalSet(e));
            fx.op(I::Br(0));
            fx.op(I::End);
            fx.op(I::End);
            fx.op(I::LocalGet(tupb));
            fx.op(I::Call(fidx));
            fx.op(I::Call(form_to_tree_idx));
            em.wrap_variant(&mut fx, "ok");
            fx.op(I::LocalSet(res));
        } else {
            let msg = em.intern_str(&format!(
                "macro `{}` expects {arity} arguments",
                m.name
            )) as i32;
            fx.op(I::I32Const(msg));
            em.wrap_variant(&mut fx, "err");
            fx.op(I::LocalSet(res));
        }
        fx.op(I::End);
        fx.op(I::End);
    }
    // unknown macro → err "unknown macro `<name>`"
    fx.op(I::LocalGet(res));
    fx.op(I::I32Eqz);
    fx.op(I::If(BlockType::Empty));
    let pre = em.intern_str("unknown macro `") as i32;
    let post = em.intern_str("`") as i32;
    fx.op(I::I32Const(pre));
    fx.op(I::LocalGet(name_box));
    fx.op(I::Call(em.h.strcat2));
    fx.op(I::I32Const(post));
    fx.op(I::Call(em.h.strcat2));
    em.wrap_variant(&mut fx, "err");
    fx.op(I::LocalSet(res));
    fx.op(I::End);
    // lower result<tree,string> into a return area
    let area = fx.local(I32);
    fx.op(I::I32Const(size_of(&result_ty) as i32));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(area));
    em.store_to_mem(&mut fx, &result_ty, res, area, 0)?;
    fx.op(I::LocalGet(area));
    let t = em.ty_idx(fparams, vec![I32]);
    Ok((t, fx.finish()))
}

/// The guest-internal one-step expander behind the in-macro `expand` builtin
/// (mirrors `builtins.rs` `expand`): given a form, if it is a call `(name-MACRO
/// …)` to one of this library's macros, run that macro's compiled body **once**
/// over the call's argument forms and return the result; otherwise return the
/// form unchanged. Signature `(form) -> form`.
fn mc_expand_step(em: &mut Emitter, macros: &[MacroDef]) -> Result<(u32, Function), String> {
    use ValType::I32;
    let mut fx = FnCtx::new(1);
    let form = 0u32;
    let head = fx.local(I32);
    let nargs = fx.local(I32);
    let tupb = fx.local(I32);
    let e = fx.local(I32);

    // Not a call tuple → unchanged.
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(0, 2)));
    fx.op(I::I32Const(TAG_TUP));
    fx.op(I::I32Ne);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(form));
    fx.op(I::Return);
    fx.op(I::End);
    // Empty tuple → unchanged.
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::I32Eqz);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(form));
    fx.op(I::Return);
    fx.op(I::End);
    // head = element 0; must be a payload-less symbol (TAG_VAR, payload 0).
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(8, 2)));
    fx.op(I::LocalSet(head));
    fx.op(I::LocalGet(head));
    fx.op(I::I32Load(ma(0, 2)));
    fx.op(I::I32Const(TAG_VAR));
    fx.op(I::I32Ne);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(form));
    fx.op(I::Return);
    fx.op(I::End);
    fx.op(I::LocalGet(head));
    fx.op(I::I32Load(ma(8, 2)));
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(form));
    fx.op(I::Return);
    fx.op(I::End);
    // nargs = len - 1
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::I32Const(1));
    fx.op(I::I32Sub);
    fx.op(I::LocalSet(nargs));

    for m in macros {
        let name_macro = em.intern_str(&format!("{}-MACRO", m.name)) as i32;
        let arity = m.params.len();
        let fidx = em.funcs[&m.name].0;
        // head case string == "<name>-MACRO" ?
        fx.op(I::LocalGet(head));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Const(name_macro));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(nargs));
        fx.op(I::I32Const(arity as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        for j in 1..=arity {
            fx.op(I::LocalGet(form));
            fx.op(I::I32Load(ma(8 + 4 * j as u64, 2)));
        }
        fx.op(I::Call(fidx));
        fx.op(I::Return);
        fx.op(I::Else);
        if arity == 1 {
            // a 1-param macro given several args binds the whole args tuple
            fx.op(I::LocalGet(nargs));
            fx.op(I::I32Const(4));
            fx.op(I::I32Mul);
            fx.op(I::I32Const(8));
            fx.op(I::I32Add);
            fx.op(I::Call(em.h.alloc));
            fx.op(I::LocalSet(tupb));
            fx.op(I::LocalGet(tupb));
            fx.op(I::I32Const(TAG_TUP));
            fx.op(I::I32Store(ma(0, 2)));
            fx.op(I::LocalGet(tupb));
            fx.op(I::LocalGet(nargs));
            fx.op(I::I32Store(ma(4, 2)));
            fx.op(I::I32Const(0));
            fx.op(I::LocalSet(e));
            fx.op(I::Block(BlockType::Empty));
            fx.op(I::Loop(BlockType::Empty));
            fx.op(I::LocalGet(e));
            fx.op(I::LocalGet(nargs));
            fx.op(I::I32GeU);
            fx.op(I::BrIf(1));
            fx.op(I::LocalGet(tupb));
            fx.op(I::LocalGet(e));
            fx.op(I::I32Const(4));
            fx.op(I::I32Mul);
            fx.op(I::I32Add);
            fx.op(I::LocalGet(form));
            fx.op(I::LocalGet(e));
            fx.op(I::I32Const(1));
            fx.op(I::I32Add);
            fx.op(I::I32Const(4));
            fx.op(I::I32Mul);
            fx.op(I::I32Add);
            fx.op(I::I32Load(ma(8, 2)));
            fx.op(I::I32Store(ma(8, 2)));
            fx.op(I::LocalGet(e));
            fx.op(I::I32Const(1));
            fx.op(I::I32Add);
            fx.op(I::LocalSet(e));
            fx.op(I::Br(0));
            fx.op(I::End);
            fx.op(I::End);
            fx.op(I::LocalGet(tupb));
            fx.op(I::Call(fidx));
            fx.op(I::Return);
        } else {
            // arity mismatch: leave the form unchanged (rare; the interpreter
            // would raise an eval error here).
            fx.op(I::LocalGet(form));
            fx.op(I::Return);
        }
        fx.op(I::End);
        fx.op(I::End);
    }
    // No matching macro → unchanged.
    fx.op(I::LocalGet(form));
    let t = em.ty_idx(vec![I32], vec![I32]);
    Ok((t, fx.finish()))
}

fn param_names(arena: &Arena, params_id: NodeId) -> Result<Vec<String>, String> {
    match arena.node(params_id) {
        Node::Flg(names) => Ok(names.clone()),
        Node::Rec(fields) => Ok(fields.iter().map(|(k, _)| k.clone()).collect()),
        _ => Err("malformed Fn parameters".into()),
    }
}

fn emit_helpers(em: &mut Emitter) -> Result<(), String> {
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

    // tail_h(list box) -> list box   [locals: src=0, n, m, dst, i]
    {
        let mut fx = FnCtx::new(1);
        let n = fx.local(I32);
        let m = fx.local(I32);
        let dst = fx.local(I32);
        let i = fx.local(I32);
        // require a non-empty list
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalTee(n));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        // m = n - 1
        fx.op(I::LocalGet(n));
        fx.op(I::I32Const(1));
        fx.op(I::I32Sub);
        fx.op(I::LocalSet(m));
        // dst = alloc(8 + 4*m)
        fx.op(I::I32Const(8));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Const(2));
        fx.op(I::I32Shl);
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(dst));
        fx.op(I::LocalGet(dst));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(dst));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Store(ma(4, 2)));
        // for i in 0..m: dst[8+4i] = src[8+4(i+1)]
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(m));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // dst + 8 + 4*i
        fx.op(I::LocalGet(dst));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(2));
        fx.op(I::I32Shl);
        fx.op(I::I32Add);
        // value: src[8 + 4*(i+1)] = src + 12 + 4*i
        fx.op(I::LocalGet(0));
        fx.op(I::I32Const(12));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(2));
        fx.op(I::I32Shl);
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
        fx.op(I::LocalGet(dst));
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

    // as_f64(box) -> f64   coerces an int or dec box to f64; traps otherwise.
    // Mirrors the interpreter's `want_num` widening of ints in mixed arithmetic.
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(F64)));
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::F64ConvertI64S);
        fx.op(I::Else);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_DEC));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::F64Load(ma(8, 3)));
        fx.op(I::End);
        let t = em.ty_idx(vec![I32], vec![F64]);
        em.bodies.push((t, fx.finish()));
    }

    // arith_raw(a, b, op) -> box   op: 0=add 1=sub 2=mul 3=div 4=rem.
    // Matches the interpreter `arith`: both ints → checked i64 (trap on
    // overflow / div-0 / INT_MIN÷-1); otherwise both widened to f64.
    // [locals: ia=3, ib=4, r=5 (i64); xf=6, yf=7 (f64)]
    {
        let mut fx = FnCtx::new(3);
        let ia = fx.local(I64);
        let ib = fx.local(I64);
        let r = fx.local(I64);
        let xf = fx.local(F64);
        let yf = fx.local(F64);
        // both int?
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Eq);
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Eq);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Result(I32)));
        // ---- int path
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::LocalSet(ia));
        fx.op(I::LocalGet(1));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::LocalSet(ib));
        // op == 0 : add
        fx.op(I::LocalGet(2));
        fx.op(I::I32Const(0));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(I64)));
        fx.op(I::LocalGet(ia));
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Add);
        fx.op(I::LocalSet(r));
        // overflow: ((r^ia) & (r^ib)) <s 0
        fx.op(I::LocalGet(r));
        fx.op(I::LocalGet(ia));
        fx.op(I::I64Xor);
        fx.op(I::LocalGet(r));
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Xor);
        fx.op(I::I64And);
        fx.op(I::I64Const(0));
        fx.op(I::I64LtS);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(r));
        fx.op(I::Else);
        // op == 1 : sub
        fx.op(I::LocalGet(2));
        fx.op(I::I32Const(1));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(I64)));
        fx.op(I::LocalGet(ia));
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Sub);
        fx.op(I::LocalSet(r));
        // overflow: ((ia^ib) & (ia^r)) <s 0
        fx.op(I::LocalGet(ia));
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Xor);
        fx.op(I::LocalGet(ia));
        fx.op(I::LocalGet(r));
        fx.op(I::I64Xor);
        fx.op(I::I64And);
        fx.op(I::I64Const(0));
        fx.op(I::I64LtS);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(r));
        fx.op(I::Else);
        // op == 2 : mul
        fx.op(I::LocalGet(2));
        fx.op(I::I32Const(2));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(I64)));
        fx.op(I::LocalGet(ia));
        fx.op(I::I64Eqz);
        fx.op(I::If(BlockType::Result(I64)));
        fx.op(I::I64Const(0));
        fx.op(I::Else);
        // trap on ia==-1 && ib==INT_MIN (the one case r/ia would itself trap)
        fx.op(I::LocalGet(ia));
        fx.op(I::I64Const(-1));
        fx.op(I::I64Eq);
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Const(i64::MIN));
        fx.op(I::I64Eq);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(ia));
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Mul);
        fx.op(I::LocalSet(r));
        // overflow if r / ia != ib
        fx.op(I::LocalGet(r));
        fx.op(I::LocalGet(ia));
        fx.op(I::I64DivS);
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(r));
        fx.op(I::End);
        fx.op(I::Else);
        // op == 3 : div
        fx.op(I::LocalGet(2));
        fx.op(I::I32Const(3));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(I64)));
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(ia));
        fx.op(I::I64Const(i64::MIN));
        fx.op(I::I64Eq);
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Const(-1));
        fx.op(I::I64Eq);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(ia));
        fx.op(I::LocalGet(ib));
        fx.op(I::I64DivS);
        fx.op(I::Else);
        // op == 4 : rem
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(ia));
        fx.op(I::I64Const(i64::MIN));
        fx.op(I::I64Eq);
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Const(-1));
        fx.op(I::I64Eq);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(ia));
        fx.op(I::LocalGet(ib));
        fx.op(I::I64RemS);
        fx.op(I::End); // op == 3
        fx.op(I::End); // op == 2
        fx.op(I::End); // op == 1
        fx.op(I::End); // op == 0
        fx.op(I::Call(em.h.box_int));
        fx.op(I::Else);
        // ---- float path
        fx.op(I::LocalGet(0));
        fx.op(I::Call(em.h.as_f64));
        fx.op(I::LocalSet(xf));
        fx.op(I::LocalGet(1));
        fx.op(I::Call(em.h.as_f64));
        fx.op(I::LocalSet(yf));
        fx.op(I::LocalGet(2));
        fx.op(I::I32Const(0));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(F64)));
        fx.op(I::LocalGet(xf));
        fx.op(I::LocalGet(yf));
        fx.op(I::F64Add);
        fx.op(I::Else);
        fx.op(I::LocalGet(2));
        fx.op(I::I32Const(1));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(F64)));
        fx.op(I::LocalGet(xf));
        fx.op(I::LocalGet(yf));
        fx.op(I::F64Sub);
        fx.op(I::Else);
        fx.op(I::LocalGet(2));
        fx.op(I::I32Const(2));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(F64)));
        fx.op(I::LocalGet(xf));
        fx.op(I::LocalGet(yf));
        fx.op(I::F64Mul);
        fx.op(I::Else);
        fx.op(I::LocalGet(2));
        fx.op(I::I32Const(3));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(F64)));
        fx.op(I::LocalGet(xf));
        fx.op(I::LocalGet(yf));
        fx.op(I::F64Div);
        fx.op(I::Else);
        // rem: xf - trunc(xf/yf)*yf  (matches Rust f64 `%`)
        fx.op(I::LocalGet(xf));
        fx.op(I::LocalGet(xf));
        fx.op(I::LocalGet(yf));
        fx.op(I::F64Div);
        fx.op(I::F64Trunc);
        fx.op(I::LocalGet(yf));
        fx.op(I::F64Mul);
        fx.op(I::F64Sub);
        fx.op(I::End); // op == 3
        fx.op(I::End); // op == 2
        fx.op(I::End); // op == 1
        fx.op(I::End); // op == 0
        fx.op(I::Call(em.h.box_dec));
        fx.op(I::End); // int vs float
        let t = em.ty_idx(vec![I32, I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // cmp_raw(a, b) -> i32 in {-1, 0, 1}   total order over strings (byte
    // lexicographic) and numbers (widened to f64); traps on NaN/non-comparable,
    // matching the interpreter's `compare`. Chars ride in int boxes, so the
    // numeric path already orders them by codepoint.
    // [locals: xf=2, yf=3 (f64); la=4, lb=5, n=6, i=7, ca=8, cb=9 (i32)]
    {
        let mut fx = FnCtx::new(2);
        let xf = fx.local(F64);
        let yf = fx.local(F64);
        let la = fx.local(I32);
        let lb = fx.local(I32);
        let n = fx.local(I32);
        let i = fx.local(I32);
        let ca = fx.local(I32);
        let cb = fx.local(I32);
        // both str?
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Eq);
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Eq);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Result(I32)));
        // ---- string lexicographic compare
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(la));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(lb));
        // n = min(la, lb)
        fx.op(I::LocalGet(la));
        fx.op(I::LocalGet(lb));
        fx.op(I::I32LtU);
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::LocalGet(la));
        fx.op(I::Else);
        fx.op(I::LocalGet(lb));
        fx.op(I::End);
        fx.op(I::LocalSet(n));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(n));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(8, 0)));
        fx.op(I::LocalSet(ca));
        fx.op(I::LocalGet(1));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(8, 0)));
        fx.op(I::LocalSet(cb));
        fx.op(I::LocalGet(ca));
        fx.op(I::LocalGet(cb));
        fx.op(I::I32LtU);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(-1));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::LocalGet(ca));
        fx.op(I::LocalGet(cb));
        fx.op(I::I32GtU);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(1));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End); // loop
        fx.op(I::End); // block
        // equal prefix: shorter string is less
        fx.op(I::LocalGet(la));
        fx.op(I::LocalGet(lb));
        fx.op(I::I32LtU);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(-1));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::LocalGet(la));
        fx.op(I::LocalGet(lb));
        fx.op(I::I32GtU);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(1));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::I32Const(0));
        fx.op(I::Else);
        // ---- numeric compare (widened to f64)
        fx.op(I::LocalGet(0));
        fx.op(I::Call(em.h.as_f64));
        fx.op(I::LocalSet(xf));
        fx.op(I::LocalGet(1));
        fx.op(I::Call(em.h.as_f64));
        fx.op(I::LocalSet(yf));
        fx.op(I::LocalGet(xf));
        fx.op(I::LocalGet(yf));
        fx.op(I::F64Lt);
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::I32Const(-1));
        fx.op(I::Else);
        fx.op(I::LocalGet(xf));
        fx.op(I::LocalGet(yf));
        fx.op(I::F64Gt);
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::I32Const(1));
        fx.op(I::Else);
        fx.op(I::LocalGet(xf));
        fx.op(I::LocalGet(yf));
        fx.op(I::F64Eq);
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::I32Const(0));
        fx.op(I::Else);
        // unordered (NaN) — not comparable
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::End); // str vs numeric
        let t = em.ty_idx(vec![I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // neg_raw(box) -> box   negates an int (wrapping, as the interpreter's `-n`)
    // or a dec; traps on anything else.
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::I64Const(0));
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::I64Sub);
        fx.op(I::Call(em.h.box_int));
        fx.op(I::Else);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_DEC));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::F64Load(ma(8, 3)));
        fx.op(I::F64Neg);
        fx.op(I::Call(em.h.box_dec));
        fx.op(I::End);
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    Ok(())
}

// ----------------------------------------------------------- WIT synthesis

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
) -> Result<String, String> {
    let mut out = format!("package {};\n\n", info.package);

    let ifaces = crate::wit::iface_order(&info.exports, !info.types.is_empty());
    // External interfaces (e.g. wasi:http/incoming-handler, wasi:cli/run) are
    // defined by the dependency's WIT; we only export them by name, never
    // re-declare them here.
    for iface in ifaces.iter().filter(|i| !is_external_iface(i)) {
        out.push_str(&format!("interface {iface} {{\n"));
        if iface == "api" {
            for (name, ty) in &info.types {
                out.push_str(&format!("  {}\n", type_decl(arena, name, *ty)?));
            }
        }
        for sig in info.exports.iter().filter(|s| &s.iface == iface) {
            out.push_str(&format!("  {}\n", sig.to_wit()));
        }
        out.push_str("}\n\n");
    }

    out.push_str(&format!("world {} {{\n", info.world));
    for imp in &info.imports {
        // A pure macro import (§6.3) is compile-time only: it is resolved to a
        // macro component and run during expansion, contributing no runtime
        // import to the synthesized world. Skip it here (mirroring `build`'s
        // dep-resolution skip) so a file that uses foreign macros but no runtime
        // dependency from that package still synthesizes a valid world.
        if crate::wit::is_macro_only(imp) {
            continue;
        }
        let iface = import_iface(&imp.path);
        let dep = deps
            .get(&imp.package)
            .ok_or(format!("dependency `{}` is not in the build set", imp.package))?;
        out.push_str(&format!("  import {};\n", versioned_iface(&dep.package, &iface)));
    }
    for iface in &ifaces {
        if is_external_iface(iface) {
            out.push_str(&format!("  export {};\n", external_versioned_in(iface, deps)));
        } else {
            out.push_str(&format!("  export {iface};\n"));
        }
    }
    out.push_str("}\n");

    // Append each dep's nested-package WIT, but emit any given package only once.
    // A `wit/deps` dep carries its whole transitive closure (e.g. both the
    // `wasi:http` and `wasi:io/streams` deps render `wasi:io`, `wasi:clocks`,
    // …), so concatenating them verbatim would define a package twice.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for dep in deps.values() {
        for block in split_package_blocks(&dep.package_wit) {
            let dup = package_block_name(block).is_some_and(|name| !seen.insert(name));
            if !dup {
                out.push_str(block);
            }
        }
    }
    Ok(out)
}

/// Split a concatenation of top-level `package NAME { … }` blocks (and any
/// leading flat `package NAME;` lines) into individual block slices, splitting
/// on brace balance returning to zero. Text that isn't a braced package block
/// (e.g. a trailing `package x;` line) is returned as its own slice.
fn split_package_blocks(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut blocks = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    // include the trailing newline if present
                    let mut end = i + 1;
                    if end < bytes.len() && bytes[end] == b'\n' {
                        end += 1;
                    }
                    blocks.push(&s[start..end]);
                    start = end;
                    i = end;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if start < s.len() {
        let tail = &s[start..];
        if !tail.trim().is_empty() {
            blocks.push(tail);
        }
    }
    blocks
}

/// The `ns:name@ver` of a `package NAME { … }` or `package NAME;` block, if it
/// starts with the `package` keyword.
fn package_block_name(block: &str) -> Option<String> {
    let rest = block.trim_start().strip_prefix("package ")?;
    let name: String = rest
        .chars()
        .take_while(|&c| c != '{' && c != ';' && !c.is_whitespace())
        .collect();
    if name.is_empty() { None } else { Some(name) }
}

// ---------------------------------------------------------- set-resource tests
//
// Step 02 verification. We drive the REAL `emit_set_resource` bodies (not the
// hand-authored spike ones) through the SAME `embed_component_metadata` +
// `ComponentEncoder` pipeline `emit_component` uses, then instantiate via
// `HostComponent` and exercise ctor → add (incl. a duplicate) → size → contains.
// This proves the rep/list/eq_raw bodies dedup and answer membership correctly.
// It does NOT go through `emit_component` (that wiring is step 03), so it is a
// minimal hand-assembled module — but every `set` body is the production one.
#[cfg(test)]
mod set_resource_tests {
    use super::*;
    use crate::host::{HostComponent, Val};
    use crate::wit::{FunctorInst, FunctorKind};

    const IFACE: &str = "demo:app/s32-set@0.1.0";
    const EXPORT_MOD: &str = "[export]demo:app/s32-set@0.1.0";

    const WIT: &str = r#"package demo:app@0.1.0;

interface s32-set {
  resource set {
    constructor();
    add: func(value: s32);
    contains: func(value: s32) -> bool;
    size: func() -> u32;
  }
}

world app {
  export s32-set;
}
"#;

    /// Stand up a minimal `Emitter` exactly as `emit_core_module` does up to the
    /// point `emit_set_resource` needs (static boxes, the three resource-intrinsic
    /// imports, helper indices, helper bodies), call `emit_set_resource`, and
    /// assemble a core module with the verified ABI export names.
    fn build_core(elem: &WitTy) -> Result<Vec<u8>, String> {
        use ValType::I32;

        // A FileInfo / deps with nothing in them: the set bodies are self-contained.
        let arena = Arena::new();
        let info = FileInfo {
            package: "demo:app@0.1.0".to_string(),
            package_path: "demo:app".to_string(),
            world: "app".to_string(),
            imports: Vec::new(),
            functors: Vec::new(),
            exports: Vec::new(),
            types: Vec::new(),
            defs: HashMap::new(),
            fn_defs: HashMap::new(),
            value_defs: Vec::new(),
            overload_bodies: HashMap::new(),
        };
        let deps: HashMap<String, Dep> = HashMap::new();

        let mut em = Emitter {
            arena: &arena,
            info: &info,
            deps: &deps,
            type_env: TypeEnv::default(),
            data: Vec::new(),
            str_cache: HashMap::new(),
            types: Vec::new(),
            imports: Vec::new(),
            import_fn: HashMap::new(),
            h: Helpers {
                alloc: 0, realloc: 0, box_int: 0, box_bool: 0, box_dec: 0,
                box_str: 0, truthy: 0, unbox_int: 0, unbox_dec: 0, eq_raw: 0,
                len_raw: 0, head_h: 0, tail_h: 0, strcat2: 0, case_h: 0,
                to_str: 0, rec_get: 0, as_f64: 0, arith_raw: 0, cmp_raw: 0,
                neg_raw: 0,
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
            macro_expand_idx: None,
        };

        // static boxes: false @16, true @24 (same as emit_core_module).
        em.false_addr = DATA_BASE;
        em.put_i32(TAG_BOOL);
        em.put_i32(0);
        em.true_addr = DATA_BASE + 8;
        em.put_i32(TAG_BOOL);
        em.put_i32(1);

        // ---- imports: the three resource intrinsics, declared up front so the
        // function index space is imports-first (exactly emit_component's order).
        let mut n_imports = 0u32;
        let mut add_import =
            |em: &mut Emitter, field: &str, p: Vec<ValType>, r: Vec<ValType>| {
                let t = em.ty_idx(p, r);
                em.imports.push((EXPORT_MOD.to_string(), field.to_string(), t));
                em.import_fn
                    .insert((EXPORT_MOD.to_string(), field.to_string()), n_imports);
                n_imports += 1;
            };
        add_import(&mut em, "[resource-new]set", vec![I32], vec![I32]);
        add_import(&mut em, "[resource-rep]set", vec![I32], vec![I32]);
        add_import(&mut em, "[resource-drop]set", vec![I32], vec![]);
        let new_i = em.import_idx(EXPORT_MOD, "[resource-new]set");
        let rep_i = em.import_idx(EXPORT_MOD, "[resource-rep]set");
        let drop_i = em.import_idx(EXPORT_MOD, "[resource-drop]set");

        // ---- helper indices (same order/assignment as emit_core_module).
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
        em.h.tail_h = take();
        em.h.strcat2 = take();
        em.h.case_h = take();
        em.h.to_str = take();
        em.h.rec_get = take();
        em.h.as_f64 = take();
        em.h.arith_raw = take();
        em.h.cmp_raw = take();
        em.h.neg_raw = take();

        // helper bodies (must precede our set bodies, matching index order).
        emit_helpers(&mut em)?;

        let inst = FunctorInst {
            kind: FunctorKind::Set,
            alias: "xs".to_string(),
            elem: "s32".to_string(),
            iface: "s32-set".to_string(),
        };
        let fns = emit_set_resource(&mut em, &inst, elem, new_i, rep_i, drop_i)?;

        // ---- assemble (mirror emit_core_module's section order, minus the
        // features the set bodies don't use: no closures, globals, value defs).
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

        let mut fs = FunctionSection::new();
        for (t, _) in &em.bodies {
            fs.function(*t);
        }
        module.section(&fs);

        let mut ms = MemorySection::new();
        ms.memory(MemoryType {
            minimum: pages,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&ms);

        // heap pointer global (global 0), same as the real assembly.
        let mut gs = GlobalSection::new();
        gs.global(
            GlobalType { val_type: I32, mutable: true, shared: false },
            &ConstExpr::i32_const(heap_base as i32),
        );
        module.section(&gs);

        let mut es = ExportSection::new();
        es.export("memory", ExportKind::Memory, 0);
        es.export("cabi_realloc", ExportKind::Func, em.h.realloc);
        es.export(&format!("{IFACE}#[constructor]set"), ExportKind::Func, fns.ctor);
        es.export(&format!("{IFACE}#[method]set.add"), ExportKind::Func, fns.add);
        es.export(&format!("{IFACE}#[method]set.contains"), ExportKind::Func, fns.contains);
        es.export(&format!("{IFACE}#[method]set.size"), ExportKind::Func, fns.size);
        es.export(&format!("{IFACE}#[dtor]set"), ExportKind::Func, fns.dtor);
        module.section(&es);

        let mut cs = CodeSection::new();
        for (_, f) in &em.bodies {
            cs.function(f);
        }
        module.section(&cs);

        let mut ds = DataSection::new();
        ds.active(0, &ConstExpr::i32_const(DATA_BASE as i32), em.data.iter().copied());
        module.section(&ds);

        Ok(module.finish())
    }

    /// Run a core module through the real componentize pipeline.
    fn componentize(elem: &WitTy) -> Result<Vec<u8>, String> {
        let mut module = build_core(elem)?;

        let mut resolve = wit_parser::Resolve::default();
        let pkg = resolve
            .push_str("set.wit", WIT)
            .map_err(|e| format!("WIT parse: {e:#}"))?;
        let world = resolve
            .select_world(&[pkg], Some("app"))
            .map_err(|e| format!("world select: {e:#}"))?;

        wit_component::embed_component_metadata(
            &mut module,
            &resolve,
            world,
            wit_component::StringEncoding::UTF8,
        )
        .map_err(|e| format!("embed metadata: {e:#}"))?;

        if std::env::var("SET_DUMP").is_ok() {
            std::fs::write("/tmp/set_embedded_core.wasm", &module).unwrap();
        }

        wit_component::ComponentEncoder::default()
            .validate(true)
            .module(&module)
            .map_err(|e| format!("componentize: {e:#}"))?
            .encode()
            .map_err(|e| format!("encode: {e:#}"))
    }

    #[test]
    fn set_bodies_dedup_and_membership_s32() {
        let bytes = componentize(&WitTy::IntS).expect("componentize + validate");
        let mut c = HostComponent::from_bytes(&bytes).expect("instantiate");

        // constructor() -> own<set>
        let ctor_out = c
            .call_instance(IFACE, "[constructor]set", &[])
            .expect("constructor call");
        let handle = match &ctor_out[0] {
            Val::Resource(_) => ctor_out[0].clone(),
            other => panic!("ctor should return a resource, got {other:?}"),
        };

        let size = |c: &mut HostComponent, h: &Val| -> u32 {
            match c.call_instance(IFACE, "[method]set.size", &[h.clone()]).unwrap()[..] {
                [Val::U32(n)] => n,
                ref other => panic!("size returned {other:?}"),
            }
        };
        let contains = |c: &mut HostComponent, h: &Val, v: i32| -> bool {
            match c
                .call_instance(IFACE, "[method]set.contains", &[h.clone(), Val::S32(v)])
                .unwrap()[..]
            {
                [Val::Bool(b)] => b,
                ref other => panic!("contains returned {other:?}"),
            }
        };
        let add = |c: &mut HostComponent, h: &Val, v: i32| {
            c.call_instance(IFACE, "[method]set.add", &[h.clone(), Val::S32(v)])
                .unwrap();
        };

        // fresh set is empty
        assert_eq!(size(&mut c, &handle), 0, "new set is empty");
        assert!(!contains(&mut c, &handle, 7), "empty set contains nothing");

        // add 7, 42, 7 (duplicate) → dedup keeps size 2
        add(&mut c, &handle, 7);
        add(&mut c, &handle, 42);
        add(&mut c, &handle, 7); // duplicate: must NOT grow the set
        assert_eq!(size(&mut c, &handle), 2, "duplicate add is deduped by eq_raw");

        // membership is exact
        assert!(contains(&mut c, &handle, 7), "7 is present");
        assert!(contains(&mut c, &handle, 42), "42 is present");
        assert!(!contains(&mut c, &handle, 100), "100 was never added");

        // a third distinct element grows the set; identity persists across calls
        add(&mut c, &handle, 100);
        assert_eq!(size(&mut c, &handle), 3, "distinct add grows the set");
        assert!(contains(&mut c, &handle, 100), "100 now present");

        // dropping the handle runs the no-op dtor cleanly.
        c.drop_resource(handle).expect("drop runs the no-op dtor");
    }
}
