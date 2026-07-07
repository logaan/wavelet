use super::*;

impl<'a> Emitter<'a> {
    /// Each clause is a block: a failed test branches past the clause; a
    /// matched clause leaves its result and branches to the end. No clause
    /// matching traps (the interpreter raises "no Match clause" instead).
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

    /// A list pattern over a canonical (ptr, len) list at `v + off` (5.5):
    /// the length must equal the pattern's arity (checked at runtime — a
    /// list's length is a value property, unlike a tuple's static arity),
    /// then each element sub-pattern destructures the packed element at its
    /// stride offset.
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

    /// A variant-case pattern `(case p…)` over a canonical variant layout
    /// (5.4): the case name resolves to its numeric discriminant at COMPILE
    /// time, so the match is one integer comparison — no runtime case-name
    /// strings — and the payload destructures at the canonical payload
    /// offset. A case the static type lacks, or an arity the case's payload
    /// cannot satisfy, can never match: the clause branches out.
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

    /// A clause's top-level pattern: like [`Self::pattern`], except a bare
    /// binder over a scrutinee with a known scalar kind binds a TYPED local
    /// (unboxed once, at bind time) instead of a box pointer.
    pub(crate) fn pattern_top(
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
    pub(crate) fn pattern(&mut self, fx: &mut FnCtx, pat: NodeId, v: u32, fail: u32) -> Result<(), String> {
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
