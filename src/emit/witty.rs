use super::*;

#[derive(Clone, PartialEq)]
pub(crate) enum WitTy {
    Bool,
    Char,
    IntS(u8),
    IntU(u8),
    S64,

    F32,
    F64,
    Str,
    List(Box<WitTy>),
    Record(Vec<(String, WitTy)>),

    Tuple(Vec<WitTy>),
    Option(Box<WitTy>),
    Result(Box<WitTy>, Box<WitTy>),

    Handle,

    Enum(Vec<String>),

    Variant(Vec<(String, Option<WitTy>)>),

    Flags(Vec<String>),
}

impl WitTy {
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

#[derive(Clone)]
pub enum TypeDef {
    Resource,

    Enum(Vec<String>),

    Variant(Vec<(String, Option<String>)>),

    Flags(Vec<String>),
}

#[derive(Default)]
pub(crate) struct TypeEnv {
    pub(crate) records: HashMap<String, Vec<(String, String)>>,
    pub(crate) defs: HashMap<String, TypeDef>,
    pub(crate) aliases: HashMap<String, String>,
}

pub(crate) fn wit_ty(s: &str, env: &TypeEnv) -> Result<WitTy, String> {
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
                return wit_ty(target, env);
            } else {
                return Err(format!(
                    "type `{other}` not supported by the wasm backend yet"
                ));
            }
        }
    })
}

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

pub(crate) fn join_flat(a: &[ValType], b: &[ValType]) -> Result<Vec<ValType>, String> {
    let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    let mut out = long.to_vec();
    for (i, t) in short.iter().enumerate() {
        out[i] = join_vt(out[i], *t);
    }
    Ok(out)
}

pub(crate) fn coerce_flat_to(fx: &mut FnCtx, have: ValType, want: ValType) {
    use ValType::{F32, F64, I32, I64};
    match (have, want) {
        _ if have == want => {}

        (I32, I64) => fx.op(I::I64ExtendI32U),

        (F32, I32) => fx.op(I::I32ReinterpretF32),
        (F32, I64) => {
            fx.op(I::I32ReinterpretF32);
            fx.op(I::I64ExtendI32U);
        }

        (F64, I64) => fx.op(I::I64ReinterpretF64),

        _ => {}
    }
}

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
            let mut v = vec![ValType::I32];
            v.extend(joined);
            v
        }
    })
}

pub(crate) fn align_of(ty: &WitTy) -> u64 {
    match ty {
        WitTy::Bool => 1,
        WitTy::Char | WitTy::Handle => 4,
        WitTy::IntS(w) | WitTy::IntU(w) => *w as u64,
        WitTy::F32 => 4,
        WitTy::S64 | WitTy::F64 => 8,
        WitTy::Str | WitTy::List(_) => 4,
        WitTy::Record(fields) => fields.iter().map(|(_, t)| align_of(t)).max().unwrap_or(1),
        WitTy::Tuple(elems) => elems.iter().map(align_of).max().unwrap_or(1),

        WitTy::Enum(cases) => disc_size(cases.len()),
        WitTy::Flags(names) => flags_align(names.len()),
        WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_) => {
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

pub(crate) fn disc_size(n: usize) -> u64 {
    if n <= 0x100 {
        1
    } else if n <= 0x10000 {
        2
    } else {
        4
    }
}

pub(crate) fn flags_align(n: usize) -> u64 {
    if n <= 8 {
        1
    } else if n <= 16 {
        2
    } else {
        4
    }
}

pub(crate) fn variant_payload_offset(ty: &WitTy) -> u64 {
    let n = ty.variant_cases().map(|c| c.len()).unwrap_or(0);
    align_up(disc_size(n), align_of(ty))
}

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

pub(crate) fn flags_size(n: usize) -> u64 {
    if n <= 8 {
        1
    } else if n <= 16 {
        2
    } else {
        (n as u64).div_ceil(32) * 4
    }
}

pub(crate) fn flags_is_ordered_subseq(lit: &[String], decl: &[String]) -> bool {
    let mut di = 0usize;
    for name in lit {
        match decl[di..].iter().position(|d| d == name) {
            Some(rel) => di += rel + 1,
            None => return false,
        }
    }
    true
}

pub(crate) fn align_up(off: u64, align: u64) -> u64 {
    off.div_ceil(align) * align
}

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
    Retptr,
}

pub(crate) fn flat_result(sig: &FuncSig, env: &TypeEnv) -> Result<FlatRes, String> {
    match &sig.result {
        None => Ok(FlatRes::None),
        Some(t) => {
            let ty = wit_ty(t, env)?;

            if flat_len(&ty) > 1 {
                Ok(FlatRes::Retptr)
            } else {
                Ok(FlatRes::One(ty))
            }
        }
    }
}
