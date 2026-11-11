use super::*;

impl<'a> Emitter<'a> {
    pub(crate) fn mem_eq_eligible(&self, ty: &WitTy) -> bool {
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

    pub(crate) fn emit_mem_eq(
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

                self.emit_variant_payload_eq(fx, &cases, da, a, b, poff, 0)?;
                fx.op(I::Else);
                fx.op(I::I32Const(0));
                fx.op(I::End);
            }
            WitTy::Str => {
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

    pub(crate) fn emit_variant_payload_eq(
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

    pub(crate) fn emit_bytes_eq(&mut self, fx: &mut FnCtx, pa: u32, pb: u32, len: u32) {
        let i = fx.local(ValType::I32);
        let res = fx.local(ValType::I32);
        fx.op(I::I32Const(1));
        fx.op(I::LocalSet(res));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));

        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(len));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));

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

    pub(crate) fn emit_list_eq(
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

    pub(crate) fn form_kind(&mut self, fx: &mut FnCtx, arg: NodeId) -> Result<(), String> {
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

    pub(crate) fn rec_guard(&mut self, fx: &mut FnCtx, rp: u32) {
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

    pub(crate) fn gensym(&mut self, fx: &mut FnCtx) -> Result<(), String> {
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

    pub(crate) fn node_scalar(&self, id: NodeId) -> Option<Scalar> {
        Scalar::of(self.node_types.get(&id)?)
    }

    pub(crate) fn unbox_scalar(&mut self, fx: &mut FnCtx, kind: Scalar) {
        match kind {
            Scalar::Int => fx.op(I::Call(self.h.unbox_int)),
            Scalar::Float => fx.op(I::Call(self.h.as_f64)),
            Scalar::Bool => fx.op(I::Call(self.h.truthy)),
            Scalar::Char => fx.op(I::Call(self.h.unbox_char)),
        }
    }

    pub(crate) fn box_scalar(&mut self, fx: &mut FnCtx, kind: Scalar) {
        match kind {
            Scalar::Int => fx.op(I::Call(self.h.box_int)),
            Scalar::Float => fx.op(I::Call(self.h.box_dec)),
            Scalar::Bool => fx.op(I::Call(self.h.box_bool)),
            Scalar::Char => self.box_char(fx),
        }
    }

    pub(crate) fn mem_ty(&mut self, ty: &WitTy) -> MemTy {
        if let Some(i) = self.mem_tys.iter().position(|t| t == ty) {
            return i as MemTy;
        }
        self.mem_tys.push(ty.clone());
        (self.mem_tys.len() - 1) as MemTy
    }

    pub(crate) fn wit_of_check_type(&self, t: &crate::check::Type) -> Option<WitTy> {
        use crate::check::Type as T;
        Some(match t {
            T::Bool => WitTy::Bool,
            T::U8 => WitTy::IntU(1),
            T::U16 => WitTy::IntU(2),
            T::U32 => WitTy::IntU(4),
            T::S8 => WitTy::IntS(1),
            T::S16 => WitTy::IntS(2),
            T::S32 => WitTy::IntS(4),

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

            T::Named(n) => return wit_ty(n, &self.type_env).ok(),
            _ => return None,
        })
    }

    pub(crate) fn ctor_parts(
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

            if self.local_cases.get(&head) != Some(&true) {
                return None;
            }
        }
        let i = cases.iter().position(|(n, _)| *n == head)?;
        let payload = cases[i].1.cloned();
        Some((i, payload, args))
    }

    pub(crate) fn ctor_admissible(&self, look: &MemLookup, id: NodeId, ty: &WitTy) -> bool {
        let fx = match look {
            MemLookup::Fx(fx) => Some(*fx),
            MemLookup::Sim(_) => None,
        };
        let Some((_, payload, args)) = self.ctor_parts(fx, id, ty) else {
            return false;
        };

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

    pub(crate) fn tuple_ctor_args(
        &self,
        look: &MemLookup,
        id: NodeId,
        ty: &WitTy,
    ) -> Option<Vec<NodeId>> {
        let WitTy::Tuple(es) = ty else { return None };
        let Node::Tup(items) = self.arena.node(id) else {
            return None;
        };
        let Node::Sym(head) = self.arena.node(*items.first()?) else {
            return None;
        };
        let n = crate::builtins::tuple_ctor_arity(head)?;
        if n == 0 || n != es.len() || items.len() - 1 != n {
            return None;
        }
        if let MemLookup::Fx(fx) = look
            && fx.lookup(head).is_some()
        {
            return None;
        }
        if self.funcs.contains_key(head.as_str())
            || self.value_globals.contains_key(head.as_str())
            || self.local_cases.contains_key(head.as_str())
        {
            return None;
        }
        Some(items[1..].to_vec())
    }

    pub(crate) fn tuple_ctor_admissible(&self, look: &MemLookup, id: NodeId, ty: &WitTy) -> bool {
        let Some(args) = self.tuple_ctor_args(look, id, ty) else {
            return false;
        };
        let WitTy::Tuple(es) = ty else { return false };
        args.iter()
            .zip(es)
            .all(|(&a, et)| self.mem_field_ok(look, a, et))
    }

    pub(crate) fn can_mem_as(&self, look: &MemLookup, id: NodeId, ty: &WitTy) -> bool {
        match (self.arena.node(id), ty) {
            (Node::Rec(fields), WitTy::Record(tfs)) if !fields.is_empty() => {
                fields.len() == tfs.len()
                    && fields
                        .iter()
                        .zip(tfs)
                        .all(|((k, v), (tk, tf))| k == tk && self.mem_field_ok(look, *v, tf))
            }
            (Node::Lst(items), WitTy::List(elem)) => {
                items.iter().all(|&v| self.mem_field_ok(look, v, elem))
            }
            (Node::Sym(name), _) => {
                self.lookup_mem(look, name).is_some_and(|t| t == *ty)
                    || (self.lookup_mem(look, name).is_none() && self.ctor_admissible(look, id, ty))
            }

            (
                Node::Tup(items),
                WitTy::Record(_)
                | WitTy::Tuple(_)
                | WitTy::Str
                | WitTy::List(_)
                | WitTy::Option(_)
                | WitTy::Result(..)
                | WitTy::Variant(_),
            ) if !items.is_empty() => match self.arena.node(items[0]) {
                Node::Sym(name) if crate::wit::qualified_name(self.info, name).is_some() => {
                    let (alias, function) = crate::wit::qualified_name(self.info, name).unwrap();
                    self.dep_result_mem_ty(&alias, &function)
                        .is_some_and(|t| t == *ty)
                }

                Node::Sym(name)
                    if name.as_str() == "tail"
                        && matches!(ty, WitTy::List(_))
                        && items.len() == 2 =>
                {
                    self.node_mem_ty(look, items[1]).as_ref() == Some(ty)
                }

                Node::Sym(head)
                    if crate::builtins::tuple_ctor_arity(head).is_some()
                        && matches!(ty, WitTy::Tuple(_)) =>
                {
                    self.tuple_ctor_admissible(look, id, ty)
                }

                Node::Sym(_) => self.ctor_admissible(look, id, ty),
                _ => false,
            },
            _ => false,
        }
    }

    pub(crate) fn dep_result_mem_ty(&self, alias: &str, fname: &str) -> Option<WitTy> {
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
                WitTy::Str
                    | WitTy::List(_)
                    | WitTy::Option(_)
                    | WitTy::Result(..)
                    | WitTy::Variant(_)
            ))
        .then_some(rty)
    }

    pub(crate) fn lookup_mem(&self, look: &MemLookup, name: &str) -> Option<WitTy> {
        match look {
            MemLookup::Fx(fx) => match fx.lookup(name)?.repr {
                Repr::Mem(t) => Some(self.mem_tys[t as usize].clone()),
                _ => None,
            },
            MemLookup::Sim(env) => env.iter().rev().find_map(|scope| scope.get(name)).cloned(),
        }
    }

    pub(crate) fn mem_field_ok(&self, look: &MemLookup, v: NodeId, tf: &WitTy) -> bool {
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

            WitTy::Tuple(_) => self.tuple_ctor_admissible(look, v, tf),

            WitTy::Flags(decl) => match self.arena.node(v) {
                Node::Flg(names) => flags_is_ordered_subseq(names, decl),
                _ => false,
            },

            _ => false,
        }
    }

    pub(crate) fn node_mem_ty(&self, look: &MemLookup, id: NodeId) -> Option<WitTy> {
        let t = self.node_types.get(&id)?.clone();
        let ty = self.wit_of_check_type(&t)?;
        if !(matches!(&ty, WitTy::Record(fs) if !fs.is_empty())
            || matches!(&ty, WitTy::Tuple(es) if !es.is_empty())
            || matches!(
                &ty,
                WitTy::Str
                    | WitTy::List(_)
                    | WitTy::Option(_)
                    | WitTy::Result(..)
                    | WitTy::Variant(_)
            ))
        {
            return None;
        }
        if !self.can_mem_as(look, id, &ty) {
            return None;
        }
        Some(ty)
    }

    pub(crate) fn node_mem(&mut self, fx: &FnCtx, id: NodeId) -> Option<MemTy> {
        let ty = self.node_mem_ty(&MemLookup::Fx(fx), id)?;
        Some(self.mem_ty(&ty))
    }

    pub(crate) fn def_sig(&mut self, params_id: NodeId, body: NodeId) -> FnSig {
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

    pub(crate) fn predict_body_mem(
        &self,
        id: NodeId,
        env: &mut Vec<HashMap<String, WitTy>>,
    ) -> Option<WitTy> {
        match self.arena.node(id).clone() {
            Node::Rec(_) | Node::Lst(_) => self.node_mem_ty(&MemLookup::Sim(env), id),
            Node::Sym(name) => env
                .iter()
                .rev()
                .find_map(|s| s.get(&name))
                .cloned()
                .or_else(|| self.node_mem_ty(&MemLookup::Sim(env), id)),
            Node::Tup(items) if !items.is_empty() => {
                if matches!(self.arena.node(items[0]), Node::Sym(name) if crate::wit::qualified_name(self.info, name).is_some())
                {
                    return self.node_mem_ty(&MemLookup::Sim(env), id);
                }
                let Node::Sym(head) = self.arena.node(items[0]).clone() else {
                    return None;
                };
                let args = &items[1..];
                match (head.as_str(), args) {
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

                    _ => self.node_mem_ty(&MemLookup::Sim(env), id),
                }
            }
            _ => None,
        }
    }

    pub(crate) fn expr_mem(
        &mut self,
        fx: &mut FnCtx,
        id: NodeId,
        t: MemTy,
        tail: bool,
    ) -> Result<(), String> {
        let ty = self.mem_tys[t as usize].clone();
        match self.arena.node(id).clone() {
            Node::Sym(name) => match fx.lookup(&name) {
                Some(b) => match b.repr {
                    Repr::Mem(bt) if bt == t => {
                        fx.op(I::LocalGet(b.local));
                        Ok(())
                    }

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
            },
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
                let p = fx.local(ValType::I32);
                fx.op(I::I32Const(8));
                fx.op(I::Call(self.h.alloc));
                fx.op(I::LocalSet(p));
                self.mem_field_into(fx, id, &ty, p, 0)?;
                fx.op(I::LocalGet(p));
                Ok(())
            }
            Node::Tup(items) if !items.is_empty() => {
                if let Node::Sym(name) = self.arena.node(items[0])
                    && let Some((alias, fname)) = crate::wit::qualified_name(self.info, name)
                {
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

                        "tail" if matches!(ty, WitTy::List(_)) => {
                            let WitTy::List(elem) = ty.clone() else {
                                unreachable!()
                            };
                            let [operand] = args else {
                                return Err("malformed tail".into());
                            };
                            let ot = self
                                .node_mem(fx, *operand)
                                .ok_or("internal: canonical tail over a non-canonical operand")?;
                            let area = fx.local(ValType::I32);
                            self.expr_mem(fx, *operand, ot, false)?;
                            fx.op(I::LocalSet(area));

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

                            fx.op(I::LocalGet(p));
                            fx.op(I::LocalGet(area));
                            fx.op(I::I32Load(ma(0, 2)));
                            fx.op(I::I32Const(elem_size(&elem) as i32));
                            fx.op(I::I32Add);
                            fx.op(I::I32Store(ma(0, 2)));

                            fx.op(I::LocalGet(p));
                            fx.op(I::LocalGet(area));
                            fx.op(I::I32Load(ma(4, 2)));
                            fx.op(I::I32Const(1));
                            fx.op(I::I32Sub);
                            fx.op(I::I32Store(ma(4, 2)));
                            fx.op(I::LocalGet(p));
                            return Ok(());
                        }

                        _ if crate::builtins::tuple_ctor_arity(&head).is_some()
                            && matches!(ty, WitTy::Tuple(_)) =>
                        {
                            let a = fx.local(ValType::I32);
                            fx.op(I::I32Const(size_of(&ty) as i32));
                            fx.op(I::Call(self.h.alloc));
                            fx.op(I::LocalSet(a));
                            self.mem_tuple_into(fx, id, &ty, a, 0)?;
                            fx.op(I::LocalGet(a));
                            return Ok(());
                        }
                        _ if ty.variant_cases().is_some() => {
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

    pub(crate) fn expr_mem_into(
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

    pub(crate) fn mem_var_into(
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

    pub(crate) fn mem_tuple_into(
        &mut self,
        fx: &mut FnCtx,
        id: NodeId,
        ty: &WitTy,
        dst: u32,
        off: u64,
    ) -> Result<(), String> {
        let args = {
            let look = MemLookup::Fx(fx);
            self.tuple_ctor_args(&look, id, ty)
                .filter(|_| self.tuple_ctor_admissible(&look, id, ty))
        };
        let Some(args) = args else {
            let l = fx.local(ValType::I32);
            self.expr(fx, id, false)?;
            fx.op(I::LocalSet(l));
            return self.store_to_mem(fx, ty, l, dst, off);
        };
        for ((o, et), &a) in record_field_offsets(ty).into_iter().zip(&args) {
            self.mem_field_into(fx, a, &et, dst, off + o)?;
        }
        Ok(())
    }

    pub(crate) fn mem_field_into(
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
            WitTy::Tuple(_) => self.mem_tuple_into(fx, v, tf, dst, off)?,
            WitTy::Option(_) | WitTy::Result(..) | WitTy::Variant(_) => {
                self.mem_var_into(fx, v, tf, dst, off)?;
            }
            WitTy::Flags(decl) => {
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

    pub(crate) fn expr_scalar(
        &mut self,
        fx: &mut FnCtx,
        id: NodeId,
        want: Scalar,
    ) -> Result<(), String> {
        self.expr_scalar_t(fx, id, want, false)
    }

    pub(crate) fn expr_repr(
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

    pub(crate) fn expr_scalar_t(
        &mut self,
        fx: &mut FnCtx,
        id: NodeId,
        want: Scalar,
        tail: bool,
    ) -> Result<(), String> {
        match self.arena.node(id).clone() {
            Node::Int(n) => {
                if want == Scalar::Float {
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

        self.expr(fx, id, false)?;
        if self.node_scalar(id) == Some(Scalar::Int) && want == Scalar::Float {
            fx.op(I::Call(self.h.unbox_int));
            fx.op(I::F64ConvertI64S);
        } else {
            self.unbox_scalar(fx, want);
        }
        Ok(())
    }

    pub(crate) fn scalar_op(
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
                    self.expr_scalar(fx, args[0], Float)?;
                    self.expr_scalar(fx, args[1], Float)?;
                    match name {
                        "add" => fx.op(I::F64Add),
                        "sub" => fx.op(I::F64Sub),
                        "mul" => fx.op(I::F64Mul),
                        "div" => fx.op(I::F64Div),
                        _ => {
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
                    fx.op(I::I64Const(0));
                    self.expr_scalar(fx, args[0], Int)?;
                    fx.op(I::I32Const(1));
                    fx.op(I::Call(self.h.arith_int));
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
                        self.expr_scalar(fx, args[0], Char)?;
                        self.expr_scalar(fx, args[1], Char)?;
                    }
                    (Int | Float, Int | Float) => {
                        self.expr_scalar(fx, args[0], Float)?;
                        self.expr_scalar(fx, args[1], Float)?;
                        fx.op(I::Call(self.h.cmp_f64));
                        fx.op(I::I64ExtendI32S);
                        fx.op(I::I64Const(0));
                    }

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

            "to-u8" | "to-u16" | "to-u32" | "to-u64" | "to-s8" | "to-s16" | "to-s32" | "to-s64"
                if args.len() == 1 =>
            {
                match self.node_scalar(args[0]) {
                    Some(k @ (Int | Char)) => self.expr_scalar(fx, args[0], k)?,
                    Some(Float) => {
                        self.expr_scalar(fx, args[0], Float)?;
                        let f = fx.local(ValType::F64);
                        fx.op(I::LocalTee(f));
                        fx.op(I::LocalGet(f));
                        fx.op(I::F64Trunc);
                        fx.op(I::F64Ne);
                        fx.op(I::If(BlockType::Empty));
                        fx.op(I::Unreachable);
                        fx.op(I::End);
                        fx.op(I::LocalGet(f));
                        fx.op(I::I64TruncSatF64S);
                    }
                    _ => return Ok(None),
                }

                let range = match name {
                    "to-u8" => Some((0, u8::MAX as i64)),
                    "to-u16" => Some((0, u16::MAX as i64)),
                    "to-u32" => Some((0, u32::MAX as i64)),
                    "to-u64" => Some((0, i64::MAX)),
                    "to-s8" => Some((i8::MIN as i64, i8::MAX as i64)),
                    "to-s16" => Some((i16::MIN as i64, i16::MAX as i64)),
                    "to-s32" => Some((i32::MIN as i64, i32::MAX as i64)),
                    _ => None,
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
                    self.expr_scalar(fx, args[0], Float)?;
                    Ok(Some(Float))
                }
                _ => Ok(None),
            },
            "to-char" if args.len() == 1 => match self.node_scalar(args[0]) {
                Some(Char) => {
                    self.expr_scalar(fx, args[0], Char)?;
                    Ok(Some(Char))
                }
                Some(Int) => {
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
}
