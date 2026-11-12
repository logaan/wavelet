use super::*;

mod text;

fn expect_arity(name: &str, items: &[NodeId], expected: usize) -> Result<(), String> {
    if items.len() == expected {
        Ok(())
    } else {
        Err(format!(
            "`{name}` expects {expected} argument(s), got {}",
            items.len()
        ))
    }
}

impl<'a> Emitter<'a> {
    pub(crate) fn builtin(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        args: &[NodeId],
    ) -> Result<(), String> {
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

        match name {
            "eq" | "not" | "lt" | "le" | "gt" | "ge" | "add" | "sub" | "mul" | "div" | "rem"
            | "neg" | "abs" | "min" | "max" => self.builtin_numeric(fx, name, items),
            "len" | "head" | "tail" | "reverse" | "range" | "empty" | "drop" => {
                self.builtin_seq(fx, name, items)
            }
            "map" | "fold" | "filter" | "zip" => self.builtin_higher_order(fx, name, items),
            "get" | "put" | "push" | "concat" => self.builtin_index(fx, name, items),
            "contains" | "join" | "split" => self.builtin_search(fx, name, items),
            "str-cat" | "to-string" | "to-char" => self.builtin_string(fx, name, items),
            "some" | "ok" | "err" => self.builtin_variant(fx, name, items),
            "cell-new" | "cell-get" | "cell-set" => self.builtin_cell(fx, name, items),
            "form-kind" | "rec-key" | "rec-val" | "gensym" | "expand" => {
                self.builtin_form(fx, name, items)
            }
            _ if crate::builtins::tuple_ctor_arity(name).is_some() => {
                self.builtin_tuple(fx, name, items)
            }
            other => Err(format!(
                "builtin `{other}` not supported by the wasm backend yet"
            )),
        }
    }

    pub(crate) fn builtin_tuple(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        items: &[NodeId],
    ) -> Result<(), String> {
        let n = crate::builtins::tuple_ctor_arity(name).expect("routed by tuple_ctor_arity");
        expect_arity(name, items, n)?;
        self.seq_box(fx, items, TAG_TUP)
    }

