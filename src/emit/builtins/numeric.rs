use super::*;

impl Emitter<'_> {
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
                let order = fx.local(ValType::I32);
                fx.op(I::LocalTee(order));
                fx.op(I::I32Const(0));
                fx.op(match name {
                    "lt" => I::I32LtS,
                    "le" => I::I32LeS,
                    "gt" => I::I32GtS,
                    _ => I::I32GeS,
                });
                fx.op(I::LocalGet(order));
                fx.op(I::I32Const(2));
                fx.op(I::I32Ne);
                fx.op(I::I32And);
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
                self.builtin_extrema(fx, name, items)?;
            }
            _ => unreachable!("builtin_numeric routed unexpected name `{name}`"),
        }
        Ok(())
    }

    fn builtin_extrema(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        items: &[NodeId],
    ) -> Result<(), String> {
        expect_arity(name, items, 2)?;
        let av = fx.local(ValType::I32);
        self.expr(fx, items[0], false)?;
        fx.op(I::LocalSet(av));
        let bv = fx.local(ValType::I32);
        self.expr(fx, items[1], false)?;
        fx.op(I::LocalSet(bv));
        fx.op(I::LocalGet(av));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_DEC));
        fx.op(I::I32Eq);
        fx.op(I::LocalGet(bv));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_DEC));
        fx.op(I::I32Eq);
        fx.op(I::I32Or);
        fx.op(I::If(BlockType::Result(ValType::I32)));
        fx.op(I::LocalGet(av));
        fx.op(I::Call(self.h.as_f64));
        fx.op(I::LocalGet(bv));
        fx.op(I::Call(self.h.as_f64));
        fx.op(if name == "min" { I::F64Min } else { I::F64Max });
        fx.op(I::Call(self.h.box_dec));
        fx.op(I::Else);
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
        fx.op(I::End);
        Ok(())
    }
}
