use super::*;

mod allocation;
mod collections;
mod numeric;
mod persistence;
mod printing;

pub(crate) fn emit_substr(
    em: &mut Emitter,
    fx: &mut FnCtx,
    src: u32,
    start: u32,
    sublen: u32,
    out: u32,
    j: u32,
) {
    fx.op(I::LocalGet(sublen));
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(out));
    fx.op(I::LocalGet(out));
    fx.op(I::I32Const(TAG_STR));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(out));
    fx.op(I::LocalGet(sublen));
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(j));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(j));
    fx.op(I::LocalGet(sublen));
    fx.op(I::I32GeU);
    fx.op(I::BrIf(1));

    fx.op(I::LocalGet(out));
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(j));
    fx.op(I::I32Add);

    fx.op(I::LocalGet(src));
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(start));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(j));
    fx.op(I::I32Add);
    fx.op(I::I32Load8U(ma(0, 0)));
    fx.op(I::I32Store8(ma(0, 0)));
    fx.op(I::LocalGet(j));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(j));
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn to_str_seq(
    fx: &mut FnCtx,
    box_l: u32,
    n_l: u32,
    i_l: u32,
    acc_l: u32,
    base_l: u32,
    elem_l: u32,
    open_addr: u32,
    close_addr: u32,
    comma_addr: u32,
    stride: i32,
    strcat2: u32,
    to_str: u32,
    recurse: bool,
) {
    fx.op(I::I32Const(open_addr as i32));
    fx.op(I::LocalSet(acc_l));
    fx.op(I::LocalGet(box_l));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(n_l));
    fx.op(I::LocalGet(box_l));
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(base_l));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(i_l));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(i_l));
    fx.op(I::LocalGet(n_l));
    fx.op(I::I32GeS);
    fx.op(I::BrIf(1));
    fx.op(I::LocalGet(i_l));
    fx.op(I::I32Const(0));
    fx.op(I::I32GtS);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(acc_l));
    fx.op(I::I32Const(comma_addr as i32));
    fx.op(I::Call(strcat2));
    fx.op(I::LocalSet(acc_l));
    fx.op(I::End);
    fx.op(I::LocalGet(base_l));
    fx.op(I::LocalGet(i_l));
    fx.op(I::I32Const(stride));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(0, 2)));
    fx.op(I::LocalSet(elem_l));
    fx.op(I::LocalGet(acc_l));
    fx.op(I::LocalGet(elem_l));
    if recurse {
        fx.op(I::Call(to_str));
    }
    fx.op(I::Call(strcat2));
    fx.op(I::LocalSet(acc_l));
    fx.op(I::LocalGet(i_l));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(i_l));
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::LocalGet(acc_l));
    fx.op(I::I32Const(close_addr as i32));
    fx.op(I::Call(strcat2));
    fx.op(I::Return);
}

