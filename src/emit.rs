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

// ---------------------------------------------------------------- WIT types

#[derive(Clone, PartialEq)]
enum WitTy {
    Bool,
    Char,     // a Unicode scalar — i32 flat (u32 codepoint), carried in an int box
    IntS(u8), // s8/s16/s32 (byte width 1/2/4) — i32 flat, sign-extended into the int box
    IntU(u8), // u8/u16/u32 (byte width 1/2/4)
    S64,      // s64/u64 — i64 flat
    /// f32 — a BOUNDARY-ONLY representation (goal 5 / 5.2): one f32 flat,
    /// stored 4-byte in memory, but carried internally as the interpreter's
    /// f64 `Value::Dec` (promote on lift, demote on lower), exactly as the
    /// interpreter models every float as f64.
    F32,
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
            WitTy::Variant(cases) => Some(
                cases
                    .iter()
                    .map(|(n, p)| (n.as_str(), p.as_ref()))
                    .collect(),
            ),
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
///
/// `aliases` resolves a *named type alias* (`DefType pair list<s32>`,
/// `DefType coord tuple<s32, s32>`) to its underlying WIT type text. WIT only
/// allows a simple identifier for a functor element / a record field type, so a
/// `list`/`tuple`/`option`/`result` element must be named with a `DefType`; that
/// name lands here and `wit_ty` expands it before lowering (matching the
/// interpreter, which carries those values structurally regardless of the name).
#[derive(Default)]
struct TypeEnv {
    records: HashMap<String, Vec<(String, String)>>,
    defs: HashMap<String, TypeDef>,
    aliases: HashMap<String, String>,
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
            if a.is_empty() || a == "_" {
                Ok(None)
            } else {
                Ok(Some(wit_ty(a, env)?))
            }
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
                return Err(format!("`{s}`: a result takes at most two type arguments"));
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
        "s8" => WitTy::IntS(1),
        "s16" => WitTy::IntS(2),
        "s32" => WitTy::IntS(4),
        "u8" => WitTy::IntU(1),
        "u16" => WitTy::IntU(2),
        "u32" => WitTy::IntU(4),
        "s64" | "u64" => WitTy::S64,
        "f32" => WitTy::F32,
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
            } else if let Some(target) = env.aliases.get(other) {
                // A named alias to a compound WIT type (`list<…>`, `tuple<…>`,
                // `option<…>`, `result<…>`, or another alias). Expand to the
                // underlying type text and lower that — the value-level carriage
                // (TAG_LIST/TAG_TUP/TAG_VAR boxes) is the same one the interpreter
                // builds, so `eq_raw` dedups these structurally just like records.
                return wit_ty(target, env);
            } else {
                return Err(format!(
                    "type `{other}` not supported by the wasm backend yet"
                ));
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
        | WitTy::IntS(_)
        | WitTy::IntU(_)
        | WitTy::S64
        | WitTy::F32
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
        | WitTy::IntS(_)
        | WitTy::IntU(_)
        | WitTy::Handle
        | WitTy::Enum(_)
        | WitTy::Flags(_) => vec![ValType::I32],
        WitTy::S64 => vec![ValType::I64],
        WitTy::F32 => vec![ValType::F32],
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
        WitTy::IntS(w) | WitTy::IntU(w) => *w as u64,
        WitTy::F32 => 4,
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

/// Canonical-ABI alignment of a `flags` with `n` members: the size of the
/// smallest int that holds the bitset (1/2/4 bytes for ≤8/≤16/≤32), then
/// 4-byte alignment for the multi-word bitset. Matches `flags_size` and the
/// widths `store_to_mem`/`load_from_mem` read.
fn flags_align(n: usize) -> u64 {
    if n <= 8 {
        1
    } else if n <= 16 {
        2
    } else {
        4
    }
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
        WitTy::IntS(w) | WitTy::IntU(w) => *w as u64,
        WitTy::F32 => 4,
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

/// Whether `lit` is a duplicate-free subsequence of `decl` in declaration
/// order — the condition under which a flags literal's canonical bitset
/// (rebuilt in declaration order at the boundary) round-trips to the same
/// order-observable value the interpreter's `Value::Flg` holds.
fn flags_is_ordered_subseq(lit: &[String], decl: &[String]) -> bool {
    let mut di = 0usize;
    for name in lit {
        match decl[di..].iter().position(|d| d == name) {
            // advance PAST the matched member: enforces strictly increasing
            // positions, which rejects both reorderings and duplicates.
            Some(rel) => di += rel + 1,
            None => return false,
        }
    }
    true
}

fn align_up(off: u64, align: u64) -> u64 {
    off.div_ceil(align) * align
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
    matches!(ty, WitTy::IntU(1) | WitTy::IntS(1))
}

fn elem_size(ty: &WitTy) -> u64 {
    match ty {
        WitTy::Bool => 1,
        WitTy::Char | WitTy::Handle => 4,
        WitTy::IntS(w) | WitTy::IntU(w) => *w as u64,
        WitTy::F32 => 4,
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
            if flat_len(&ty) > 1 {
                Ok(FlatRes::Retptr)
            } else {
                Ok(FlatRes::One(ty))
            }
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
            if let Some(&head) = items.first()
                && let Node::Qsym(alias, name) = arena.node(head)
            {
                let key = (alias.clone(), name.clone());
                if !feats.dep_calls.contains(&key) {
                    feats.dep_calls.push(key);
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
    let nid = WitTy::IntU(4); // node-id = u32
    WitTy::Variant(vec![
        ("bool-val".into(), Some(WitTy::Bool)),
        ("int-val".into(), Some(WitTy::S64)),
        ("dec-val".into(), Some(WitTy::F64)),
        ("char-val".into(), Some(WitTy::Char)),
        ("str-val".into(), Some(WitTy::Str)),
        ("sym".into(), Some(WitTy::Str)),
        (
            "qsym".into(),
            Some(WitTy::Tuple(vec![WitTy::Str, WitTy::Str])),
        ),
        ("tup".into(), Some(WitTy::List(Box::new(nid.clone())))),
        ("lst".into(), Some(WitTy::List(Box::new(nid.clone())))),
        (
            "rec".into(),
            Some(WitTy::List(Box::new(WitTy::Tuple(vec![
                WitTy::Str,
                nid.clone(),
            ])))),
        ),
        ("flg".into(), Some(WitTy::List(Box::new(WitTy::Str)))),
    ])
}

/// The `wavelet:meta` `tree` record as a backend [`WitTy`].
fn meta_tree_wit_ty() -> WitTy {
    WitTy::Record(vec![
        ("nodes".into(), WitTy::List(Box::new(meta_node_wit_ty()))),
        ("root".into(), WitTy::IntU(4)),
        (
            "spans".into(),
            WitTy::List(Box::new(WitTy::Tuple(vec![WitTy::IntU(4), WitTy::IntU(4)]))),
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
    for (params, body) in info.defs.values() {
        let _ = params;
        scan(arena, *body, &mut feats);
    }
    for (_, expr) in &info.value_defs {
        scan(arena, *expr, &mut feats);
    }
    feats
}

/// Record types from a file's `DefType` forms: name → field (name, type-string).
/// Only record-shaped types are collected here; variants/flags go through
/// [`local_non_record_types`] (into `TypeEnv::defs`) and bare aliases (`list`,
/// `tuple`, …) into `TypeEnv::aliases`, so every `DefType` kind has a boundary
/// ABI — the layouts already exist (`WitTy::List`/`Tuple`/`Variant`/`Flags`).
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

/// Public: non-record named types a sibling dependency file defines —
/// variants/enums/flags as [`TypeDef`]s plus name → type-text aliases — so a
/// sibling `.wlt` dep carries the same type surface a parsed WIT dep does
/// (4.1/4.4: case constructors and alias expansion across the build set).
pub fn dep_non_record_types(
    arena: &Arena,
    info: &FileInfo,
) -> (Vec<(String, TypeDef)>, Vec<(String, String)>) {
    local_non_record_types(arena, &info.types)
}

/// Non-record local `DefType`s, split into the two `TypeEnv` channels:
///   * variants/flags become [`TypeDef`]s (keyed by name) — `Node::Lst` is a
///     `variant` (payload-less cases are an enum, the same as a variant with all
///     `None` payloads — and how `wit::type_decl` already renders them), and
///     `Node::Flg` is a `flags`. This mirrors what `witdep.rs` builds for *dep*
///     type_defs, so a local and an imported variant/flags lower identically.
///   * everything else `wit::type_text` can render — `list<…>`, `tuple<…>`,
///     `option<…>`, `result<…>`, or an alias to another named type — becomes an
///     *alias* (name → WIT type text), which `wit_ty` expands recursively.
///
/// Records are handled by [`record_types`] and skipped here. A `DefType` whose
/// body neither parses as a known kind nor renders to type text is left out
/// (any reference to it still surfaces the honest "not supported" error).
fn local_non_record_types(
    arena: &Arena,
    types: &[(String, NodeId)],
) -> (Vec<(String, TypeDef)>, Vec<(String, String)>) {
    let mut defs = Vec::new();
    let mut aliases = Vec::new();
    for (name, node) in types {
        match arena.node(*node) {
            Node::Rec(_) => {} // records: see `record_types`
            Node::Flg(names) => defs.push((name.clone(), TypeDef::Flags(names.clone()))),
            Node::Lst(cases) => {
                // A `[case …]` form is a variant; a payload carries as a Tup
                // `[head, payload…]` exactly as `wit::type_decl` reads it.
                let mut resolved = Vec::with_capacity(cases.len());
                let mut ok = true;
                for &c in cases {
                    match arena.node(c) {
                        Node::Sym(s) => resolved.push((s.clone(), None)),
                        Node::Tup(items) => {
                            let Some((&h, payload)) = items.split_first() else {
                                ok = false;
                                break;
                            };
                            let Node::Sym(case) = arena.node(h) else {
                                ok = false;
                                break;
                            };
                            // Multi-payload cases would need a tuple payload; the
                            // backend (like the variant ABI) carries one payload
                            // box, so only single-payload cases are supported.
                            match payload {
                                [] => resolved.push((case.clone(), None)),
                                [one] => match crate::wit::type_text(arena, *one) {
                                    Ok(t) => resolved.push((case.clone(), Some(t))),
                                    Err(_) => {
                                        ok = false;
                                        break;
                                    }
                                },
                                _ => {
                                    ok = false;
                                    break;
                                }
                            }
                        }
                        _ => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    // All-payload-less cases are a WIT `enum` (matching what
                    // `wit::type_decl` now synthesizes and what `witdep.rs`
                    // builds for dep enums); any payload makes it a `variant`.
                    // The flat ABI (a lone i32 discriminant) is identical.
                    if resolved.iter().all(|(_, p)| p.is_none()) {
                        let names = resolved.into_iter().map(|(n, _)| n).collect();
                        defs.push((name.clone(), TypeDef::Enum(names)));
                    } else {
                        defs.push((name.clone(), TypeDef::Variant(resolved)));
                    }
                }
            }
            // A bare alias: `list<…>`, `tuple<…>`, `option<…>`, `result<…>`, or a
            // name for another named type. Record its WIT type text for `wit_ty`.
            _ => {
                if let Ok(t) = crate::wit::type_text(arena, *node) {
                    aliases.push((name.clone(), t));
                }
            }
        }
    }
    (defs, aliases)
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

/// Whether `name` is a case of one of `dep`'s variant/enum types: `Some(true)`
/// for a payloaded variant case, `Some(false)` for a payload-less variant or
/// enum case, `None` when no type declares it (4.1).
fn dep_case(dep: &Dep, name: &str) -> Option<bool> {
    dep.type_defs.iter().find_map(|(_, def)| match def {
        TypeDef::Enum(cases) => cases.iter().any(|c| c == name).then_some(false),
        TypeDef::Variant(cases) => cases
            .iter()
            .find(|(c, _)| c == name)
            .map(|(_, p)| p.is_some()),
        _ => None,
    })
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
            "some" | "ok" | "err" => {
                // the argument(s) bundle into the variant payload, exactly as
                // the interpreter binds it. `ok()`/`err()` with no arguments
                // construct the payload-less case (4.2), like the interpreter.
                if args.is_empty() && name != "some" {
                    let addr = self.none_like_box(name);
                    fx.op(I::I32Const(addr as i32));
                    return Ok(());
                }
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
            "split" => {
                // s.split(sep) over UTF-8 bytes (oracle: want_str s, want_str
                // sep). Non-empty sep: scan left-to-right for non-overlapping
                // byte matches, emitting the run before each match and the final
                // tail (at most slen+1 pieces, so the list is over-allocated and
                // its length fixed at the end). Empty sep replicates Rust's
                // char-boundary split: a leading "", one piece per char, a
                // trailing "".
                nargs(2)?;
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
            other => {
                return Err(format!(
                    "builtin `{other}` not supported by the wasm backend yet"
                ));
            }
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

// --------------------------------------------------------- helper bodies

/// 5.8 pre-scan: collect the `Fn`-literal nodes that qualify for capture
/// devirtualization — an inline `Fn` bound directly as a `Let` binding init
/// whose checked type is a fully-concrete arrow (`Type::Fn`). Walks a def body
/// in deterministic pre-order; the caller reserves one core-function index per
/// returned node (in this order) so the lifted bodies land at known indices.
///
/// Only nodes whose enclosing def is actually emitted are scanned (the caller
/// passes internal-def bodies), so every reserved index gets exactly one body —
/// no dangling function declarations. A qualifying lambda the scan misses (in
/// an overload/value/resource body) simply falls back to the boxed closure.
fn scan_let_lambdas(
    arena: &Arena,
    id: NodeId,
    node_types: &crate::check::NodeTypes,
    out: &mut Vec<NodeId>,
) {
    let is_fn_arrow = |n: NodeId| -> bool {
        matches!(arena.node(n), Node::Tup(items)
            if !items.is_empty()
                && matches!(arena.node(items[0]), Node::Sym(s) if s == "fn-MACRO"))
            && matches!(node_types.get(&n), Some(crate::check::Type::Fn(..)))
    };
    if let Node::Tup(items) = arena.node(id)
        && items.len() >= 3
        && matches!(arena.node(items[0]), Node::Sym(s) if s == "let-MACRO")
        && let Node::Rec(fields) = arena.node(items[1])
    {
        for (_, v) in fields {
            if is_fn_arrow(*v) {
                out.push(*v);
            }
        }
    }
    match arena.node(id) {
        Node::Tup(items) | Node::Lst(items) => {
            for &c in items {
                scan_let_lambdas(arena, c, node_types, out);
            }
        }
        Node::Rec(fields) => {
            for (_, c) in fields {
                scan_let_lambdas(arena, *c, node_types, out);
            }
        }
        _ => {}
    }
}

fn emit_core_module(
    arena: &Arena,
    roots: &[NodeId],
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> Result<Vec<u8>, String> {
    let feats = features_of(arena, info);

    // Per-node static types (goal 5). Reconstruct the same import-signature
    // table the build step feeds the checker (`build.rs`), so this table
    // matches what checking saw — including qualified-call result types. A
    // program that fails checking yields an *empty* table (boxed fallback
    // everywhere) rather than a second error surface: the build path has
    // already rejected ill-typed programs before emitting.
    let node_types_tbl = {
        let mut import_sigs = crate::check::ImportSigs::new();
        for imp in &info.imports {
            if crate::wit::is_macro_only(imp) {
                continue;
            }
            if let Some(dep) = deps.get(&imp.package) {
                let iface = imp.path.split_once('/').map(|(_, i)| i).unwrap_or("api");
                import_sigs.extend(crate::wit::import_sigs_for(&imp.alias, &dep.funcs, iface));
            }
        }
        import_sigs.extend(crate::wit::functor_import_sigs(&info.functors));
        crate::check::node_types_with_imports(arena, roots, &import_sigs).unwrap_or_default()
    };

    // named types in scope: this file's own DefTypes, plus those of every dep.
    // Records resolve via `records`; enum/variant/flags (dep-only today) via
    // `defs`.
    let mut type_env = TypeEnv::default();
    for (name, fields) in record_types(arena, &info.types) {
        type_env.records.insert(name, fields);
    }
    // Local non-record `DefType`s: variants/flags as `defs`, list/tuple/option/
    // result/alias names as `aliases`. This lets a functor instantiate over a
    // named compound element (`DefType pair list<s32>` → `pair-set`) and lets an
    // export pass/return those types — matching the interpreter, which already
    // handles every element kind structurally.
    let (local_defs, local_aliases) = local_non_record_types(arena, &info.types);
    // The file's own variant/enum cases, for bare case construction (4.1).
    let mut local_cases: HashMap<String, bool> = HashMap::new();
    for (_name, def) in &local_defs {
        match def {
            TypeDef::Enum(cases) => {
                for c in cases {
                    local_cases.insert(c.clone(), false);
                }
            }
            TypeDef::Variant(cases) => {
                for (c, p) in cases {
                    local_cases.insert(c.clone(), p.is_some());
                }
            }
            _ => {}
        }
    }
    for (name, def) in local_defs {
        type_env.defs.insert(name, def);
    }
    for (name, target) in local_aliases {
        type_env.aliases.insert(name, target);
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
        for (name, target) in &dep.aliases {
            type_env
                .aliases
                .entry(name.clone())
                .or_insert_with(|| target.clone());
        }
    }
    // Each functor instantiation exports a `set` resource. Declaring `set` as a
    // `TypeDef::Resource` makes `wit_ty` map a bare `set` (and `own<set>` /
    // `borrow<set>`) to `WitTy::Handle` at the boundary — `emit.rs:177`–`184`,
    // `:260`. All instantiations name their resource `set` (interfaces differ:
    // `point-set`, `string-set`, …); since the element type is the only thing
    // that varies and the rep is element-generic, one `set -> Resource` entry
    // serves every instantiation. The element type itself resolves through the
    // same `type_env` (records via `type_env.records`, primitives intrinsically).
    for inst in &info.functors {
        // The bare resource name, as it appears in a method signature
        // (`add: func(value: point)` → receiver `set`).
        type_env
            .defs
            .entry("set".to_string())
            .or_insert(TypeDef::Resource);
        // The dotted `<iface>.set` form is the *return type text* an export body
        // gets for `alias/new()` (see `wit::functor_op_table`: a `Handle` op
        // infers to `"{iface}.set"`). Register it as a resource too so the export
        // wrapper's `flat_result`/`wit_ty` map it to `WitTy::Handle` rather than
        // rejecting it as an unknown type.
        type_env
            .defs
            .entry(format!("{}.set", inst.iface))
            .or_insert(TypeDef::Resource);
    }

    // User-declared resources (4.5): register each resource NAME as a boundary
    // resource type so `wit_ty` maps `counter` / `own(counter)` / `borrow(counter)`
    // to a handle, exactly as for the functor `set`. The classified member forms
    // drive index reservation, body emission, call routing, and boundary exports.
    let user_res_forms = user_resource_forms(arena, roots)?;
    for rf in &user_res_forms {
        type_env
            .defs
            .entry(rf.name.clone())
            .or_insert(TypeDef::Resource);
    }

    let mut em = Emitter {
        arena,
        info,
        deps,
        type_env,
        local_cases,
        data: Vec::new(),
        str_cache: HashMap::new(),
        types: Vec::new(),
        imports: Vec::new(),
        import_fn: HashMap::new(),
        h: Helpers::default(),
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
        node_types: node_types_tbl,
        mem_tys: Vec::new(),
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
    let mut add_import =
        |em: &mut Emitter, module: &str, field: &str, p: Vec<ValType>, r: Vec<ValType>| {
            let t = em.ty_idx(p, r);
            em.imports.push((module.to_string(), field.to_string(), t));
            em.import_fn
                .insert((module.to_string(), field.to_string()), n_imports);
            n_imports += 1;
        };

    use ValType::{F64, I32, I64};
    let _ = (I32, I64, F64);
    for (alias, fname) in &feats.dep_calls {
        // A functor alias (`pts` in `pts/new`) is NOT a runtime import — its
        // `set` resource is *exported*, not imported, so it contributes no import
        // here. Routing the call itself is step 04; the body's `dep_call` stubs it
        // for now. Skip it so the import-declaration loop does not reject it as an
        // unknown import alias.
        if info.functors.iter().any(|f| &f.alias == alias) {
            continue;
        }
        // A user-resource method/static (`counter/next`, `counter/sum`) is an
        // exported resource member, not a runtime import; `dep_call` routes it to
        // the emitted member fn. Skip it here for the same reason as a functor.
        if user_res_forms.iter().any(|rf| &rf.name == alias) {
            continue;
        }
        let imp = info
            .imports
            .iter()
            .find(|i| &i.alias == alias)
            .ok_or(format!("unknown import alias `{alias}`"))?;
        let dep = deps.get(&imp.package).ok_or(format!(
            "dependency `{}` is not in the build set",
            imp.package
        ))?;
        let iface = import_iface(&imp.path);
        // Same op-name resolution as `dep_call`, so a resource operation's
        // core import is declared under its mangled WIT name (`sig.name`).
        // A dep *case constructor* call (`t/circle(…)`, 4.1) is not a runtime
        // import — the variant box is built locally — so it declares nothing.
        let sig = match resolve_dep_func(dep, &iface, fname) {
            Ok(sig) => sig,
            Err(e) => {
                if dep_case(dep, fname).is_some() {
                    continue;
                }
                return Err(e);
            }
        };
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

    // ---- functor resource-intrinsic imports (one triple per instantiation)
    //
    // For every `Set` functor instantiation the encoder synthesizes three
    // resource intrinsics — `[resource-new/rep/drop]set` — under the module
    // string `[export]<versioned specialized iface>` (summary 01 §2). They MUST
    // be declared here, in the imports-first index block, so the function index
    // space stays imports-first; their indices are captured for `emit_set_resource`
    // (which the constructor calls `resource.new` through). The intrinsics only
    // exist because the synthesized world *exports* the resource interface.
    let mut functor_intrinsics: Vec<(u32, u32, u32)> = Vec::new();
    for inst in &info.functors {
        let module = format!("[export]{}", versioned_iface(&info.package, &inst.iface));
        add_import(&mut em, &module, "[resource-new]set", vec![I32], vec![I32]);
        add_import(&mut em, &module, "[resource-rep]set", vec![I32], vec![I32]);
        add_import(&mut em, &module, "[resource-drop]set", vec![I32], vec![]);
        let new_i = em.import_idx(&module, "[resource-new]set");
        let rep_i = em.import_idx(&module, "[resource-rep]set");
        let drop_i = em.import_idx(&module, "[resource-drop]set");
        functor_intrinsics.push((new_i, rep_i, drop_i));
    }

    // ---- user-resource resource-intrinsic imports (one triple per resource)
    //
    // `[resource-new/rep/drop]<name>` under `[export]<versioned export iface>`,
    // exactly as for the functor `set`. Only exported resources are supported by
    // the backend; an unexported `DefResource` has no boundary interface to hang
    // the intrinsics on.
    let mut user_res_intrinsics: HashMap<String, (u32, u32, u32)> = HashMap::new();
    for rf in &user_res_forms {
        let Some(decl) = info.resources.iter().find(|r| r.name == rf.name) else {
            continue;
        };
        let Some(iface_path) = &decl.iface else {
            return Err(format!(
                "resource `{}` is declared but not exported; the wasm backend only \
                 supports exported resources yet",
                rf.name
            ));
        };
        let iface = if is_external_iface(iface_path) {
            external_versioned_in(iface_path, deps)
        } else {
            versioned_iface(&info.package, iface_path)
        };
        let module = format!("[export]{iface}");
        let nnew = format!("[resource-new]{}", rf.name);
        let nrep = format!("[resource-rep]{}", rf.name);
        let ndrop = format!("[resource-drop]{}", rf.name);
        add_import(&mut em, &module, &nnew, vec![I32], vec![I32]);
        add_import(&mut em, &module, &nrep, vec![I32], vec![I32]);
        add_import(&mut em, &module, &ndrop, vec![I32], vec![]);
        let ni = em.import_idx(&module, &nnew);
        let ri = em.import_idx(&module, &nrep);
        let di = em.import_idx(&module, &ndrop);
        user_res_intrinsics.insert(rf.name.clone(), (ni, ri, di));
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
    em.h.unbox_char = take();
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
    em.h.arith_int = take();
    em.h.cmp_f64 = take();
    em.h.persist_alloc = take();
    em.h.persist = take();

    // ---- reserve the functor `set` resource core-func indices (step 04)
    //
    // The five resource funcs (ctor/add/contains/size/dtor) per instantiation are
    // EMITTED just after `emit_helpers` below, so in `em.bodies` they sit directly
    // after the helper bodies and before the internal/export bodies. Reserve their
    // indices here, in that exact position in the `take()` sequence, so the indices
    // recorded in `em.functor_fns` match where `emit_set_resource` will self-index
    // them — and so the internal/overload/export `take()`s that follow are shifted
    // past the resource slots. `dep_call` reads `em.functor_fns` to route an
    // `alias/op` call while the internal/export bodies (which contain those calls)
    // are still being lowered, i.e. before the resource bodies exist.
    for (inst, &(new_i, rep_i, drop_i)) in info.functors.iter().zip(&functor_intrinsics) {
        let ctor = take();
        let add = take();
        let contains = take();
        let size = take();
        let dtor = take();
        em.functor_fns.insert(
            inst.alias.clone(),
            ResourceFns {
                ctor,
                add,
                contains,
                size,
                dtor,
                new_import: new_i,
                rep_import: rep_i,
                drop_import: drop_i,
            },
        );
    }

    // ---- reserve user-resource member fn indices (4.5)
    //
    // Reserve indices for each resource's constructor, methods, statics, and dtor
    // in the same take() position their bodies are emitted (right after the functor
    // `set` bodies), so indices line up. The constructor is registered under the
    // bare resource name and each method/static under `name/op`, so ordinary call
    // resolution (the `Sym`/`Qsym` arms) finds them; routing data goes in `user_res`.
    for rf in &user_res_forms {
        let (_ni, _ri, _di) = user_res_intrinsics[&rf.name];
        if let Some((pid, _)) = rf.ctor {
            let names = param_names(arena, pid)?;
            let n = names.len();
            let idx = take();
            em.funcs.insert(
                rf.name.clone(),
                (idx, names, FnSig { params: vec![Repr::Boxed; n], result: Repr::Boxed }),
            );
        }
        let mut methods = std::collections::HashSet::new();
        for (key, pid, _) in &rf.methods {
            let names = param_names(arena, *pid)?;
            let n = names.len();
            let idx = take();
            em.funcs.insert(
                format!("{}/{}", rf.name, key),
                (idx, names, FnSig { params: vec![Repr::Boxed; n], result: Repr::Boxed }),
            );
            methods.insert(key.clone());
        }
        let mut statics = std::collections::HashSet::new();
        for (key, pid, _) in &rf.statics {
            let names = param_names(arena, *pid)?;
            let n = names.len();
            let idx = take();
            em.funcs.insert(
                format!("{}/{}", rf.name, key),
                (idx, names, FnSig { params: vec![Repr::Boxed; n], result: Repr::Boxed }),
            );
            statics.insert(key.clone());
        }
        let dtor = take();
        em.user_res.insert(rf.name.clone(), UserRes { methods, statics, dtor });
    }

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
        if let Node::Tup(items) = arena.node(root)
            && items.len() >= 2
            && matches!(arena.node(items[0]), Node::Sym(s) if s == "def-MACRO")
            && let Node::Sym(name) = arena.node(items[1])
            && info.defs.contains_key(name)
            && !overloaded_origins.contains(name)
            && !internal_order.contains(name)
        {
            internal_order.push(name.clone());
        }
    }
    for (i, (name, _)) in info.value_defs.iter().enumerate() {
        em.value_globals.insert(name.clone(), 1 + i as u32); // global 0 = heap ptr
    }
    for name in &internal_order {
        let (params_id, body) = info.defs[name];
        let params = param_names(arena, params_id)?;
        let sig = em.def_sig(params_id, body);
        em.funcs.insert(name.clone(), (take(), params, sig));
    }
    // Mangled overload members get their own internal-function indices.
    for mangled in &overload_order {
        let (params_id, body) = info.overload_bodies[mangled];
        let params = param_names(arena, params_id)?;
        let sig = em.def_sig(params_id, body);
        em.funcs.insert(mangled.clone(), (take(), params, sig));
    }

    // ---- reserve lifted-lambda indices (5.8 Fn-literal capture devirt)
    //
    // Scan every internal def body for `Fn`-literal Let inits with a concrete
    // arrow type and reserve one core-function index per hit, HERE — right after
    // the overload block and before the export wrappers (which `take()` their
    // own indices later). The lifted bodies are pushed into `em.bodies` between
    // the overload bodies and the export wrappers, in this same reservation
    // order, so positions match indices (asserted at the push site).
    for name in &internal_order {
        let (_, body) = info.defs[name];
        scan_let_lambdas(arena, body, &em.node_types, &mut em.lambda_order);
    }
    for &node in &em.lambda_order.clone() {
        let idx = take();
        em.lambda_reserved.insert(node, idx);
    }

    // ---- helper bodies (order must match index assignment above)
    emit_helpers(&mut em)?;

    // ---- functor `set` resource bodies (step 02 bodies; emitted here in step 04)
    //
    // Emitted right after the helpers and BEFORE the internal/export bodies, so
    // each resource's five `em.bodies` positions line up with the indices reserved
    // in `em.functor_fns` above. `dep_call` routes `alias/op` calls to those
    // reserved indices; the canonical export NAMES are registered later (alongside
    // the export wrappers). The element type is resolved through the boundary
    // `type_env`; `flat_checked` inside `emit_set_resource` rejects any element the
    // backend can't flatten with an honest error.
    for (inst, &(new_i, rep_i, drop_i)) in info.functors.iter().zip(&functor_intrinsics) {
        let elem = wit_ty(&inst.elem, &em.type_env).map_err(|e| {
            format!(
                "functor `set` over `{}` (alias `{}`): {e}",
                inst.elem, inst.alias
            )
        })?;
        let fns = emit_set_resource(&mut em, inst, &elem, new_i, rep_i, drop_i)?;
        debug_assert_eq!(
            em.functor_fns.get(&inst.alias).copied(),
            Some(fns),
            "reserved functor `set` indices must match the emitted body positions"
        );
    }

    // ---- user-resource member bodies (4.5)
    //
    // Emitted right after the functor `set` bodies so positions match the indices
    // reserved above (constructor, methods, statics, dtor per resource, in that
    // order). A resource *value* is carried guest-internally as its REP (the cell
    // box the `New` body returns) — the same thing the interpreter binds `self`
    // to and that `cell-get`/`cell-set` operate on. So the constructor, methods,
    // and statics are ORDINARY internal bodies over reps; the own/borrow handle
    // conversions (`resource.new`/`resource.rep`) happen only at the boundary.
    // The dtor runs `Drop` (rep = param 0) or no-ops.
    for rf in &user_res_forms {
        let mut member_bodies: Vec<(String, NodeId)> = Vec::new();
        if let Some((_pid, body)) = rf.ctor {
            member_bodies.push((rf.name.clone(), body));
        }
        for (key, _pid, body) in rf.methods.iter().chain(rf.statics.iter()) {
            member_bodies.push((format!("{}/{}", rf.name, key), *body));
        }
        for (fname, body) in member_bodies {
            let (_, params, sig) = em.funcs[&fname].clone();
            let mut fx = FnCtx::new(params.len() as u32);
            let mut scope = HashMap::new();
            for (i, pn) in params.iter().enumerate() {
                scope.insert(pn.clone(), Binding::new(i as u32, sig.params[i] ));
            }
            fx.scopes.push(scope);
            em.expr_repr(&mut fx, body, sig.result, true)
                .map_err(|e| format!("in resource member `{fname}`: {e}"))?;
            let t = em.ty_idx(sig.param_vts(), vec![repr_vt(sig.result)]);
            em.bodies.push((t, fx.finish()));
        }
        {
            let mut fx = FnCtx::new(1);
            if let Some((pid, body)) = rf.drop_member {
                let mut scope = HashMap::new();
                if let Some(pn) = param_names(arena, pid)?.first() {
                    scope.insert(pn.clone(), Binding::new(0, Repr::Boxed ));
                }
                fx.scopes.push(scope);
                em.expr_repr(&mut fx, body, Repr::Boxed, false)
                    .map_err(|e| format!("in resource `{}` Drop: {e}", rf.name))?;
                fx.op(I::Drop); // discard the unit result
            }
            let t = em.ty_idx(vec![ValType::I32], vec![]);
            em.bodies.push((t, fx.finish()));
        }
    }

    // ---- internal function bodies (typed per their repr signature — 5.2)
    for name in &internal_order {
        let (_, body) = info.defs[name];
        let (_, params, sig) = em.funcs[name].clone();
        let n = params.len();
        let mut fx = FnCtx::new(n as u32);
        let mut scope = HashMap::new();
        for (i, p) in params.iter().enumerate() {
            scope.insert(
                p.clone(),
                Binding::new(i as u32, sig.params[i]),
            );
        }
        fx.scopes.push(scope);
        em.expr_repr(&mut fx, body, sig.result, true)
            .map_err(|e| format!("in `{name}`: {e}"))?;
        let t = em.ty_idx(sig.param_vts(), vec![repr_vt(sig.result)]);
        em.bodies.push((t, fx.finish()));
    }
    // ---- mangled overload member bodies (same paired order as their indices)
    for mangled in &overload_order {
        let (_, body) = info.overload_bodies[mangled];
        let (_, params, sig) = em.funcs[mangled].clone();
        let n = params.len();
        let mut fx = FnCtx::new(n as u32);
        let mut scope = HashMap::new();
        for (i, p) in params.iter().enumerate() {
            scope.insert(
                p.clone(),
                Binding::new(i as u32, sig.params[i]),
            );
        }
        fx.scopes.push(scope);
        em.expr_repr(&mut fx, body, sig.result, true)
            .map_err(|e| format!("in `{mangled}`: {e}"))?;
        let t = em.ty_idx(sig.param_vts(), vec![repr_vt(sig.result)]);
        em.bodies.push((t, fx.finish()));
    }

    // ---- lifted lambda bodies (5.8 Fn-literal capture devirtualization)
    //
    // Pushed here — after the overload bodies, before the export wrappers — so
    // each body's position matches the index reserved for it right after the
    // overload block (asserted below). Every reserved node was compiled during
    // its enclosing internal def body (`compile_known_lambda` stashed it), so
    // the stash is complete.
    for node in em.lambda_order.clone() {
        let (t, f) = em
            .lambda_stash
            .remove(&node)
            .ok_or_else(|| format!("internal: lifted lambda body for node {node} missing"))?;
        debug_assert_eq!(
            n_imports + em.bodies.len() as u32,
            em.lambda_reserved[&node],
            "lifted lambda body position must match its reserved index"
        );
        em.bodies.push((t, f));
    }

    // ---- export wrappers
    let mut exports: Vec<(String, u32)> = Vec::new(); // (export name, fn idx)
    // Each export's (full export name, flat results) — a canonical post-return
    // companion is emitted per entry after this loop (5.1 arena-per-call).
    let mut post_returns: Vec<(String, Vec<ValType>)> = Vec::new();
    // Classify a boundary type text as a user-resource handle (4.5): a resource
    // value crosses the boundary as an own handle (`counter`) or a borrow
    // (`borrow<counter>`), but is carried guest-internally as its rep. `own`
    // params/results convert with `resource.rep`/`resource.new`; a `borrow` param
    // already arrives AS the rep (the canonical ABI hands a borrow of a
    // self-owned resource the rep directly). The functor `set` (handle carriage)
    // is deliberately excluded — it is not in `user_res_intrinsics`.
    let classify_res = |t: &str| -> Option<(String, bool)> {
        if let Some(inner) = t.strip_prefix("borrow<").and_then(|x| x.strip_suffix('>'))
            && user_res_intrinsics.contains_key(inner)
        {
            return Some((inner.to_string(), true));
        }
        let bare = t
            .strip_prefix("own<")
            .and_then(|x| x.strip_suffix('>'))
            .unwrap_or(t);
        if user_res_intrinsics.contains_key(bare) {
            return Some((bare.to_string(), false));
        }
        None
    };
    for sig in &info.exports {
        let (fidx, _, fsig) = em
            .funcs
            .get(&sig.name)
            .cloned()
            .ok_or(format!("export `{}` has no Def Fn", sig.name))?;
        let mut fparams = Vec::new();
        let mut lifted: Vec<(WitTy, u32, Option<(String, bool)>)> = Vec::new();
        for (_, t) in &sig.params {
            let ty = wit_ty(t, &em.type_env)?;
            let res = classify_res(t);
            lifted.push((ty.clone(), fparams.len() as u32, res));
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
        // 5.2 export fast path: a typed def param whose WIT type is the same
        // scalar kind receives the flat ABI value DIRECTLY (extend/promote),
        // skipping the box entirely; everything else lifts to a box as
        // before (plus one unbox if the def param is typed anyway).
        let typed_ok = fsig.params.len() == lifted.len();
        for (i, (ty, base, res)) in lifted.iter().enumerate() {
            if let Some((rname, is_borrow)) = res {
                // own: `resource.rep(handle) -> rep`; borrow: the flat i32 already
                // IS the rep box pointer.
                fx.op(I::LocalGet(*base));
                if !is_borrow {
                    let (_n, rep_i, _d) = user_res_intrinsics[rname];
                    fx.op(I::Call(rep_i));
                }
                continue;
            }
            let slot = if typed_ok {
                fsig.params[i]
            } else {
                Repr::Boxed
            };
            match (slot, wit_scalar(ty)) {
                (Repr::Scalar(k), Some(wk)) if k == wk => {
                    fx.op(I::LocalGet(*base));
                    match ty {
                        WitTy::Bool => {
                            // normalize to 0/1, like the interpreter's Bool
                            fx.op(I::I32Const(0));
                            fx.op(I::I32Ne);
                        }
                        WitTy::IntS(_) => fx.op(I::I64ExtendI32S),
                        WitTy::IntU(_) | WitTy::Char => fx.op(I::I64ExtendI32U),
                        WitTy::S64 | WitTy::F64 => {}
                        WitTy::F32 => fx.op(I::F64PromoteF32),
                        _ => unreachable!("wit_scalar admitted a non-scalar"),
                    }
                }
                (Repr::Scalar(k), _) => {
                    em.lift_flat(&mut fx, ty, *base)?;
                    em.unbox_scalar(&mut fx, k);
                }
                (Repr::Boxed, _) => em.lift_flat(&mut fx, ty, *base)?,
                (Repr::Mem(_), _) => {
                    return Err("internal: Mem def params are not emitted yet (5.3)".into());
                }
            }
        }
        fx.op(I::Call(fidx));
        // the call produced the def's result repr; the paths below expect
        // the value they were written for (flat scalar or box)
        let res_kind = if typed_ok { fsig.result } else { Repr::Boxed };
        let fresults = match flat_result(sig, &em.type_env)? {
            FlatRes::None => {
                fx.op(I::Drop);
                vec![]
            }
            FlatRes::One(t) if classify_res(sig.result.as_deref().unwrap_or("")).is_some() => {
                // A user-resource own result: the def returned the rep; mint the
                // own handle to hand out across the boundary.
                let (rname, _) = classify_res(sig.result.as_deref().unwrap()).unwrap();
                let (new_i, _r, _d) = user_res_intrinsics[&rname];
                if let Repr::Scalar(k) = res_kind {
                    em.box_scalar(&mut fx, k);
                }
                fx.op(I::Call(new_i));
                flat(&t)
            }
            FlatRes::One(t) => {
                match res_kind {
                    // typed result, same scalar kind as the WIT type: convert
                    // directly to the flat form — no box was ever built
                    Repr::Scalar(k) if wit_scalar(&t) == Some(k) => match t {
                        WitTy::Bool | WitTy::S64 | WitTy::F64 => {}
                        WitTy::IntS(_) | WitTy::IntU(_) | WitTy::Char => fx.op(I::I32WrapI64),
                        WitTy::F32 => fx.op(I::F32DemoteF64),
                        _ => unreachable!("wit_scalar admitted a non-scalar"),
                    },
                    // typed result but a non-scalar/mismatched WIT type:
                    // box at the seam and lower as before
                    Repr::Scalar(k) => {
                        em.box_scalar(&mut fx, k);
                        em.lower(&mut fx, &t)?;
                    }
                    Repr::Boxed => em.lower(&mut fx, &t)?,
                    Repr::Mem(mt) => {
                        let l = fx.local(I32);
                        fx.op(I::LocalSet(l));
                        let mty = em.mem_tys[mt as usize].clone();
                        em.load_from_mem(&mut fx, &mty, l, 0)?;
                        em.lower(&mut fx, &t)?;
                    }
                }
                flat(&t)
            }
            FlatRes::Retptr => {
                let ty = wit_ty(sig.result.as_deref().unwrap(), &em.type_env)?;
                // 5.3 fast path: the def's result already lives in canonical
                // layout of exactly the boundary type — the pointer on the
                // stack IS the callee-owned result area. No box, no copy.
                if matches!(res_kind, Repr::Mem(mt) if em.mem_tys[mt as usize] == ty) {
                    let t = em.ty_idx(fparams, vec![I32]);
                    em.bodies.push((t, fx.finish()));
                    let own_iface = if is_external_iface(&sig.iface) {
                        external_versioned_in(&sig.iface, deps)
                    } else {
                        versioned_iface(&info.package, &sig.iface)
                    };
                    let export_name = format!("{own_iface}#{}", sig.name);
                    post_returns.push((export_name.clone(), vec![I32]));
                    exports.push((export_name, take()));
                    continue;
                }
                // compound results are boxed; a (mismatched) typed scalar or
                // canonical result reboxes at the seam first
                match res_kind {
                    Repr::Boxed => {}
                    Repr::Scalar(k) => em.box_scalar(&mut fx, k),
                    Repr::Mem(mt) => {
                        let l = fx.local(I32);
                        fx.op(I::LocalSet(l));
                        let mty = em.mem_tys[mt as usize].clone();
                        em.load_from_mem(&mut fx, &mty, l, 0)?;
                    }
                }
                let area = fx.local(I32);
                if matches!(
                    ty,
                    WitTy::Record(_)
                        | WitTy::Tuple(_)
                        | WitTy::Option(_)
                        | WitTy::Result(..)
                        | WitTy::Variant(_)
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
        let t = em.ty_idx(fparams, fresults.clone());
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
        let export_name = format!("{own_iface}#{}", sig.name);
        post_returns.push((export_name.clone(), fresults));
        exports.push((export_name, take()));
    }

    // ---- per-call arena reset via canonical post-return (5.1)
    //
    // The memory story decided on 5.1 (arena-per-call, headerless): every
    // export gets a `cabi_post_<export-name>` companion, which wit-component
    // wires as the export's post-return function and the host runtime invokes
    // once the caller has finished reading the results. At that point nothing
    // allocated during the call is live — the lowered arguments, the call's
    // temporaries, and the result area are all dead — so the bump pointer
    // resets to the arena floor (the heap base) and the lazily-cached
    // value-def globals clear to recompute on the next call (their cached
    // boxes died with the arena; value defs are pure in the backend, so
    // recomputation is semantics-neutral). Interned statics live below the
    // heap base and are untouched.
    //
    // Components with functor instantiations OR user-declared resources (4.5)
    // used to OPT OUT of the reset and keep never-free behaviour, because a
    // resource's rep — the `set` cell and its stored list, or a user `counter`'s
    // `New` cell — must survive across export calls. As of the 5.1 evacuation
    // that state now lives in the PERSISTENT region below the arena floor
    // (`persist_alloc` + the `persist` write barrier, wired into cell-new/set and
    // the functor `set` ctor/add), which the reset does not touch — so every
    // component, resource-bearing or not, resets its arena at post-return.
    //
    // Global indices: 0 = arena bump ptr, 1..=n value defs, 1+n gensym counter,
    // 2+n = the arena floor (heap_base + persistent reserve for resource
    // components, heap_base otherwise), 3+n = the persistent bump ptr.
    let arena_floor_g = 2 + info.value_defs.len() as u32;
    for (name, fresults) in post_returns {
        let mut fx = FnCtx::new(fresults.len() as u32);
        fx.op(I::GlobalGet(arena_floor_g));
        fx.op(I::GlobalSet(0));
        for i in 0..info.value_defs.len() {
            fx.op(I::I32Const(0));
            fx.op(I::GlobalSet(1 + i as u32));
        }
        let t = em.ty_idx(fresults, vec![]);
        em.bodies.push((t, fx.finish()));
        exports.push((format!("cabi_post_{name}"), take()));
    }

    // ---- user-resource boundary exports (4.5)
    //
    // Emit the canonical resource-ABI boundary functions per exported resource:
    // `[constructor]`/`[method]`/`[static]` wrappers that lift the flat params to
    // boxes, call the internal member fn, and lower the boxed result back to the
    // flat ABI; the `[dtor]` body was emitted above and is exported directly. A
    // constructor returns the own handle (unbox the internal fn's boxed handle
    // value); a method receives the rep as core param 0 (already a box pointer).
    for rf in &user_res_forms {
        let Some(decl) = info.resources.iter().find(|r| r.name == rf.name) else {
            continue;
        };
        let Some(iface_path) = &decl.iface else {
            continue;
        };
        let iface = if is_external_iface(iface_path) {
            external_versioned_in(iface_path, deps)
        } else {
            versioned_iface(&info.package, iface_path)
        };
        let (new_i, _rep_i, _drop_i) = user_res_intrinsics[&rf.name];
        if let Some(ctor_params) = &decl.constructor {
            let ctor_idx = em.funcs[&rf.name].0;
            let mut fparams = Vec::new();
            let mut lifted: Vec<(WitTy, u32)> = Vec::new();
            for (_, t) in ctor_params {
                let ty = wit_ty(t, &em.type_env)?;
                lifted.push((ty.clone(), fparams.len() as u32));
                fparams.extend_from_slice(&flat_checked(&ty)?);
            }
            let mut fx = FnCtx::new(fparams.len() as u32);
            for (ty, base) in &lifted {
                em.lift_flat(&mut fx, ty, *base)?;
            }
            fx.op(I::Call(ctor_idx)); // -> rep pointer (the New cell)
            fx.op(I::Call(new_i)); // mint an own handle from the rep
            let t = em.ty_idx(fparams, vec![I32]);
            em.bodies.push((t, fx.finish()));
            exports.push((format!("{iface}#[constructor]{}", rf.name), take()));
        }
        for m in &decl.methods {
            let midx = em.funcs[&format!("{}/{}", rf.name, m.name)].0;
            let mut fparams = vec![I32]; // rep (self)
            let mut lifted: Vec<(WitTy, u32)> = Vec::new();
            for (_, t) in &m.params {
                let ty = wit_ty(t, &em.type_env)?;
                lifted.push((ty.clone(), fparams.len() as u32));
                fparams.extend_from_slice(&flat_checked(&ty)?);
            }
            let mut fx = FnCtx::new(fparams.len() as u32);
            fx.op(I::LocalGet(0)); // rep as the boxed `self` argument
            for (ty, base) in &lifted {
                em.lift_flat(&mut fx, ty, *base)?;
            }
            fx.op(I::Call(midx));
            let fresults = match flat_result(m, &em.type_env)? {
                FlatRes::None => {
                    fx.op(I::Drop);
                    vec![]
                }
                FlatRes::One(WitTy::Handle) => {
                    fx.op(I::Call(new_i)); // rep -> own handle
                    vec![I32]
                }
                FlatRes::One(t) => {
                    em.lower(&mut fx, &t)?;
                    flat(&t)
                }
                FlatRes::Retptr => {
                    return Err(format!(
                        "resource `{}` method `{}` returns a memory-spilled type; \
                         not supported by the wasm backend yet",
                        rf.name, m.name
                    ));
                }
            };
            let t = em.ty_idx(fparams, fresults);
            em.bodies.push((t, fx.finish()));
            exports.push((format!("{iface}#[method]{}.{}", rf.name, m.name), take()));
        }
        for st in &decl.statics {
            let sidx = em.funcs[&format!("{}/{}", rf.name, st.name)].0;
            let mut fparams = Vec::new();
            let mut lifted: Vec<(WitTy, u32)> = Vec::new();
            for (_, t) in &st.params {
                let ty = wit_ty(t, &em.type_env)?;
                lifted.push((ty.clone(), fparams.len() as u32));
                fparams.extend_from_slice(&flat_checked(&ty)?);
            }
            let mut fx = FnCtx::new(fparams.len() as u32);
            for (ty, base) in &lifted {
                em.lift_flat(&mut fx, ty, *base)?;
            }
            fx.op(I::Call(sidx));
            let fresults = match flat_result(st, &em.type_env)? {
                FlatRes::None => {
                    fx.op(I::Drop);
                    vec![]
                }
                FlatRes::One(WitTy::Handle) => {
                    fx.op(I::Call(new_i)); // rep -> own handle
                    vec![I32]
                }
                FlatRes::One(t) => {
                    em.lower(&mut fx, &t)?;
                    flat(&t)
                }
                FlatRes::Retptr => {
                    return Err(format!(
                        "resource `{}` static `{}` returns a memory-spilled type; \
                         not supported by the wasm backend yet",
                        rf.name, st.name
                    ));
                }
            };
            let t = em.ty_idx(fparams, fresults);
            em.bodies.push((t, fx.finish()));
            exports.push((format!("{iface}#[static]{}.{}", rf.name, st.name), take()));
        }
        exports.push((
            format!("{iface}#[dtor]{}", rf.name),
            em.user_res[&rf.name].dtor,
        ));
    }

    let _ = take; // `next`/`take` are done; the resource bodies were emitted above.

    // ---- functor `set` resource EXPORTS (canonical names) — one per instantiation
    //
    // The five core funcs were already emitted right after the helpers (so their
    // indices match `em.functor_fns`). Here we only register their canonical
    // resource-ABI export names, prefixed by the versioned specialized interface
    // (`demo:geo/point-set@0.1.0`); the index for each comes from the reserved
    // `em.functor_fns`. `ExportSection` later writes these verbatim, same as any
    // ordinary export.
    for inst in &info.functors {
        let fns = em.functor_fns[&inst.alias];
        let iface = versioned_iface(&info.package, &inst.iface);
        exports.push((format!("{iface}#[constructor]set"), fns.ctor));
        exports.push((format!("{iface}#[method]set.add"), fns.add));
        exports.push((format!("{iface}#[method]set.contains"), fns.contains));
        exports.push((format!("{iface}#[method]set.size"), fns.size));
        exports.push((format!("{iface}#[dtor]set"), fns.dtor));
    }

    // ---- assemble
    let heap_base = {
        em.align8();
        DATA_BASE + em.data.len() as u32
    };
    // 5.1: resource/functor components reserve a persistent region [heap_base,
    // arena_floor) for resource state that outlives the per-call arena reset;
    // the arena bump pointer starts at (and resets to) `arena_floor`. Other
    // components reserve nothing, so arena_floor == heap_base as before.
    let has_persist = !info.functors.is_empty() || !info.resources.is_empty();
    let persist_reserve = if has_persist { PERSIST_RESERVE } else { 0 };
    let arena_floor = heap_base + persist_reserve;
    let pages = (arena_floor as u64 >> 16) + 1;

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
    // global 0: the arena bump pointer. Starts at the arena floor (above the
    // persistent reserve for resource components; == heap_base otherwise).
    gs.global(
        GlobalType {
            val_type: I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(arena_floor as i32),
    );
    for _ in &info.value_defs {
        gs.global(
            GlobalType {
                val_type: I32,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i32_const(0),
        );
    }
    // The `gensym` counter (always present): an i64 incremented once per
    // `gensym` call, so fresh symbols are unique and deterministic across every
    // expansion in one component instance. Index = 1 + value_defs.len() (global
    // 0 is the heap pointer, then one i32 per value def).
    gs.global(
        GlobalType {
            val_type: ValType::I64,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i64_const(0),
    );
    // The arena floor (5.1 arena-per-call): the base the per-export post-return
    // bodies reset the arena bump pointer to. For a resource/functor component
    // it sits above the persistent reserve; otherwise it is the heap base.
    // Index = 2 + value_defs.len() (see the post-return emission above).
    gs.global(
        GlobalType {
            val_type: I32,
            mutable: false,
            shared: false,
        },
        &ConstExpr::i32_const(arena_floor as i32),
    );
    // 5.1 persistent bump pointer (index 3 + value_defs.len()). Grows up from
    // heap_base within the reserve; never reset. Left at heap_base and unused
    // by non-resource components.
    gs.global(
        GlobalType {
            val_type: I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(heap_base as i32),
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
        let idxs: Vec<u32> = (0..em.closure_bodies.len() as u32)
            .map(|k| closure_base + k)
            .collect();
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
    ds.active(
        0,
        &ConstExpr::i32_const(DATA_BASE as i32),
        em.data.iter().copied(),
    );
    module.section(&ds);

    Ok(module.finish())
}

// ------------------------------------------------- functor `set` resource bodies
//
// Emit the core wasm functions that implement a `set` resource for ONE
// instantiation at element type `elem`. This is the
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

/// Routing data for one user-declared resource (4.5). `methods`/`statics` are
/// the member op-names; a method call converts the receiver handle to its rep
/// with `rep_import` before dispatch, a static call is a plain internal call.
#[derive(Clone)]
struct UserRes {
    methods: std::collections::HashSet<String>,
    statics: std::collections::HashSet<String>,
    /// core fn idx of the `[dtor]<name>` body (runs `Drop` or no-op).
    dtor: u32,
}

/// One `DefResource` form, classified for the backend: the constructor (`New`),
/// instance methods, statics, and destructor (`Drop`), each as `(params, body)`
/// arena node ids. Mirrors the interpreter's `defresource-MACRO` handling.
struct ResForm {
    name: String,
    ctor: Option<(NodeId, NodeId)>,
    methods: Vec<(String, NodeId, NodeId)>,
    statics: Vec<(String, NodeId, NodeId)>,
    drop_member: Option<(NodeId, NodeId)>,
}

/// Walk the file roots for `DefResource` forms, classifying each member the same
/// way the interpreter and WIT collector do (`New`/`Drop` keys, `Static Fn`
/// marker, otherwise an instance method).
fn user_resource_forms(arena: &Arena, roots: &[NodeId]) -> Result<Vec<ResForm>, String> {
    let mut out = Vec::new();
    for &root in roots {
        let Node::Tup(items) = arena.node(root) else {
            continue;
        };
        if items.len() < 3
            || !matches!(arena.node(items[0]), Node::Sym(s) if s == "defresource-MACRO")
        {
            continue;
        }
        let Node::Sym(name) = arena.node(items[1]) else {
            continue;
        };
        let Node::Rec(members) = arena.node(items[2]) else {
            continue;
        };
        let mut rf = ResForm {
            name: name.clone(),
            ctor: None,
            methods: Vec::new(),
            statics: Vec::new(),
            drop_member: None,
        };
        for (key, val) in members {
            let (is_static, fn_id) = match arena.node(*val) {
                Node::Tup(t)
                    if t.len() == 2
                        && matches!(arena.node(t[0]), Node::Sym(s) if s == "static-MACRO") =>
                {
                    (true, t[1])
                }
                _ => (false, *val),
            };
            let Node::Tup(fnitems) = arena.node(fn_id) else {
                return Err(format!("resource `{name}` member `{key}` must be an `Fn`"));
            };
            if fnitems.len() != 3
                || !matches!(arena.node(fnitems[0]), Node::Sym(s) if s == "fn-MACRO")
            {
                return Err(format!("resource `{name}` member `{key}` must be an `Fn`"));
            }
            let (params_id, body) = (fnitems[1], fnitems[2]);
            match key.as_str() {
                "New" => rf.ctor = Some((params_id, body)),
                "Drop" => rf.drop_member = Some((params_id, body)),
                other if is_static => rf.statics.push((other.to_string(), params_id, body)),
                other => rf.methods.push((other.to_string(), params_id, body)),
            }
        }
        out.push(rf);
    }
    Ok(out)
}

/// The five core functions implementing one `set` instantiation, by core
/// function index, plus the resource-intrinsic import indices their bodies
/// reference. Step 03 wires these into the export/import sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
        // cell = persist_alloc(4): the set rep must survive the per-call arena
        // reset, so it lives in the persistent region with a stable identity.
        fx.op(I::I32Const(4));
        fx.op(I::Call(em.h.persist_alloc));
        fx.op(I::LocalSet(cell));
        // cell[0] = empty list box, deep-copied into the persistent region
        fx.op(I::LocalGet(cell));
        emit_empty_list_box(em, &mut fx);
        fx.op(I::Call(em.h.persist));
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
        // *self = persist(old list + needle): the grown list is built in the
        // arena, then deep-copied into the persistent region so it outlives the
        // reset (already-persistent elements are kept in place, only the new
        // needle is copied).
        fx.op(I::LocalGet(0));
        emit_list_append(em, &mut fx, list, needle);
        fx.op(I::Call(em.h.persist));
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
        let Node::Tup(items) = arena.node(root) else {
            continue;
        };
        if items.len() != 4 {
            continue;
        }
        let Node::Sym(h) = arena.node(items[0]) else {
            continue;
        };
        if h != "defmacro-MACRO" {
            continue;
        }
        let Node::Sym(name) = arena.node(items[1]) else {
            continue;
        };
        let params = param_names(arena, items[2])?;
        macros.push(MacroDef {
            name: name.clone(),
            params,
            body: items[3],
        });
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
        resources: Vec::new(),
    };
    let deps: HashMap<String, Dep> = HashMap::new();

    let mut em = Emitter {
        arena,
        info: &info,
        deps: &deps,
        type_env: TypeEnv::default(),
        local_cases: HashMap::new(),
        data: Vec::new(),
        str_cache: HashMap::new(),
        types: Vec::new(),
        imports: Vec::new(),
        import_fn: HashMap::new(),
        h: Helpers::default(),
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
    em.h.unbox_char = take();
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
    em.h.arith_int = take();
    em.h.cmp_f64 = take();
    em.h.persist_alloc = take();
    em.h.persist = take();
    emit_helpers(&mut em)?;

    // macro body functions (each compiles like a Fn over its param forms)
    for m in &macros {
        let idx = take();
        em.funcs.insert(
            m.name.clone(),
            (idx, m.params.clone(), FnSig::boxed(m.params.len())),
        );
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
        (
            "wavelet:meta/macros@0.1.0#manifest".to_string(),
            manifest_idx,
        ),
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
        GlobalType {
            val_type: I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(heap_base as i32),
    );
    // gensym counter (index 1, since there are no value defs)
    gs.global(
        GlobalType {
            val_type: ValType::I64,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i64_const(0),
    );
    // arena floor (index 2) + persistent bump pointer (index 3): a macro library
    // has no resources, so both are the heap base and the persist helpers (which
    // reference them) are dead code.
    gs.global(
        GlobalType {
            val_type: I32,
            mutable: false,
            shared: false,
        },
        &ConstExpr::i32_const(heap_base as i32),
    );
    gs.global(
        GlobalType {
            val_type: I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(heap_base as i32),
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
        let idxs: Vec<u32> = (0..em.closure_bodies.len() as u32)
            .map(|k| closure_base + k)
            .collect();
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
    ds.active(
        0,
        &ConstExpr::i32_const(DATA_BASE as i32),
        em.data.iter().copied(),
    );
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
        scope.insert(p.clone(), Binding::boxed(i as u32));
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
    // char-val: the wire payload is a `char`, lifted as a TAG_CHAR box already —
    // use it directly.
    {
        let s = em.intern_str("char-val") as i32;
        fx.op(I::LocalGet(case));
        fx.op(I::I32Const(s));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(payload));
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
fn emit_substr(
    em: &mut Emitter,
    fx: &mut FnCtx,
    src: u32,
    start: u32,
    sublen: u32,
    out: u32,
    j: u32,
) {
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
    // char (TAG_CHAR): wire `char-val`, payload = the char box itself, which
    // the boundary's `lower(char)` (= unbox_char) reads.
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
    let list_ty = WitTy::List(Box::new(WitTy::Tuple(vec![WitTy::Str, WitTy::IntU(4)])));
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
            let msg =
                em.intern_str(&format!("macro `{}` expects {arity} arguments", m.name)) as i32;
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

/// Emit an in-place `to_str` join loop for a sequence-shaped box (list, tuple,
/// or flags): `open` + elements joined by `comma` + `close`. Elements sit at
/// `box+8 + stride*i` for `i` in `0..load(box+4)`; when `recurse` each element
/// box is run through `to_str`, otherwise it is a `str` box appended verbatim
/// (the flags-name case). Emits its own `return`.
#[allow(clippy::too_many_arguments)]
fn to_str_seq(
    fx: &mut FnCtx,
    box_l: u32,
    n_l: u32,
    i_l: u32,
    acc_l: u32,
    base_l: u32,
    elem_l: u32,
    open_addr: u32,
    close_addr: u32,
    comma_addr: u32,
    stride: i32,
    strcat2: u32,
    to_str: u32,
    recurse: bool,
) {
    fx.op(I::I32Const(open_addr as i32));
    fx.op(I::LocalSet(acc_l));
    fx.op(I::LocalGet(box_l));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(n_l));
    fx.op(I::LocalGet(box_l));
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(base_l));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(i_l));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(i_l));
    fx.op(I::LocalGet(n_l));
    fx.op(I::I32GeS);
    fx.op(I::BrIf(1));
    fx.op(I::LocalGet(i_l));
    fx.op(I::I32Const(0));
    fx.op(I::I32GtS);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(acc_l));
    fx.op(I::I32Const(comma_addr as i32));
    fx.op(I::Call(strcat2));
    fx.op(I::LocalSet(acc_l));
    fx.op(I::End);
    fx.op(I::LocalGet(base_l));
    fx.op(I::LocalGet(i_l));
    fx.op(I::I32Const(stride));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(0, 2)));
    fx.op(I::LocalSet(elem_l));
    fx.op(I::LocalGet(acc_l));
    fx.op(I::LocalGet(elem_l));
    if recurse {
        fx.op(I::Call(to_str));
    }
    fx.op(I::Call(strcat2));
    fx.op(I::LocalSet(acc_l));
    fx.op(I::LocalGet(i_l));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(i_l));
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::LocalGet(acc_l));
    fx.op(I::I32Const(close_addr as i32));
    fx.op(I::Call(strcat2));
    fx.op(I::Return);
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
        fx.op(I::MemoryCopy {
            src_mem: 0,
            dst_mem: 0,
        });
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
        fx.op(I::MemoryCopy {
            src_mem: 0,
            dst_mem: 0,
        });
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

    // unbox_char(box) -> i64 codepoint (traps unless tag char)
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_CHAR));
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
    //
    // Structural equality mirroring the interpreter's `impl PartialEq for Value`
    // (src/value.rs). Primitives (bool/int/char/dec/str) compare by content;
    // compound boxes (rec/list/tup/var/flg) recurse into their element boxes via
    // this very fn (`em.h.eq_raw`, already reserved). Only closures (TAG_FN) keep
    // pointer identity, matching `Rc::ptr_eq` for `Closure`/`Macro`.
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
        // char: i64 scalar @8 (TAG_INT layout)
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_CHAR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::LocalGet(1));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::I64Eq);
        fx.op(I::Return);
        fx.op(I::End);
        // record: n @4, then (key strbox @8+8i, value box @12+8i) pairs.
        // Order-sensitive (Value::Rec is a Vec compare): both n must match, then
        // each key AND value compared positionally by recursing eq_raw.
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_REC));
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
        // key: load a[8+8i], b[8+8i] and recurse
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalGet(1));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        // value: load a[12+8i], b[12+8i] and recurse
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(12, 2)));
        fx.op(I::LocalGet(1));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(12, 2)));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::I32Eqz);
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
        // list / tuple / flags: len @4, element boxes @8+4i. Order-sensitive
        // (Value::Lst/Tup/Flg are Vec compares). All three share this layout: a
        // flags box stores its name str boxes @8+4i, so structural recursion over
        // them matches the interpreter's `Flg(Vec<String>)` equality too.
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Eq);
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_TUP));
        fx.op(I::I32Eq);
        fx.op(I::I32Or);
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_FLG));
        fx.op(I::I32Eq);
        fx.op(I::I32Or);
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
        // element: load a[8+4i], b[8+4i] and recurse
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalGet(1));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::I32Eqz);
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
        // variant: case-name strbox @4, payload box @8 (0 if none). Equal iff
        // case names match (recurse) and payloads match: both absent (0) is equal,
        // exactly one absent is unequal, else recurse on the two payload boxes.
        // Mirrors `Variant(a,p) == Variant(b,q) => a == b && p == q`.
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        // case names
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        // payload presence: la = a.payload, i = b.payload
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalSet(la));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalSet(i));
        // both absent -> equal
        fx.op(I::LocalGet(la));
        fx.op(I::I32Eqz);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Eqz);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(1));
        fx.op(I::Return);
        fx.op(I::End);
        // exactly one absent -> unequal (la == 0 XOR i == 0)
        fx.op(I::LocalGet(la));
        fx.op(I::I32Eqz);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Eqz);
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        // both present -> recurse on payloads
        fx.op(I::LocalGet(la));
        fx.op(I::LocalGet(i));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::Return);
        fx.op(I::End);
        // closures (TAG_FN) and anything else unhandled: pointer identity,
        // matching the interpreter's `Rc::ptr_eq` for Closure/Macro.
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
        fx.op(I::MemoryCopy {
            src_mem: 0,
            dst_mem: 0,
        });
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(la));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(1));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(lb));
        fx.op(I::MemoryCopy {
            src_mem: 0,
            dst_mem: 0,
        });
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
        // interned punctuation for the compound-value printers
        let str_lb = em.intern_str("[");
        let str_rb = em.intern_str("]");
        let str_lp = em.intern_str("(");
        let str_rp = em.intern_str(")");
        let str_lc = em.intern_str("{");
        let str_rc = em.intern_str("}");
        let str_comma = em.intern_str(", ");
        let str_colon = em.intern_str(": ");
        let str_cell = em.intern_str("cell(");
        let mut fx = FnCtx::new(1);
        let tag = fx.local(I32);
        let n = fx.local(I64);
        let neg = fx.local(I32);
        let buf = fx.local(I32);
        let i = fx.local(I32);
        // extra locals for the string-quoting branch
        let s_src = fx.local(I32);
        let s_len = fx.local(I32);
        let s_out = fx.local(I32);
        let s_oi = fx.local(I32);
        let s_ci = fx.local(I32);
        let s_byte = fx.local(I32);
        // extra locals for the compound-value branches
        let c_n = fx.local(I32);
        let c_i = fx.local(I32);
        let c_acc = fx.local(I32);
        let c_base = fx.local(I32);
        let c_elem = fx.local(I32);
        let c_key = fx.local(I32);
        let c_val = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(tag));
        // out[s_oi] = <const byte>; s_oi += 1
        let put_c = |fx: &mut FnCtx, out: u32, oi: u32, b: i32| {
            fx.op(I::LocalGet(out));
            fx.op(I::LocalGet(oi));
            fx.op(I::I32Add);
            fx.op(I::I32Const(b));
            fx.op(I::I32Store8(ma(0, 0)));
            fx.op(I::LocalGet(oi));
            fx.op(I::I32Const(1));
            fx.op(I::I32Add);
            fx.op(I::LocalSet(oi));
        };
        // string: quote + escape to match print_value's `{s:?}`. Escapes
        // `"` `\` `\n` `\t` `\r`; other bytes (incl. UTF-8 continuation) pass
        // through. Rust escapes other control/non-printable codepoints too, so
        // strings containing them still diverge from the oracle (kept SKIP);
        // the common printable cases now agree.
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        // s_len = box@4 ; s_src = box+8
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(s_len));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(s_src));
        // s_out = alloc(s_len*2 + 2)  (worst case: every byte -> 2 bytes, + 2 quotes)
        fx.op(I::LocalGet(s_len));
        fx.op(I::I32Const(2));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(2));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(s_out));
        // out[0] = '"' ; s_oi = 1 ; s_ci = 0
        fx.op(I::LocalGet(s_out));
        fx.op(I::I32Const(b'"' as i32));
        fx.op(I::I32Store8(ma(0, 0)));
        fx.op(I::I32Const(1));
        fx.op(I::LocalSet(s_oi));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(s_ci));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        // if s_ci >= s_len break
        fx.op(I::LocalGet(s_ci));
        fx.op(I::LocalGet(s_len));
        fx.op(I::I32GeS);
        fx.op(I::BrIf(1));
        // s_byte = load8(s_src + s_ci)
        fx.op(I::LocalGet(s_src));
        fx.op(I::LocalGet(s_ci));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(0, 0)));
        fx.op(I::LocalSet(s_byte));
        // escape ladder
        fx.op(I::LocalGet(s_byte));
        fx.op(I::I32Const(b'"' as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        put_c(&mut fx, s_out, s_oi, b'\\' as i32);
        put_c(&mut fx, s_out, s_oi, b'"' as i32);
        fx.op(I::Else);
        fx.op(I::LocalGet(s_byte));
        fx.op(I::I32Const(b'\\' as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        put_c(&mut fx, s_out, s_oi, b'\\' as i32);
        put_c(&mut fx, s_out, s_oi, b'\\' as i32);
        fx.op(I::Else);
        fx.op(I::LocalGet(s_byte));
        fx.op(I::I32Const(b'\n' as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        put_c(&mut fx, s_out, s_oi, b'\\' as i32);
        put_c(&mut fx, s_out, s_oi, b'n' as i32);
        fx.op(I::Else);
        fx.op(I::LocalGet(s_byte));
        fx.op(I::I32Const(b'\t' as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        put_c(&mut fx, s_out, s_oi, b'\\' as i32);
        put_c(&mut fx, s_out, s_oi, b't' as i32);
        fx.op(I::Else);
        fx.op(I::LocalGet(s_byte));
        fx.op(I::I32Const(b'\r' as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        put_c(&mut fx, s_out, s_oi, b'\\' as i32);
        put_c(&mut fx, s_out, s_oi, b'r' as i32);
        fx.op(I::Else);
        // default: copy the byte verbatim
        fx.op(I::LocalGet(s_out));
        fx.op(I::LocalGet(s_oi));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(s_byte));
        fx.op(I::I32Store8(ma(0, 0)));
        fx.op(I::LocalGet(s_oi));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(s_oi));
        fx.op(I::End); // \r
        fx.op(I::End); // \t
        fx.op(I::End); // \n
        fx.op(I::End); // backslash
        fx.op(I::End); // quote
        // s_ci += 1 ; continue
        fx.op(I::LocalGet(s_ci));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(s_ci));
        fx.op(I::Br(0));
        fx.op(I::End); // loop
        fx.op(I::End); // block
        // closing quote, then box
        put_c(&mut fx, s_out, s_oi, b'"' as i32);
        fx.op(I::LocalGet(s_out));
        fx.op(I::LocalGet(s_oi));
        fx.op(I::Call(em.h.box_str));
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
        // ---- compound values: recurse via to_str, matching print_value ----
        let seq_strcat2 = em.h.strcat2;
        let seq_to_str = em.h.to_str;
        // list: [e0, e1, ...]
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        to_str_seq(
            &mut fx, 0, c_n, c_i, c_acc, c_base, c_elem, str_lb, str_rb, str_comma,
            4, seq_strcat2, seq_to_str, true,
        );
        fx.op(I::End);
        // tuple: (e0, e1, ...)
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_TUP));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        to_str_seq(
            &mut fx, 0, c_n, c_i, c_acc, c_base, c_elem, str_lp, str_rp, str_comma,
            4, seq_strcat2, seq_to_str, true,
        );
        fx.op(I::End);
        // flags: {a, b, ...} (names are str boxes, appended verbatim)
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_FLG));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        to_str_seq(
            &mut fx, 0, c_n, c_i, c_acc, c_base, c_elem, str_lc, str_rc, str_comma,
            4, seq_strcat2, seq_to_str, false,
        );
        fx.op(I::End);
        // record: {k0: v0, k1: v1, ...}
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_REC));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(str_lc as i32));
        fx.op(I::LocalSet(c_acc));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(c_n));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(c_base));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(c_i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(c_i));
        fx.op(I::LocalGet(c_n));
        fx.op(I::I32GeS);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(c_i));
        fx.op(I::I32Const(0));
        fx.op(I::I32GtS);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(c_acc));
        fx.op(I::I32Const(str_comma as i32));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::LocalSet(c_acc));
        fx.op(I::End);
        // key = load(base + 8*i)  (a str box, appended verbatim)
        fx.op(I::LocalGet(c_base));
        fx.op(I::LocalGet(c_i));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(c_key));
        // val = load(base + 8*i + 4)
        fx.op(I::LocalGet(c_base));
        fx.op(I::LocalGet(c_i));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Const(4));
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(c_val));
        fx.op(I::LocalGet(c_acc));
        fx.op(I::LocalGet(c_key));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::LocalSet(c_acc));
        fx.op(I::LocalGet(c_acc));
        fx.op(I::I32Const(str_colon as i32));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::LocalSet(c_acc));
        fx.op(I::LocalGet(c_acc));
        fx.op(I::LocalGet(c_val));
        fx.op(I::Call(seq_to_str));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::LocalSet(c_acc));
        fx.op(I::LocalGet(c_i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(c_i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(c_acc));
        fx.op(I::I32Const(str_rc as i32));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::Return);
        fx.op(I::End);
        // variant: name  or  name(payload)
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(c_key)); // case-name str box
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalSet(c_val)); // payload box (0 if none)
        fx.op(I::LocalGet(c_val));
        fx.op(I::I32Const(0));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(c_key));
        fx.op(I::I32Const(str_lp as i32));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::LocalSet(c_acc));
        fx.op(I::LocalGet(c_acc));
        fx.op(I::LocalGet(c_val));
        fx.op(I::Call(seq_to_str));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::LocalSet(c_acc));
        fx.op(I::LocalGet(c_acc));
        fx.op(I::I32Const(str_rp as i32));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::Return);
        fx.op(I::End);
        // no payload: the bare case name
        fx.op(I::LocalGet(c_key));
        fx.op(I::Return);
        fx.op(I::End);
        // cell: cell(inner)
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_CELL));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(c_val));
        fx.op(I::I32Const(str_cell as i32));
        fx.op(I::LocalGet(c_val));
        fx.op(I::Call(seq_to_str));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::LocalSet(c_acc));
        fx.op(I::LocalGet(c_acc));
        fx.op(I::I32Const(str_rp as i32));
        fx.op(I::Call(seq_strcat2));
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
    // [locals: xf=3, yf=4 (f64)]
    {
        let mut fx = FnCtx::new(3);
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
        // ---- int path: the shared checked-arithmetic core (arith_int), so
        // the boxed and typed (goal 5) paths cannot drift apart
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::LocalGet(1));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::LocalGet(2));
        fx.op(I::Call(em.h.arith_int));
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
    // lexicographic), chars (by codepoint) and numbers (widened to f64); traps
    // on NaN/non-comparable, matching the interpreter's `compare`.
    // [locals: la=2, lb=3, n=4, i=5, ca=6, cb=7 (i32)]
    {
        let mut fx = FnCtx::new(2);
        let la = fx.local(I32);
        let lb = fx.local(I32);
        let n = fx.local(I32);
        let i = fx.local(I32);
        let ca = fx.local(I32);
        let cb = fx.local(I32);
        // both char? order by codepoint (interpreter: `Char(x).cmp(Char(y))`)
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_CHAR));
        fx.op(I::I32Eq);
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_CHAR));
        fx.op(I::I32Eq);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::LocalGet(1));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::I64LtU);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(-1));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::LocalGet(1));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::I64GtU);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(1));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
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
        // ---- numeric compare: the shared cmp_f64 core (widened to f64;
        // traps on NaN), so the boxed and typed (goal 5) paths cannot drift
        fx.op(I::LocalGet(0));
        fx.op(I::Call(em.h.as_f64));
        fx.op(I::LocalGet(1));
        fx.op(I::Call(em.h.as_f64));
        fx.op(I::Call(em.h.cmp_f64));
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

    // arith_int(a: i64, b: i64, op: i32) -> i64 — the checked integer
    // arithmetic core (op: 0=add 1=sub 2=mul 3=div 4=rem): trap on overflow /
    // div-0 / INT_MIN÷-1, exactly the interpreter's checked_* semantics. The
    // boxed arith_raw and the goal-5 typed scalar path both call this, so the
    // two representations share one copy of the semantics.
    // [locals: ia=3, ib=4, r=5 (i64)]
    {
        let mut fx = FnCtx::new(3);
        let ia = fx.local(I64);
        let ib = fx.local(I64);
        let r = fx.local(I64);
        fx.op(I::LocalGet(0));
        fx.op(I::LocalSet(ia));
        fx.op(I::LocalGet(1));
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
        let t = em.ty_idx(vec![I64, I64, I32], vec![I64]);
        em.bodies.push((t, fx.finish()));
    }

    // cmp_f64(x: f64, y: f64) -> i32 in {-1, 0, 1}; traps on NaN (the
    // interpreter's "values are not comparable"). The numeric tail of
    // cmp_raw, shared with the goal-5 typed scalar path.
    {
        let mut fx = FnCtx::new(2);
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(1));
        fx.op(I::F64Lt);
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::I32Const(-1));
        fx.op(I::Else);
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(1));
        fx.op(I::F64Gt);
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::I32Const(1));
        fx.op(I::Else);
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(1));
        fx.op(I::F64Eq);
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::I32Const(0));
        fx.op(I::Else);
        // unordered (NaN) — not comparable
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::End);
        let t = em.ty_idx(vec![F64, F64], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // ---- 5.1 persistent region: allocator + deep-copy write barrier
    //
    // Resource/functor components hold resource state that must survive the
    // per-call arena reset. That state lives in a PERSISTENT region below the
    // arena floor: global `persist_g` bumps up from `heap_base`, capped at the
    // arena floor (global `floor_g`); the arena grows above the floor and is
    // reset each post-return. A non-resource component has floor == heap_base
    // (zero reserve), so these helpers are emitted but never called.
    let floor_g = 2 + em.info.value_defs.len() as u32;
    let persist_g = 3 + em.info.value_defs.len() as u32;

    // persist_alloc(n) -> ptr  [param0=n, r=1, end=2]
    {
        let mut fx = FnCtx::new(1);
        let r = fx.local(I32);
        let end = fx.local(I32);
        fx.op(I::GlobalGet(persist_g));
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
        // trap if the fixed persistent reserve is exhausted
        fx.op(I::LocalGet(end));
        fx.op(I::GlobalGet(floor_g));
        fx.op(I::I32GtU);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(end));
        fx.op(I::GlobalSet(persist_g));
        fx.op(I::LocalGet(r));
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // persist(box) -> box  [param0=box, tg=1, sz=2, pbase=3, pcount=4, new=5, i=6, off=7]
    //
    // Interned/already-persistent nodes (box < arena_floor) are returned as-is.
    // An arena box is copied whole into the persistent region, then each of its
    // child pointer words is re-persisted recursively (a null child persists to
    // null, since 0 < arena_floor). `persist` is self-recursive via em.h.persist.
    {
        let mut fx = FnCtx::new(1);
        let tg = fx.local(I32);
        let sz = fx.local(I32);
        let pbase = fx.local(I32);
        let pcount = fx.local(I32);
        let new = fx.local(I32);
        let i = fx.local(I32);
        let off = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::GlobalGet(floor_g));
        fx.op(I::I32LtU);
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::LocalGet(0)); // interned / already persistent
        fx.op(I::Else);
        // defaults: flat 16-byte box, no children (INT/DEC/CHAR)
        fx.op(I::I32Const(16));
        fx.op(I::LocalSet(sz));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(pbase));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(pcount));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(tg));
        // TAG_FN: closures in resource state unsupported
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_FN));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        // TAG_STR: sz = 8 + len; no children (bytes inline)
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(sz));
        fx.op(I::End);
        // TAG_LIST / TAG_TUP / TAG_FLG: pbase=8, pcount=n, sz=8+4n
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Eq);
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_TUP));
        fx.op(I::I32Eq);
        fx.op(I::I32Or);
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_FLG));
        fx.op(I::I32Eq);
        fx.op(I::I32Or);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(pcount));
        fx.op(I::I32Const(8));
        fx.op(I::LocalSet(pbase));
        fx.op(I::LocalGet(pcount));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(sz));
        fx.op(I::End);
        // TAG_REC: pbase=8, pcount=2n, sz=8+8n
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_REC));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Const(2));
        fx.op(I::I32Mul);
        fx.op(I::LocalSet(pcount));
        fx.op(I::I32Const(8));
        fx.op(I::LocalSet(pbase));
        fx.op(I::LocalGet(pcount));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(sz));
        fx.op(I::End);
        // TAG_VAR: pbase=4, pcount=2 (case ptr + payload; null payload persists to null), sz=12
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(4));
        fx.op(I::LocalSet(pbase));
        fx.op(I::I32Const(2));
        fx.op(I::LocalSet(pcount));
        fx.op(I::I32Const(12));
        fx.op(I::LocalSet(sz));
        fx.op(I::End);
        // TAG_CELL: pbase=4, pcount=1, sz=8
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_CELL));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(4));
        fx.op(I::LocalSet(pbase));
        fx.op(I::I32Const(1));
        fx.op(I::LocalSet(pcount));
        fx.op(I::I32Const(8));
        fx.op(I::LocalSet(sz));
        fx.op(I::End);
        // new = persist_alloc(sz); memory.copy(new, box, sz)
        fx.op(I::LocalGet(sz));
        fx.op(I::Call(em.h.persist_alloc));
        fx.op(I::LocalSet(new));
        fx.op(I::LocalGet(new));
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(sz));
        fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
        // for i in 0..pcount: new[pbase+4i] = persist(box[pbase+4i])
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(pcount));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(pbase));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalSet(off));
        // dst = new + off
        fx.op(I::LocalGet(new));
        fx.op(I::LocalGet(off));
        fx.op(I::I32Add);
        // val = persist(box[off])
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(off));
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::Call(em.h.persist));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End); // loop
        fx.op(I::End); // block
        fx.op(I::LocalGet(new));
        fx.op(I::End); // if box<floor
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