    pub(crate) fn builtin_numeric(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        items: &[NodeId],
    ) -> Result<(), String> {
        match name {
            "eq" => {
                expect_arity(name, items, 2)?;

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
                expect_arity(name, items, 1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.truthy));
                fx.op(I::I32Eqz);
                fx.op(I::Call(self.h.box_bool));
            }
            "lt" | "le" | "gt" | "ge" => {
                expect_arity(name, items, 2)?;
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
                expect_arity(name, items, 2)?;
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
                expect_arity(name, items, 1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.neg_raw));
            }
            "abs" => {
                expect_arity(name, items, 1)?;
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
                fx.op(I::I32Const(1));
                fx.op(I::Call(self.h.arith_int));
                fx.op(I::Else);
                fx.op(I::LocalGet(n));
                fx.op(I::End);
                fx.op(I::Call(self.h.box_int));
                fx.op(I::Else);

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
                expect_arity(name, items, 2)?;
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
            _ => unreachable!("builtin_numeric routed unexpected name `{name}`"),
        }
        Ok(())
    }

    pub(crate) fn builtin_seq(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        items: &[NodeId],
    ) -> Result<(), String> {
        match name {
            "len" => {
                expect_arity(name, items, 1)?;

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
                expect_arity(name, items, 1)?;

                if let Some(mt) = self.node_mem(fx, items[0])
                    && let WitTy::List(elem) = self.mem_tys[mt as usize].clone()
                {
                    let area = fx.local(ValType::I32);
                    self.expr_mem(fx, items[0], mt, false)?;
                    fx.op(I::LocalSet(area));

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
                expect_arity(name, items, 1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.tail_h));
            }
            "reverse" => {
                expect_arity(name, items, 1)?;
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
                expect_arity(name, items, 2)?;
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
            "empty" => {
                expect_arity(name, items, 1)?;
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
                for &x in items {
                    self.expr(fx, x, false)?;
                    fx.op(I::Drop);
                }
                let addr = self.intern_unit_rec();
                fx.op(I::I32Const(addr as i32));
            }
            _ => unreachable!("builtin_seq routed unexpected name `{name}`"),
        }
        Ok(())
    }

    pub(crate) fn builtin_higher_order(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        items: &[NodeId],
    ) -> Result<(), String> {
        match name {
            "map" => {
                expect_arity(name, items, 2)?;
                let fp = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(fp));
                let lp = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::LocalSet(lp));
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(lp));
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
                let apply_ty = self.ty_idx(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
                fx.op(I::I32Const(0));
                fx.op(I::LocalSet(i));
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
                fx.op(I::LocalGet(i));
                fx.op(I::LocalGet(n));
                fx.op(I::I32GeU);
                fx.op(I::BrIf(1));

                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);

                fx.op(I::LocalGet(fp));

                fx.op(I::LocalGet(lp));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));

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
                fx.op(I::End);
                fx.op(I::End);
                fx.op(I::LocalGet(out));
            }
            "fold" => {
                expect_arity(name, items, 3)?;
                let fp = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(fp));
                let acc = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::LocalSet(acc));
                let lp = fx.local(ValType::I32);
                self.expr(fx, items[2], false)?;
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
                fx.op(I::End);
                fx.op(I::End);
                fx.op(I::LocalGet(acc));
            }
            "filter" => {
                expect_arity(name, items, 2)?;
                let fp = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(fp));
                let lp = fx.local(ValType::I32);
                self.expr(fx, items[1], false)?;
                fx.op(I::LocalSet(lp));
                let n = fx.local(ValType::I32);
                fx.op(I::LocalGet(lp));
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

                fx.op(I::LocalGet(lp));
                fx.op(I::LocalGet(i));
                fx.op(I::I32Const(4));
                fx.op(I::I32Mul);
                fx.op(I::I32Add);
                fx.op(I::I32Const(8));
                fx.op(I::I32Add);
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::LocalSet(elem));

                fx.op(I::LocalGet(fp));
                fx.op(I::LocalGet(elem));
                fx.op(I::LocalGet(fp));
                fx.op(I::I32Load(ma(4, 2)));
                fx.op(I::CallIndirect {
                    type_index: apply_ty,
                    table_index: 0,
                });
                fx.op(I::LocalSet(r));

                fx.op(I::LocalGet(r));
                fx.op(I::I32Load(ma(0, 2)));
                fx.op(I::I32Const(TAG_BOOL));
                fx.op(I::I32Ne);
                fx.op(I::If(BlockType::Empty));
                fx.op(I::Unreachable);
                fx.op(I::End);

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
                fx.op(I::End);
                fx.op(I::End);

                fx.op(I::LocalGet(out));
                fx.op(I::LocalGet(k));
                fx.op(I::I32Store(ma(4, 2)));
                fx.op(I::LocalGet(out));
            }
            "zip" => {
                expect_arity(name, items, 2)?;
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
            _ => unreachable!("builtin_higher_order routed unexpected name `{name}`"),
        }
        Ok(())
    }

    pub(crate) fn builtin_index(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        items: &[NodeId],
    ) -> Result<(), String> {
        match name {
            "get" => {
                expect_arity(name, items, 2)?;
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
                expect_arity(name, items, 3)?;
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
                fx.op(I::MemoryCopy {
                    src_mem: 0,
                    dst_mem: 0,
                });
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
                expect_arity(name, items, 2)?;
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
                fx.op(I::MemoryCopy {
                    src_mem: 0,
                    dst_mem: 0,
                });
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
                expect_arity(name, items, 2)?;
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
                fx.op(I::MemoryCopy {
                    src_mem: 0,
                    dst_mem: 0,
                });
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
                fx.op(I::MemoryCopy {
                    src_mem: 0,
                    dst_mem: 0,
                });
                fx.op(I::LocalGet(out));
            }
            _ => unreachable!("builtin_index routed unexpected name `{name}`"),
        }
        Ok(())
    }

    pub(crate) fn builtin_search(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        items: &[NodeId],
    ) -> Result<(), String> {
        match name {
            "join" => {
                expect_arity(name, items, 2)?;
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

                fx.op(I::LocalGet(i));
                fx.op(I::If(BlockType::Empty));
                fx.op(I::LocalGet(acc));
                fx.op(I::LocalGet(sep));
                fx.op(I::Call(self.h.strcat2));
                fx.op(I::LocalSet(acc));
                fx.op(I::End);

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
                expect_arity(name, items, 2)?;
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
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
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
                fx.op(I::Block(BlockType::Empty));
                fx.op(I::Loop(BlockType::Empty));
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
                fx.op(I::End);
                fx.op(I::End);
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
                fx.op(I::End);
                fx.op(I::End);
                fx.op(I::LocalGet(found));
                fx.op(I::Call(self.h.box_bool));
            }
            "split" => return self.builtin_split(fx, items),
            _ => unreachable!("builtin_search routed unexpected name `{name}`"),
        }
        Ok(())
    }

    pub(crate) fn builtin_split(&mut self, fx: &mut FnCtx, items: &[NodeId]) -> Result<(), String> {
        if items.len() != 2 {
            return Err(format!(
                "`split` expects 2 argument(s), got {}",
                items.len()
            ));
        }
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

        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(empty));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::I32Const(1));
        fx.op(I::LocalSet(oi));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(slen));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalSet(start));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));

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

        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(out));
        Ok(())
    }

    pub(crate) fn builtin_variant(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        items: &[NodeId],
    ) -> Result<(), String> {
        match name {
            "some" | "ok" | "err" => {
                if items.is_empty() && name != "some" {
                    let addr = self.none_like_box(name);
                    fx.op(I::I32Const(addr as i32));
                    return Ok(());
                }
                self.var_box(fx, name, items)
            }
            _ => unreachable!("builtin_variant routed unexpected name `{name}`"),
        }
    }

    pub(crate) fn builtin_cell(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        items: &[NodeId],
    ) -> Result<(), String> {
        match name {
            "cell-new" => {
                expect_arity(name, items, 1)?;
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
                expect_arity(name, items, 1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::I32Load(ma(4, 2)));
            }
            "cell-set" => {
                expect_arity(name, items, 2)?;
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
            _ => unreachable!("builtin_cell routed unexpected name `{name}`"),
        }
        Ok(())
    }

    pub(crate) fn builtin_form(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        items: &[NodeId],
    ) -> Result<(), String> {
        match name {
            "form-kind" => {
                expect_arity(name, items, 1)?;
                return self.form_kind(fx, items[0]);
            }
            "rec-key" => {
                expect_arity(name, items, 1)?;
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
                expect_arity(name, items, 1)?;
                let rp = fx.local(ValType::I32);
                self.expr(fx, items[0], false)?;
                fx.op(I::LocalSet(rp));
                self.rec_guard(fx, rp);
                fx.op(I::LocalGet(rp));
                fx.op(I::I32Load(ma(12, 2)));
            }
            "gensym" => {
                expect_arity(name, items, 0)?;
                return self.gensym(fx);
            }
            "expand" => {
                expect_arity(name, items, 1)?;
                match self.macro_expand_idx {
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
            _ => unreachable!("builtin_form routed unexpected name `{name}`"),
        }
        Ok(())
    }
}

pub(crate) const BUILTINS: &[&str] = &[
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
    "head",
    "tail",
    "map",
    "fold",
    "filter",
    "str-cat",
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
    "form-kind",
    "rec-key",
    "rec-val",
    "gensym",
    "expand",
    "tuple0",
    "tuple1",
    "tuple2",
    "tuple3",
    "tuple4",
    "tuple5",
    "tuple6",
    "tuple7",
    "tuple8",
    "tuple9",
    "tuple10",
    "tuple11",
    "tuple12",
    "tuple13",
    "tuple14",
    "tuple15",
    "tuple16",
];