#[allow(clippy::too_many_arguments)]
fn to_str_dec(
    fx: &mut FnCtx,
    alloc: u32,
    box_str: u32,
    nan_s: u32,
    inf_s: u32,
    ninf_s: u32,
    xf: u32,
    e: u32,
    k: u32,
    lastd: u32,
    t: u32,
    buf: u32,
    oi: u32,
) {
    let put_c = |fx: &mut FnCtx, b: i32| {
        fx.op(I::LocalGet(buf));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Add);
        fx.op(I::I32Const(b));
        fx.op(I::I32Store8(ma(0, 0)));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(oi));
    };

    let put_digit = |fx: &mut FnCtx| {
        fx.op(I::LocalGet(buf));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(buf));
        fx.op(I::LocalGet(k));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(40, 0)));
        fx.op(I::I32Const(b'0' as i32));
        fx.op(I::I32Add);
        fx.op(I::I32Store8(ma(0, 0)));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(oi));
    };
    let inc = |fx: &mut FnCtx, l: u32, by: i32| {
        fx.op(I::LocalGet(l));
        fx.op(I::I32Const(by));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(l));
    };
    fx.op(I::LocalGet(0));
    fx.op(I::F64Load(ma(8, 3)));
    fx.op(I::LocalSet(xf));

    fx.op(I::LocalGet(xf));
    fx.op(I::LocalGet(xf));
    fx.op(I::F64Ne);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::I32Const(nan_s as i32));
    fx.op(I::Return);
    fx.op(I::End);
    fx.op(I::LocalGet(xf));
    fx.op(I::F64Const(f64::INFINITY.into()));
    fx.op(I::F64Eq);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::I32Const(inf_s as i32));
    fx.op(I::Return);
    fx.op(I::End);
    fx.op(I::LocalGet(xf));
    fx.op(I::F64Const(f64::NEG_INFINITY.into()));
    fx.op(I::F64Eq);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::I32Const(ninf_s as i32));
    fx.op(I::Return);
    fx.op(I::End);
    fx.op(I::I32Const(48));
    fx.op(I::Call(alloc));
    fx.op(I::LocalSet(buf));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(oi));

    fx.op(I::LocalGet(xf));
    fx.op(I::I64ReinterpretF64);
    fx.op(I::I64Const(0));
    fx.op(I::I64LtS);
    fx.op(I::If(BlockType::Empty));
    put_c(fx, b'-' as i32);
    fx.op(I::LocalGet(xf));
    fx.op(I::F64Neg);
    fx.op(I::LocalSet(xf));
    fx.op(I::End);
    fx.op(I::LocalGet(xf));
    fx.op(I::F64Const(0.0.into()));
    fx.op(I::F64Eq);
    fx.op(I::If(BlockType::Empty));
    put_c(fx, b'0' as i32);
    put_c(fx, b'.' as i32);
    put_c(fx, b'0' as i32);
    fx.op(I::LocalGet(buf));
    fx.op(I::LocalGet(oi));
    fx.op(I::Call(box_str));
    fx.op(I::Return);
    fx.op(I::End);

    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(e));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(xf));
    fx.op(I::F64Const(10.0.into()));
    fx.op(I::F64Lt);
    fx.op(I::BrIf(1));
    fx.op(I::LocalGet(xf));
    fx.op(I::F64Const(10.0.into()));
    fx.op(I::F64Div);
    fx.op(I::LocalSet(xf));
    inc(fx, e, 1);
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(xf));
    fx.op(I::F64Const(1.0.into()));
    fx.op(I::F64Ge);
    fx.op(I::BrIf(1));
    fx.op(I::LocalGet(xf));
    fx.op(I::F64Const(10.0.into()));
    fx.op(I::F64Mul);
    fx.op(I::LocalSet(xf));
    inc(fx, e, -1);
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);

    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(k));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(k));
    fx.op(I::I32Const(7));
    fx.op(I::I32GeS);
    fx.op(I::BrIf(1));
    fx.op(I::LocalGet(xf));
    fx.op(I::I32TruncF64U);
    fx.op(I::LocalSet(t));
    fx.op(I::LocalGet(buf));
    fx.op(I::LocalGet(k));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(t));
    fx.op(I::I32Store8(ma(40, 0)));
    fx.op(I::LocalGet(xf));
    fx.op(I::LocalGet(t));
    fx.op(I::F64ConvertI32U);
    fx.op(I::F64Sub);
    fx.op(I::F64Const(10.0.into()));
    fx.op(I::F64Mul);
    fx.op(I::LocalSet(xf));
    inc(fx, k, 1);
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);

    fx.op(I::LocalGet(buf));
    fx.op(I::I32Load8U(ma(46, 0)));
    fx.op(I::I32Const(5));
    fx.op(I::I32GeU);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::I32Const(5));
    fx.op(I::LocalSet(k));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(k));
    fx.op(I::I32Const(0));
    fx.op(I::I32LtS);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(buf));
    fx.op(I::I32Const(1));
    fx.op(I::I32Store8(ma(40, 0)));
    inc(fx, e, 1);
    fx.op(I::Br(2));
    fx.op(I::End);
    fx.op(I::LocalGet(buf));
    fx.op(I::LocalGet(k));
    fx.op(I::I32Add);
    fx.op(I::I32Load8U(ma(40, 0)));
    fx.op(I::LocalSet(t));
    fx.op(I::LocalGet(t));
    fx.op(I::I32Const(9));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(buf));
    fx.op(I::LocalGet(k));
    fx.op(I::I32Add);
    fx.op(I::I32Const(0));
    fx.op(I::I32Store8(ma(40, 0)));
    inc(fx, k, -1);
    fx.op(I::Br(1));
    fx.op(I::Else);
    fx.op(I::LocalGet(buf));
    fx.op(I::LocalGet(k));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(t));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::I32Store8(ma(40, 0)));
    fx.op(I::Br(2));
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::End);

    fx.op(I::I32Const(5));
    fx.op(I::LocalSet(lastd));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(lastd));
    fx.op(I::I32Const(0));
    fx.op(I::I32LeS);
    fx.op(I::BrIf(1));
    fx.op(I::LocalGet(buf));
    fx.op(I::LocalGet(lastd));
    fx.op(I::I32Add);
    fx.op(I::I32Load8U(ma(40, 0)));
    fx.op(I::BrIf(1));
    inc(fx, lastd, -1);
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);

    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(-4));
    fx.op(I::I32GeS);
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(5));
    fx.op(I::I32LeS);
    fx.op(I::I32And);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(0));
    fx.op(I::I32GeS);
    fx.op(I::If(BlockType::Empty));

    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(k));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(k));
    fx.op(I::LocalGet(e));
    fx.op(I::I32GtS);
    fx.op(I::BrIf(1));
    put_digit(fx);
    inc(fx, k, 1);
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
    put_c(fx, b'.' as i32);
    fx.op(I::LocalGet(lastd));
    fx.op(I::LocalGet(e));
    fx.op(I::I32GtS);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(k));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(k));
    fx.op(I::LocalGet(lastd));
    fx.op(I::I32GtS);
    fx.op(I::BrIf(1));
    put_digit(fx);
    inc(fx, k, 1);
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::Else);
    put_c(fx, b'0' as i32);
    fx.op(I::End);
    fx.op(I::Else);

    put_c(fx, b'0' as i32);
    put_c(fx, b'.' as i32);
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(k));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(k));
    fx.op(I::I32Const(0));
    fx.op(I::I32GeS);
    fx.op(I::BrIf(1));
    put_c(fx, b'0' as i32);
    inc(fx, k, 1);
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(k));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(k));
    fx.op(I::LocalGet(lastd));
    fx.op(I::I32GtS);
    fx.op(I::BrIf(1));
    put_digit(fx);
    inc(fx, k, 1);
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::Else);

    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(k));
    put_digit(fx);
    fx.op(I::LocalGet(lastd));
    fx.op(I::I32Const(0));
    fx.op(I::I32GtS);
    fx.op(I::If(BlockType::Empty));
    put_c(fx, b'.' as i32);
    fx.op(I::I32Const(1));
    fx.op(I::LocalSet(k));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(k));
    fx.op(I::LocalGet(lastd));
    fx.op(I::I32GtS);
    fx.op(I::BrIf(1));
    put_digit(fx);
    inc(fx, k, 1);
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::End);
    put_c(fx, b'e' as i32);
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(0));
    fx.op(I::I32LtS);
    fx.op(I::If(BlockType::Empty));
    put_c(fx, b'-' as i32);
    fx.op(I::I32Const(0));
    fx.op(I::LocalGet(e));
    fx.op(I::I32Sub);
    fx.op(I::LocalSet(e));
    fx.op(I::End);
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(100));
    fx.op(I::I32GeS);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(buf));
    fx.op(I::LocalGet(oi));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(100));
    fx.op(I::I32DivS);
    fx.op(I::I32Const(b'0' as i32));
    fx.op(I::I32Add);
    fx.op(I::I32Store8(ma(0, 0)));
    inc(fx, oi, 1);
    fx.op(I::End);
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(10));
    fx.op(I::I32GeS);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(buf));
    fx.op(I::LocalGet(oi));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(10));
    fx.op(I::I32DivS);
    fx.op(I::I32Const(10));
    fx.op(I::I32RemS);
    fx.op(I::I32Const(b'0' as i32));
    fx.op(I::I32Add);
    fx.op(I::I32Store8(ma(0, 0)));
    inc(fx, oi, 1);
    fx.op(I::End);
    fx.op(I::LocalGet(buf));
    fx.op(I::LocalGet(oi));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(10));
    fx.op(I::I32RemS);
    fx.op(I::I32Const(b'0' as i32));
    fx.op(I::I32Add);
    fx.op(I::I32Store8(ma(0, 0)));
    inc(fx, oi, 1);
    fx.op(I::End);
    fx.op(I::LocalGet(buf));
    fx.op(I::LocalGet(oi));
    fx.op(I::Call(box_str));
    fx.op(I::Return);
}

