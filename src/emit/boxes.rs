//! `Emitter` methods that build value boxes: sequences, records, variants,
//! flags, quote/quasiquote forms, and closure/def-wrapper slots.

use super::*;

impl<'a> Emitter<'a> {
    /// Build a list box `[TAG_LIST, len, elem ptrs…]` from element forms.
    pub(crate) fn list_box(&mut self, fx: &mut FnCtx, items: &[NodeId]) -> Result<(), String> {
        self.seq_box(fx, items, TAG_LIST)
    }

    /// Build a sequence box `[tag, len, elem ptrs…]`; `tag` is TAG_LIST or
    /// TAG_TUP (identical layout, distinct identity at the value level).
    pub(crate) fn seq_box(&mut self, fx: &mut FnCtx, items: &[NodeId], tag: i32) -> Result<(), String> {
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
    pub(crate) fn rec_box(&mut self, fx: &mut FnCtx, fields: &[(String, NodeId)]) -> Result<(), String> {
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
    pub(crate) fn var_box(&mut self, fx: &mut FnCtx, case: &str, args: &[NodeId]) -> Result<(), String> {
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
    pub(crate) fn quote_box(&mut self, fx: &mut FnCtx, id: NodeId) -> Result<(), String> {
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
    pub(crate) fn flg_box(&mut self, fx: &mut FnCtx, names: &[String]) {
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
    pub(crate) fn box_char(&mut self, fx: &mut FnCtx) {
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
    pub(crate) fn quote_seq(&mut self, fx: &mut FnCtx, items: &[NodeId], tag: i32) -> Result<(), String> {
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
    pub(crate) fn quote_rec(&mut self, fx: &mut FnCtx, fields: &[(String, NodeId)]) -> Result<(), String> {
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
    pub(crate) fn quasi_box(&mut self, fx: &mut FnCtx, id: NodeId, depth: u32) -> Result<(), String> {
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
    pub(crate) fn quasi_rebuild_head(
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
    pub(crate) fn quasi_seq(
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
    pub(crate) fn none_like_box(&mut self, case: &str) -> u32 {
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
    pub(crate) fn wrap_variant(&mut self, fx: &mut FnCtx, case: &str) {
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
    pub(crate) fn fn_value_box(&mut self, name: &str) -> Result<u32, String> {
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
    pub(crate) fn def_wrapper_slot(&mut self, name: &str) -> Result<u32, String> {
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
}
