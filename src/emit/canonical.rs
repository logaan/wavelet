//! `Emitter` methods for the canonical ABI: lower/lift between boxes and flat
//! core values, and store/load against linear memory, incl. variant chains.

use super::*;

impl<'a> Emitter<'a> {
    /// box on stack → flat value(s) on stack
    pub(crate) fn lower(&mut self, fx: &mut FnCtx, ty: &WitTy) -> Result<(), String> {
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
    pub(crate) fn lower_variant_chain(
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
    pub(crate) fn lower_enum_chain(
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
    pub(crate) fn lower_variant_case(
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
    pub(crate) fn lower_list(&mut self, fx: &mut FnCtx, elem: &WitTy) -> Result<(), String> {
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
    pub(crate) fn lift_list(
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
    pub(crate) fn lift(&mut self, fx: &mut FnCtx, ty: &WitTy) {
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
    pub(crate) fn lift_enum(&mut self, fx: &mut FnCtx, d: u32, cases: &[String], i: usize) {
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
    pub(crate) fn lift_flags(&mut self, fx: &mut FnCtx, v: u32, names: &[String]) {
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
    pub(crate) fn lift_flat(&mut self, fx: &mut FnCtx, ty: &WitTy, base: u32) -> Result<(), String> {
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
    pub(crate) fn lift_variant_flat_chain(
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
    pub(crate) fn lift_variant_case(
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
    pub(crate) fn store_to_mem(
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
    pub(crate) fn store_variant_case(
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
    pub(crate) fn store_variant_chain(
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
    pub(crate) fn load_from_mem(
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
    pub(crate) fn load_variant_case(
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
    pub(crate) fn load_variant_chain(
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
}
