use super::*;

impl<'a> Emitter<'a> {
    pub(crate) fn match_form(
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
                None => self.pattern_top(fx, pair[0], scrut, scrut_kind, scrut_form),
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

    pub(crate) fn pattern_top_mem(
        &mut self,
        fx: &mut FnCtx,
        pat: NodeId,
        v: u32,
        t: MemTy,
    ) -> Result<(), String> {
        let ty = self.mem_tys[t as usize].clone();
        match self.arena.node(pat).clone() {
            Node::Sym(name) if name != "none" && self.local_cases.get(&name) != Some(&false) => {
                fx.scopes
                    .last_mut()
                    .unwrap()
                    .insert(name, Binding::new(v, Repr::Mem(t)));
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

            Node::Sym(name)
                if (name == "none" || self.local_cases.get(&name) == Some(&false))
                    && ty.variant_cases().is_some() =>
            {
                self.pattern_mem_var(fx, &[pat], &ty, v, 0, 0)
            }
            _ => {
                let l = fx.local(ValType::I32);
                self.load_from_mem(fx, &ty, v, 0)?;
                fx.op(I::LocalSet(l));
                self.pattern(fx, pat, l, 0)
            }
        }
    }

    pub(crate) fn pattern_mem_rec(
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
                fx.op(I::Br(fail));
                return Ok(());
            };
            let (o, tf) = &offsets[i];
            self.pattern_mem_field(fx, *p, tf, v, off + o, fail)?;
        }
        Ok(())
    }

    pub(crate) fn pattern_mem_tup(
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

    pub(crate) fn pattern_mem_lst(
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

    pub(crate) fn pattern_mem_var(
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

            (n, Some(WitTy::Tuple(es))) if n > 1 && es.len() == n => {
                let t = WitTy::Tuple(es);
                self.pattern_mem_tup(fx, rest, &t, v, off + poff, fail)
            }

            _ => {
                fx.op(I::Br(fail));
                Ok(())
            }
        }
    }

    pub(crate) fn pattern_mem_field(
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

            Node::Sym(name)
                if (name == "none" || self.local_cases.get(&name) == Some(&false))
                    && tf.variant_cases().is_some() =>
            {
                self.pattern_mem_var(fx, &[pat], tf, v, off, fail)
            }
            _ => {
                let l = fx.local(ValType::I32);
                self.load_from_mem(fx, tf, v, off)?;
                fx.op(I::LocalSet(l));
                self.pattern(fx, pat, l, fail)
            }
        }
    }

    pub(crate) fn mem_field_binding(
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

    pub(crate) fn pattern_top(
        &mut self,
        fx: &mut FnCtx,
        pat: NodeId,
        v: u32,
        scrut_kind: Option<Scalar>,
        scrut_form: NodeId,
    ) -> Result<(), String> {
        if let Node::Tup(pats) = self.arena.node(pat).clone()
            && matches!(
                self.node_types.get(&scrut_form),
                Some(crate::check::Type::Tuple(_))
            )
        {
            return self.seq_pattern(fx, &pats, v, 0, TAG_TUP);
        }
        if let Node::Sym(name) = self.arena.node(pat).clone()
            && name != "none"
            && self.local_cases.get(&name) != Some(&false)
            && let Some(kind) = scrut_kind
        {
            let l = fx.local(repr_vt(Repr::Scalar(kind)));
            fx.op(I::LocalGet(v));
            self.unbox_scalar(fx, kind);
            fx.op(I::LocalSet(l));
            fx.scopes
                .last_mut()
                .unwrap()
                .insert(name, Binding::new(l, Repr::Scalar(kind)));
            return Ok(());
        }
        self.pattern(fx, pat, v, 0)
    }

    pub(crate) fn pattern(
        &mut self,
        fx: &mut FnCtx,
        pat: NodeId,
        v: u32,
        fail: u32,
    ) -> Result<(), String> {
        match self.arena.node(pat).clone() {
            Node::Sym(name) if name == "none" || self.local_cases.get(&name) == Some(&false) => {
                let naddr = self.intern_str(&name);
                fx.op(I::LocalGet(v));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_VAR));
                fx.op(I::I32Ne);
                fx.op(I::BrIf(fail));
                fx.op(I::LocalGet(v));
                fx.op(I::I32Load(ma(8, 2)));
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
                fx.scopes
                    .last_mut()
                    .unwrap()
                    .insert(name, Binding::boxed(l));
                Ok(())
            }

            Node::Dec(_) => Err("a float literal cannot be a Match pattern".into()),
            Node::Int(_) | Node::Bool(_) | Node::Str(_) | Node::Char(_) | Node::Flg(_) => {
                fx.op(I::LocalGet(v));
                self.expr(fx, pat, false)?;
                fx.op(I::Call(self.h.eq_raw));
                fx.op(I::I32Eqz);
                fx.op(I::BrIf(fail));
                Ok(())
            }

            Node::Lst(pats) => self.seq_pattern(fx, &pats, v, fail, TAG_LIST),

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

    pub(crate) fn seq_pattern(
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
}