/// The `use` clauses a local interface needs for the dep-defined type names
/// its rendered signatures/type declarations reference (4.3): each entry is a
/// versioned interface path (`acme:pts/types@0.3.1`) with the names to bring
/// in. Tokenizes the WIT texts and keeps identifiers that are not primitives,
/// WIT keywords, or locally-declared types, and that some imported dependency
/// declares (records, variants/enums/flags, aliases, resources alike).
fn dep_type_uses(
    texts: &[String],
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> Vec<(String, Vec<String>)> {
    /// primitives, type constructors, and declaration keywords that can appear
    /// in rendered WIT type text — never dep type names.
    const RESERVED: &[&str] = &[
        "bool",
        "u8",
        "u16",
        "u32",
        "u64",
        "s8",
        "s16",
        "s32",
        "s64",
        "f32",
        "f64",
        "char",
        "string",
        "list",
        "option",
        "result",
        "tuple",
        "own",
        "borrow",
        "record",
        "variant",
        "enum",
        "flags",
        "type",
        "func",
        "resource",
        "static",
        "constructor",
        "use",
    ];
    let local: std::collections::HashSet<&str> =
        info.types.iter().map(|(n, _)| n.as_str()).collect();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for text in texts {
        for tok in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
            if tok.is_empty()
                || tok == "_"
                || RESERVED.contains(&tok)
                || local.contains(tok)
                || !seen.insert(tok.to_string())
            {
                continue;
            }
            // The first imported dependency declaring this name wins.
            for imp in &info.imports {
                let Some(dep) = deps.get(&imp.package) else {
                    continue;
                };
                let Some((_, di)) = dep.type_ifaces.iter().find(|(n, _)| n == tok) else {
                    continue;
                };
                let path = versioned_iface(&dep.package, di);
                match out.iter_mut().find(|(p, _)| p == &path) {
                    Some((_, names)) => names.push(tok.to_string()),
                    None => out.push((path, vec![tok.to_string()])),
                }
                break;
            }
        }
    }
    out
}

