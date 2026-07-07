//! WIT-type modelling for the boundary ABI: [`WitTy`], type-string parsing
//! (`wit_ty`), flat-type computation, and the canonical-ABI sizing, alignment,
//! and field-offset math.

use super::*;

// ---------------------------------------------------------------- WIT types

#[derive(Clone, PartialEq)]
pub(crate) enum WitTy {
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
    pub(crate) fn variant_cases(&self) -> Option<Vec<(&str, Option<&WitTy>)>> {
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
pub(crate) fn split_type_args(inner: &str) -> Vec<String> {
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
pub(crate) struct TypeEnv {
    pub(crate) records: HashMap<String, Vec<(String, String)>>,
    pub(crate) defs: HashMap<String, TypeDef>,
    pub(crate) aliases: HashMap<String, String>,
}

pub(crate) fn wit_ty(s: &str, env: &TypeEnv) -> Result<WitTy, String> {
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
pub(crate) fn join_vt(a: ValType, b: ValType) -> ValType {
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
pub(crate) fn join_flat(a: &[ValType], b: &[ValType]) -> Result<Vec<ValType>, String> {
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
pub(crate) fn coerce_flat_to(fx: &mut FnCtx, have: ValType, want: ValType) {
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
pub(crate) fn coerce_flat_from(fx: &mut FnCtx, from: ValType, to: ValType) {
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

pub(crate) fn flat(ty: &WitTy) -> Vec<ValType> {
    flat_checked(ty).expect("flat() on an unsupported boundary type")
}

/// Number of flat (core) values a type lowers to. Unlike [`flat_checked`] this
/// never needs the variant-join to succeed — it just counts — so it is safe to
/// use when only the count matters (deciding direct return vs retptr).
pub(crate) fn flat_len(ty: &WitTy) -> usize {
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

pub(crate) fn flat_checked(ty: &WitTy) -> Result<Vec<ValType>, String> {
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
pub(crate) fn align_of(ty: &WitTy) -> u64 {
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
pub(crate) fn disc_size(n: usize) -> u64 {
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
pub(crate) fn flags_align(n: usize) -> u64 {
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
pub(crate) fn variant_payload_offset(ty: &WitTy) -> u64 {
    let n = ty.variant_cases().map(|c| c.len()).unwrap_or(0);
    align_up(disc_size(n), align_of(ty))
}

/// Canonical-ABI size (bytes) in memory.
pub(crate) fn size_of(ty: &WitTy) -> u64 {
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
pub(crate) fn flags_size(n: usize) -> u64 {
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
pub(crate) fn flags_is_ordered_subseq(lit: &[String], decl: &[String]) -> bool {
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

pub(crate) fn align_up(off: u64, align: u64) -> u64 {
    off.div_ceil(align) * align
}

/// (offset, field-type) for each field of a record or element of a tuple, in
/// declaration order. Tuples lay out exactly like records (canonical-ABI treats
/// them identically — positional fields with the same alignment rules).
pub(crate) fn record_field_offsets(ty: &WitTy) -> Vec<(u64, WitTy)> {
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
pub(crate) fn is_byte_elem(ty: &WitTy) -> bool {
    matches!(ty, WitTy::IntU(1) | WitTy::IntS(1))
}

pub(crate) fn elem_size(ty: &WitTy) -> u64 {
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

pub(crate) enum FlatRes {
    None,
    One(WitTy),
    Retptr, // flattened result > 1 value (string/list/record): pass/return a pointer
}

pub(crate) fn flat_result(sig: &FuncSig, env: &TypeEnv) -> Result<FlatRes, String> {
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
