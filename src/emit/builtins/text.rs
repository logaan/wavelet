use super::*;

impl Emitter<'_> {
    pub(crate) fn builtin_string(
        &mut self,
        fx: &mut FnCtx,
        name: &str,
        items: &[NodeId],
    ) -> Result<(), String> {
        match name {
            "str-cat" => self.concatenate_strings(fx, items)?,
            "to-string" => {
                expect_arity(name, items, 1)?;
                self.expr(fx, items[0], false)?;
                fx.op(I::Call(self.h.to_str));
            }
            "to-char" => {
                expect_arity(name, items, 1)?;
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
            _ => unreachable!("builtin_string routed unexpected name `{name}`"),
        }
        Ok(())
    }

    fn concatenate_strings(&mut self, fx: &mut FnCtx, items: &[NodeId]) -> Result<(), String> {
        let empty = self.intern_str("");
        fx.op(I::I32Const(empty as i32));
        if let [item] = items {
            let value = fx.local(ValType::I32);
            self.expr(fx, *item, false)?;
            fx.op(I::LocalSet(value));
            let accumulator = fx.local(ValType::I32);
            fx.op(I::LocalSet(accumulator));
            fx.op(I::LocalGet(value));
            fx.op(I::I32Load(ma(0, 2)));
            fx.op(I::I32Const(TAG_LIST));
            fx.op(I::I32Eq);
            fx.op(I::LocalGet(value));
            fx.op(I::I32Load(ma(0, 2)));
            fx.op(I::I32Const(TAG_TUP));
            fx.op(I::I32Eq);
            fx.op(I::I32Or);
            fx.op(I::If(BlockType::Result(ValType::I32)));
            self.concatenate_sequence(fx, value, accumulator);
            fx.op(I::Else);
            fx.op(I::LocalGet(value));
            self.string_value(fx);
            fx.op(I::End);
        } else {
            for &item in items {
                self.expr(fx, item, false)?;
                self.string_value(fx);
                fx.op(I::Call(self.h.strcat2));
            }
        }
        Ok(())
    }

    fn concatenate_sequence(&mut self, fx: &mut FnCtx, sequence: u32, accumulator: u32) {
        let index = fx.local(ValType::I32);
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(index));
        fx.op(I::LocalGet(sequence));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(accumulator));
        fx.op(I::LocalGet(sequence));
        fx.op(I::LocalGet(index));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        self.string_value(fx);
        fx.op(I::Call(self.h.strcat2));
        fx.op(I::LocalSet(accumulator));
        fx.op(I::LocalGet(index));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(index));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(accumulator));
    }

    fn string_value(&mut self, fx: &mut FnCtx) {
        let value = fx.local(ValType::I32);
        fx.op(I::LocalTee(value));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_CHAR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(ValType::I32)));
        self.character_string(fx, value);
        fx.op(I::Else);
        fx.op(I::LocalGet(value));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(value));
        fx.op(I::End);
    }

    fn character_string(&mut self, fx: &mut FnCtx, value: u32) {
        let point = fx.local(ValType::I32);
        let width = fx.local(ValType::I32);
        let output = fx.local(ValType::I32);
        let index = fx.local(ValType::I32);
        fx.op(I::LocalGet(value));
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalSet(point));
        fx.op(I::I32Const(1));
        for boundary in [0x80, 0x800, 0x10000] {
            fx.op(I::LocalGet(point));
            fx.op(I::I32Const(boundary));
            fx.op(I::I32GeU);
            fx.op(I::I32Add);
        }
        fx.op(I::LocalSet(width));
        fx.op(I::I32Const(12));
        fx.op(I::Call(self.h.alloc));
        fx.op(I::LocalTee(output));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(output));
        fx.op(I::LocalGet(width));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(width));
        fx.op(I::I32Const(1));
        fx.op(I::I32Sub);
        fx.op(I::LocalSet(index));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(index));
        fx.op(I::I32Eqz);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(output));
        fx.op(I::LocalGet(index));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(point));
        fx.op(I::I32Const(63));
        fx.op(I::I32And);
        fx.op(I::I32Const(128));
        fx.op(I::I32Or);
        fx.op(I::I32Store8(ma(8, 0)));
        fx.op(I::LocalGet(point));
        fx.op(I::I32Const(6));
        fx.op(I::I32ShrU);
        fx.op(I::LocalSet(point));
        fx.op(I::LocalGet(index));
        fx.op(I::I32Const(1));
        fx.op(I::I32Sub);
        fx.op(I::LocalSet(index));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(output));
        fx.op(I::LocalGet(point));
        fx.op(I::LocalGet(width));
        fx.op(I::I32Const(1));
        fx.op(I::I32GtU);
        fx.op(I::If(BlockType::Result(ValType::I32)));
        fx.op(I::I32Const(0xff));
        fx.op(I::I32Const(8));
        fx.op(I::LocalGet(width));
        fx.op(I::I32Sub);
        fx.op(I::I32Shl);
        fx.op(I::Else);
        fx.op(I::I32Const(0));
        fx.op(I::End);
        fx.op(I::I32Or);
        fx.op(I::I32Store8(ma(8, 0)));
        fx.op(I::LocalGet(output));
    }
}