#[allow(clippy::too_many_arguments)]
fn to_str_char(fx: &mut FnCtx, alloc: u32, box_str: u32, cp: u32, t: u32, buf: u32, oi: u32) {
    let put_c = |fx: &mut FnCtx, b: i32| {
        fx.op(I::LocalGet(buf));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Add);
        fx.op(I::I32Const(b));
        fx.op(I::I32Store8(ma(0, 0)));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(oi));
    };

    let put_t = |fx: &mut FnCtx| {
        fx.op(I::LocalGet(buf));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(t));
        fx.op(I::I32Store8(ma(0, 0)));
        fx.op(I::LocalGet(oi));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(oi));
    };

    let put_t_hex = |fx: &mut FnCtx| {
        fx.op(I::LocalGet(t));
        fx.op(I::I32Const(10));
        fx.op(I::I32GeU);
        fx.op(I::If(BlockType::Result(ValType::I32)));
        fx.op(I::LocalGet(t));
        fx.op(I::I32Const(b'a' as i32 - 10));
        fx.op(I::I32Add);
        fx.op(I::Else);
        fx.op(I::LocalGet(t));
        fx.op(I::I32Const(b'0' as i32));
        fx.op(I::I32Add);
        fx.op(I::End);
        fx.op(I::LocalSet(t));
        put_t(fx);
    };
    let esc = |fx: &mut FnCtx, ch: u8| {
        put_c(fx, b'\\' as i32);
        put_c(fx, ch as i32);
    };
    fx.op(I::LocalGet(0));
    fx.op(I::I64Load(ma(8, 3)));
    fx.op(I::I32WrapI64);
    fx.op(I::LocalSet(cp));
    fx.op(I::I32Const(16));
    fx.op(I::Call(alloc));
    fx.op(I::LocalSet(buf));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(oi));
    put_c(fx, b'\'' as i32);

    let named: [(u32, u8); 5] = [
        (0x27, b'\''),
        (0x5c, b'\\'),
        (0x0a, b'n'),
        (0x0d, b'r'),
        (0x09, b't'),
    ];
    for &(code, ch) in &named {
        fx.op(I::LocalGet(cp));
        fx.op(I::I32Const(code as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        esc(fx, ch);
        put_c(fx, b'\'' as i32);
        fx.op(I::LocalGet(buf));
        fx.op(I::LocalGet(oi));
        fx.op(I::Call(box_str));
        fx.op(I::Return);
        fx.op(I::End);
    }
    fx.op(I::LocalGet(cp));
    fx.op(I::I32Const(0x20));
    fx.op(I::I32LtU);
    fx.op(I::LocalGet(cp));
    fx.op(I::I32Const(0x7f));
    fx.op(I::I32GeU);
    fx.op(I::LocalGet(cp));
    fx.op(I::I32Const(0xa0));
    fx.op(I::I32LeU);
    fx.op(I::I32And);
    fx.op(I::I32Or);
    fx.op(I::LocalGet(cp));
    fx.op(I::I32Const(0xad));
    fx.op(I::I32Eq);
    fx.op(I::I32Or);
    fx.op(I::If(BlockType::Empty));

    put_c(fx, b'\\' as i32);
    put_c(fx, b'u' as i32);
    put_c(fx, b'{' as i32);
    fx.op(I::LocalGet(cp));
    fx.op(I::I32Const(16));
    fx.op(I::I32GeU);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(cp));
    fx.op(I::I32Const(4));
    fx.op(I::I32ShrU);
    fx.op(I::LocalSet(t));
    put_t_hex(fx);
    fx.op(I::End);
    fx.op(I::LocalGet(cp));
    fx.op(I::I32Const(15));
    fx.op(I::I32And);
    fx.op(I::LocalSet(t));
    put_t_hex(fx);
    put_c(fx, b'}' as i32);
    fx.op(I::Else);

    fx.op(I::LocalGet(cp));
    fx.op(I::I32Const(0x80));
    fx.op(I::I32LtU);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(cp));
    fx.op(I::LocalSet(t));
    put_t(fx);
    fx.op(I::Else);
    fx.op(I::LocalGet(cp));
    fx.op(I::I32Const(0x800));
    fx.op(I::I32LtU);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(cp));
    fx.op(I::I32Const(6));
    fx.op(I::I32ShrU);
    fx.op(I::I32Const(0xC0));
    fx.op(I::I32Or);
    fx.op(I::LocalSet(t));
    put_t(fx);
    fx.op(I::Else);
    fx.op(I::LocalGet(cp));
    fx.op(I::I32Const(0x10000));
    fx.op(I::I32LtU);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(cp));
    fx.op(I::I32Const(12));
    fx.op(I::I32ShrU);
    fx.op(I::I32Const(0xE0));
    fx.op(I::I32Or);
    fx.op(I::LocalSet(t));
    put_t(fx);
    fx.op(I::Else);
    fx.op(I::LocalGet(cp));
    fx.op(I::I32Const(18));
    fx.op(I::I32ShrU);
    fx.op(I::I32Const(0xF0));
    fx.op(I::I32Or);
    fx.op(I::LocalSet(t));
    put_t(fx);
    fx.op(I::LocalGet(cp));
    fx.op(I::I32Const(12));
    fx.op(I::I32ShrU);
    fx.op(I::I32Const(0x3F));
    fx.op(I::I32And);
    fx.op(I::I32Const(0x80));
    fx.op(I::I32Or);
    fx.op(I::LocalSet(t));
    put_t(fx);
    fx.op(I::End);
    fx.op(I::LocalGet(cp));
    fx.op(I::I32Const(6));
    fx.op(I::I32ShrU);
    fx.op(I::I32Const(0x3F));
    fx.op(I::I32And);
    fx.op(I::I32Const(0x80));
    fx.op(I::I32Or);
    fx.op(I::LocalSet(t));
    put_t(fx);
    fx.op(I::End);
    fx.op(I::LocalGet(cp));
    fx.op(I::I32Const(0x3F));
    fx.op(I::I32And);
    fx.op(I::I32Const(0x80));
    fx.op(I::I32Or);
    fx.op(I::LocalSet(t));
    put_t(fx);
    fx.op(I::End);
    fx.op(I::End);
    put_c(fx, b'\'' as i32);
    fx.op(I::LocalGet(buf));
    fx.op(I::LocalGet(oi));
    fx.op(I::Call(box_str));
    fx.op(I::Return);
}

pub(crate) fn emit_helpers(em: &mut Emitter) -> Result<(), String> {
    allocation::emit_alloc(em);
    allocation::emit_realloc(em);
    allocation::emit_box_int(em);
    allocation::emit_box_bool(em);
    allocation::emit_box_dec(em);
    allocation::emit_box_str(em);
    allocation::emit_truthy(em);
    allocation::emit_unbox_int(em);
    allocation::emit_unbox_char(em);
    allocation::emit_unbox_dec(em);
    collections::emit_equality(em);
    collections::emit_length(em);
    collections::emit_head(em);
    collections::emit_tail(em);
    collections::emit_concat(em);
    printing::emit_string(em);
    collections::emit_record_get(em);
    numeric::emit_as_float(em);
    numeric::emit_arithmetic(em);
    numeric::emit_compare(em);
    numeric::emit_negate(em);
    numeric::emit_integer_arithmetic(em);
    numeric::emit_float_compare(em);
    persistence::emit_persist_alloc(em);
    persistence::emit_persist(em);
    Ok(())
}