fn synthesize_world_wit(
    arena: &Arena,
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> Result<String, String> {
    let mut out = format!("package {};\n\n", info.package);

    let mut ifaces = crate::wit::iface_order(&info.exports, !info.types.is_empty());
    // A resource-only export (4.5) still needs its placement interface present.
    // External-interface resource exports are defined by the dependency's WIT and
    // never re-declared here, so only fold in *internal* placement interfaces.
    for r in &info.resources {
        if let Some(iface) = &r.iface
            && !is_external_iface(iface)
            && !ifaces.contains(iface)
        {
            ifaces.push(iface.clone());
        }
    }

    // Hoisted local types (4.7). An export that returns (or takes) a functor
    // handle makes its interface `use` the functor interface; when that
    // functor's element is a local record, the functor interface would `use`
    // the record back from `api` — a WIT interface cycle, which WIT cannot
    // express. Break it by hoisting the element record (and any local types
    // its declaration references, transitively) into a shared `types`
    // interface that both `api` and the functor interface `use`.
    let hoisted = crate::wit::hoisted_types(arena, info)?;
    if !hoisted.is_empty() {
        out.push_str("interface types {\n");
        let mut texts: Vec<String> = Vec::new();
        for name in &hoisted {
            let (_, ty) = info
                .types
                .iter()
                .find(|(n, _)| n == name)
                .expect("hoisted names come from info.types");
            let d = type_decl(arena, name, *ty)?;
            texts.push(d.clone());
            out.push_str(&format!("  {d}\n"));
        }
        // …their declarations may themselves reference dep types (4.3).
        // (Re-rendered inside the loop; collect first to emit uses on top.)
        let uses = dep_type_uses(&texts, info, deps);
        if !uses.is_empty() {
            // `use` lines must be re-emitted before the decls: rebuild.
            let mut body = String::new();
            for (use_path, names) in &uses {
                body.push_str(&format!("  use {use_path}.{{{}}};\n", names.join(", ")));
            }
            for t in &texts {
                body.push_str(&format!("  {t}\n"));
            }
            let start = out.rfind("interface types {\n").expect("just pushed");
            out.truncate(start);
            out.push_str("interface types {\n");
            out.push_str(&body);
        }
        out.push_str("}\n\n");
    }

    // External interfaces (e.g. wasi:http/incoming-handler, wasi:cli/run) are
    // defined by the dependency's WIT; we only export them by name, never
    // re-declare them here.
    for iface in ifaces.iter().filter(|i| !is_external_iface(i)) {
        // An export whose signature references a functor handle gets it as the
        // dotted `<funct-iface>.set` text (from `wit::functor_op_table`). WIT does
        // not accept an inline dotted type reference; the type must be `use`-d
        // from its interface and then named bare. Detect which functor interfaces
        // an interface's signatures reference and emit a `use <funct>.{set};` for
        // each, rewriting the dotted occurrences in the signatures to bare `set`.
        // (The functor interface is declared later in the same package; WIT `use`
        // resolves forward references within a package.)
        let sigs: Vec<&FuncSig> = info.exports.iter().filter(|s| &s.iface == iface).collect();
        let used: Vec<&str> = info
            .functors
            .iter()
            .filter(|f| {
                let dotted = format!("{}.set", f.iface);
                sigs.iter().any(|s| {
                    s.result.as_deref() == Some(dotted.as_str())
                        || s.params.iter().any(|(_, t)| t == &dotted)
                })
            })
            .map(|f| f.iface.as_str())
            .collect();
        out.push_str(&format!("interface {iface} {{\n"));
        // Each functor interface names its resource `set`, so an interface that
        // references *two* functor handles (two instantiations, both returned or
        // taken by exports) would `use` two types both called `set` — a WIT
        // "defined more than once" collision. Alias each `use` to a per-functor
        // name (`set as <iface>-handle`) and rewrite the dotted `<iface>.set`
        // occurrences in the signatures to that alias. A single functor still
        // reads naturally; multiple instantiations no longer collide. (The alias
        // only renames the WIT type binding — the handle still lowers to one i32.)
        for funct in &used {
            out.push_str(&format!("  use {funct}.{{set as {funct}-handle}};\n"));
        }
        // Cross-package type references (4.3): a signature (or a local type
        // declaration) may name a type a dependency's interface defines. WIT
        // requires such names be brought into scope with a `use`, so collect
        // every dep-defined name the interface's text references and emit
        // `use <pkg>/<iface>@<ver>.{names};` per defining interface.
        let mut texts: Vec<String> = Vec::new();
        for sig in &sigs {
            for (_, t) in &sig.params {
                texts.push(t.clone());
            }
            if let Some(r) = &sig.result {
                texts.push(r.clone());
            }
        }
        let mut api_decls: Vec<String> = Vec::new();
        if iface == "api" {
            // Hoisted element types (4.7) are declared in `types` and brought
            // back into scope here; the rest declare in place as before.
            if !hoisted.is_empty() {
                out.push_str(&format!("  use types.{{{}}};\n", hoisted.join(", ")));
            }
            for (name, ty) in info.types.iter().filter(|(n, _)| !hoisted.contains(n)) {
                let d = type_decl(arena, name, *ty)?;
                texts.push(d.clone());
                api_decls.push(d);
            }
        }
        for (use_path, names) in dep_type_uses(&texts, info, deps) {
            out.push_str(&format!("  use {use_path}.{{{}}};\n", names.join(", ")));
        }
        for d in &api_decls {
            out.push_str(&format!("  {d}\n"));
        }
        // Exported user-declared resource blocks (4.5) land in their placement
        // interface. External-iface resources are defined by the dependency WIT
        // (filtered out above), so only internal placements reach here.
        for r in info
            .resources
            .iter()
            .filter(|r| r.iface.as_deref() == Some(iface.as_str()))
        {
            out.push_str(&r.to_wit());
        }
        for sig in &sigs {
            let mut line = sig.to_wit();
            for funct in &used {
                line = line.replace(&format!("{funct}.set"), &format!("{funct}-handle"));
            }
            out.push_str(&format!("  {line}\n"));
        }
        out.push_str("}\n\n");
    }

    // Functor instantiations stamp out a specialized, monomorphic interface each
    // (Steps 10–11), rendered from the SAME `SET_OPS` source as `wavelet wit`
    // (`wit::functor_interface`) so the WIT the encoder validates against and the
    // resource the wasm backend implements cannot drift.
    for f in &info.functors {
        // The element's declaring interface: `types` once hoisted (4.7), `api`
        // for an un-hoisted local type, none for a primitive element.
        let elem_iface = if hoisted.contains(&f.elem) {
            Some("types")
        } else if info.types.iter().any(|(n, _)| n == &f.elem) {
            Some("api")
        } else {
            None
        };
        out.push_str(crate::wit::functor_interface(arena, f, elem_iface)?.trim_start());
        out.push('\n');
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
        let dep = deps.get(&imp.package).ok_or(format!(
            "dependency `{}` is not in the build set",
            imp.package
        ))?;
        out.push_str(&format!(
            "  import {};\n",
            versioned_iface(&dep.package, &iface)
        ));
    }
    // The hoisted `types` interface (4.7) is exported so the interfaces that
    // `use` it resolve in the encoded component.
    if !hoisted.is_empty() {
        out.push_str("  export types;\n");
    }
    for iface in &ifaces {
        if is_external_iface(iface) {
            out.push_str(&format!(
                "  export {};\n",
                external_versioned_in(iface, deps)
            ));
        } else {
            out.push_str(&format!("  export {iface};\n"));
        }
    }
    // Each functor instantiation exports its specialized interface (so the
    // encoder synthesizes the `[resource-new/rep/drop]set` intrinsics the core
    // module imports — they only appear when the world *exports* the resource).
    for f in &info.functors {
        out.push_str(&format!("  export {};\n", f.iface));
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
            resources: Vec::new(),
        };
        let deps: HashMap<String, Dep> = HashMap::new();

        let mut em = Emitter {
            arena: &arena,
            info: &info,
            deps: &deps,
            type_env: TypeEnv::default(),
            local_cases: HashMap::new(),
            data: Vec::new(),
            str_cache: HashMap::new(),
            types: Vec::new(),
            imports: Vec::new(),
            import_fn: HashMap::new(),
            h: Helpers::default(),
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
        let mut add_import = |em: &mut Emitter, field: &str, p: Vec<ValType>, r: Vec<ValType>| {
            let t = em.ty_idx(p, r);
            em.imports
                .push((EXPORT_MOD.to_string(), field.to_string(), t));
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
        em.h.unbox_char = take();
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
        em.h.arith_int = take();
        em.h.cmp_f64 = take();
        em.h.persist_alloc = take();
        em.h.persist = take();

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
        // A set is a resource, so mirror emit_core_module's resource layout: a
        // persistent reserve below the arena floor, the arena above it.
        let arena_floor = heap_base + PERSIST_RESERVE;
        let pages = (arena_floor as u64 >> 16) + 1;

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

        // Globals, mirroring emit_core_module's resource layout: global 0 = the
        // arena bump pointer (starts above the reserve), 1 = gensym counter,
        // 2 = arena floor, 3 = persistent bump pointer (starts at heap_base).
        let mut gs = GlobalSection::new();
        gs.global(
            GlobalType {
                val_type: I32,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i32_const(arena_floor as i32),
        );
        gs.global(
            GlobalType {
                val_type: ValType::I64,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i64_const(0),
        );
        gs.global(
            GlobalType {
                val_type: I32,
                mutable: false,
                shared: false,
            },
            &ConstExpr::i32_const(arena_floor as i32),
        );
        gs.global(
            GlobalType {
                val_type: I32,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i32_const(heap_base as i32),
        );
        module.section(&gs);

        let mut es = ExportSection::new();
        es.export("memory", ExportKind::Memory, 0);
        es.export("cabi_realloc", ExportKind::Func, em.h.realloc);
        es.export(
            &format!("{IFACE}#[constructor]set"),
            ExportKind::Func,
            fns.ctor,
        );
        es.export(
            &format!("{IFACE}#[method]set.add"),
            ExportKind::Func,
            fns.add,
        );
        es.export(
            &format!("{IFACE}#[method]set.contains"),
            ExportKind::Func,
            fns.contains,
        );
        es.export(
            &format!("{IFACE}#[method]set.size"),
            ExportKind::Func,
            fns.size,
        );
        es.export(&format!("{IFACE}#[dtor]set"), ExportKind::Func, fns.dtor);
        module.section(&es);

        let mut cs = CodeSection::new();
        for (_, f) in &em.bodies {
            cs.function(f);
        }
        module.section(&cs);

        let mut ds = DataSection::new();
        ds.active(
            0,
            &ConstExpr::i32_const(DATA_BASE as i32),
            em.data.iter().copied(),
        );
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
        let bytes = componentize(&WitTy::IntS(4)).expect("componentize + validate");
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
            match c
                .call_instance(IFACE, "[method]set.size", std::slice::from_ref(h))
                .unwrap()[..]
            {
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
        assert_eq!(
            size(&mut c, &handle),
            2,
            "duplicate add is deduped by eq_raw"
        );

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
