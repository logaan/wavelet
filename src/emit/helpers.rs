//! Shared runtime helper-function bodies (`emit_helpers`: alloc, eq_raw,
//! to_str, persist, arithmetic/comparison, …) and the inline substring and
//! sequence-join emitters they and the builtins share.

use super::*;

/// Copy `sublen` bytes from `src[8 + start ..]` into a fresh `[TAG_STR, sublen,
/// bytes…]` box left in `out`. `start`/`sublen` are locals; `j` is a scratch
/// loop local.
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
    // dst = out + 8 + j
    fx.op(I::LocalGet(out));
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(j));
    fx.op(I::I32Add);
    // byte = src[8 + start + j]
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

/// Emit an in-place `to_str` join loop for a sequence-shaped box (list, tuple,
/// or flags): `open` + elements joined by `comma` + `close`. Elements sit at
/// `box+8 + stride*i` for `i` in `0..load(box+4)`; when `recurse` each element
/// box is run through `to_str`, otherwise it is a `str` box appended verbatim
/// (the flags-name case). Emits its own `return`.
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

/// Emit the float arm of `to_str`: `value::format_dec` transcribed
/// op-for-op.
///
/// Six significant digits, fixed notation for decimal exponents `-4..=5`,
/// `d.de±x` scientific outside, `nan`/`inf`/`-inf` interned. Every f64
/// operation (and its order) matches the Rust reference exactly — IEEE-754
/// doubles are deterministic, so the two pipelines produce identical text.
/// Change `format_dec` and this together or the differential suite catches
/// the drift.
///
/// `buf` holds the output bytes; the seven extracted digits live at
/// `buf + 40..47` (output never exceeds 14 bytes). `xf` is an F64 local;
/// the rest are I32 scratch. Leaves nothing on the stack: every path
/// returns the built str box.
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
    // out[oi] = <const byte>; oi += 1
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
    // out[oi] = '0' + digits[k]; oi += 1
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
    // nan / inf / -inf: interned, exactly the interpreter's spellings
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
    // sign bit (not `< 0.0`) so `-0.0` prints "-0.0" like the interpreter
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
    // normalize into [1, 10): e tracks the decimal exponent
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
    // seven digits (six significant + one to round by) at buf+40..46;
    // subtracting the integer part is exact, so xf stays in [0, 10)
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
    // round to six digits; a carry off the top is a magnitude step
    // (9.99999x -> 1.00000, e + 1)
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
    // lastd = index of the last significant digit in 0..=5
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
    // fixed for e in -4..=5, scientific outside
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
    // ddd.d — integer digits 0..=e, then the trimmed fraction (or "0")
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
    // 0.000d — (-e - 1) zeros, then all significant digits
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
    // d.de±x — exponent is at most three digits (|e| <= 324)
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

/// Emit the char arm of `to_str`: single quotes plus Rust's `{c:?}` escapes.
///
/// Exact for `\'` `\\` `\n` `\r` `\t` `\0`, `\u{..}` for the remaining
/// non-printables through Latin-1 (C0/C1 controls, DEL, NBSP, soft hyphen —
/// `0x00..=0x1f`, `0x7f..=0xa0`, `0xad`), and raw UTF-8 for everything
/// else. Rust additionally `\u{..}`-escapes non-printable and
/// grapheme-extending codepoints above Latin-1 (`'\u{200b}'`, combining
/// marks); such chars diverge from the oracle — the same policy as the
/// string branch above, where only the common escapes agree and exotic
/// examples stay on SKIP.
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
    // out[oi] = t; oi += 1
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
    // out[oi] = lowercase hex digit for t in 0..=15; oi += 1
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
    // the named escapes, then \u{..} for remaining C0/C1/DEL, then raw UTF-8
    let named: [(u32, u8); 6] = [
        (0x27, b'\''),
        (0x5c, b'\\'),
        (0x0a, b'n'),
        (0x0d, b'r'),
        (0x09, b't'),
        (0x00, b'0'),
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
    // \u{..}: cp <= 0xad here, so at most two hex digits, no leading zeros
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
    // raw UTF-8, 1-4 bytes by codepoint range
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
    use ValType::{F64, I32, I64};

    // alloc(n) -> ptr   [locals: r=1, end=2]
    {
        let mut fx = FnCtx::new(1);
        let r = fx.local(I32);
        let end = fx.local(I32);
        fx.op(I::GlobalGet(0));
        fx.op(I::LocalSet(r));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Const(7));
        fx.op(I::I32Add);
        fx.op(I::I32Const(-8));
        fx.op(I::I32And);
        fx.op(I::LocalSet(0));
        fx.op(I::LocalGet(r));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(end));
        fx.op(I::LocalGet(end));
        fx.op(I::MemorySize(0));
        fx.op(I::I32Const(16));
        fx.op(I::I32Shl);
        fx.op(I::I32GtU);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(end));
        fx.op(I::MemorySize(0));
        fx.op(I::I32Const(16));
        fx.op(I::I32Shl);
        fx.op(I::I32Sub);
        fx.op(I::I32Const(0xffff));
        fx.op(I::I32Add);
        fx.op(I::I32Const(16));
        fx.op(I::I32ShrU);
        fx.op(I::MemoryGrow(0));
        fx.op(I::I32Const(-1));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(end));
        fx.op(I::GlobalSet(0));
        fx.op(I::LocalGet(r));
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // cabi_realloc(old, old_size, align, new_size) -> ptr
    {
        let mut fx = FnCtx::new(4);
        let p = fx.local(I32);
        fx.op(I::LocalGet(3));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(p));
        fx.op(I::LocalGet(1));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(1));
        fx.op(I::MemoryCopy {
            src_mem: 0,
            dst_mem: 0,
        });
        fx.op(I::End);
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![I32, I32, I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // box_int(i64) -> ptr
    {
        let mut fx = FnCtx::new(1);
        let p = fx.local(I32);
        fx.op(I::I32Const(16));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalTee(p));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(0));
        fx.op(I::I64Store(ma(8, 3)));
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![I64], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // box_bool(i32) -> ptr (static boxes)
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::I32Const(em.true_addr as i32));
        fx.op(I::Else);
        fx.op(I::I32Const(em.false_addr as i32));
        fx.op(I::End);
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // box_dec(f64) -> ptr
    {
        let mut fx = FnCtx::new(1);
        let p = fx.local(I32);
        fx.op(I::I32Const(16));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalTee(p));
        fx.op(I::I32Const(TAG_DEC));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(0));
        fx.op(I::F64Store(ma(8, 3)));
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![F64], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // box_str(ptr, len) -> box
    {
        let mut fx = FnCtx::new(2);
        let p = fx.local(I32);
        fx.op(I::I32Const(8));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalTee(p));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(1));
        fx.op(I::MemoryCopy {
            src_mem: 0,
            dst_mem: 0,
        });
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // truthy(box) -> i32
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::I32Const(1));
        fx.op(I::Else);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::I32Ne);
        fx.op(I::End);
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // unbox_int(box) -> i64 (traps unless tag int)
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        let t = em.ty_idx(vec![I32], vec![I64]);
        em.bodies.push((t, fx.finish()));
    }

    // unbox_char(box) -> i64 codepoint (traps unless tag char)
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_CHAR));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        let t = em.ty_idx(vec![I32], vec![I64]);
        em.bodies.push((t, fx.finish()));
    }

    // unbox_dec(box) -> f64
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_DEC));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::F64Load(ma(8, 3)));
        let t = em.ty_idx(vec![I32], vec![F64]);
        em.bodies.push((t, fx.finish()));
    }

    // eq_raw(a, b) -> i32   [locals: ta=2, la=3, i=4]
    //
    // Structural equality mirroring the interpreter's `impl PartialEq for Value`
    // (src/value.rs). Primitives (bool/int/char/dec/str) compare by content;
    // compound boxes (rec/list/tup/var/flg) recurse into their element boxes via
    // this very fn (`em.h.eq_raw`, already reserved). Only closures (TAG_FN) keep
    // pointer identity, matching `Rc::ptr_eq` for `Closure`/`Macro`.
    {
        let mut fx = FnCtx::new(2);
        let ta = fx.local(I32);
        let la = fx.local(I32);
        let i = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalTee(ta));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        // bool
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Eq);
        fx.op(I::Return);
        fx.op(I::End);
        // int
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::LocalGet(1));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::I64Eq);
        fx.op(I::Return);
        fx.op(I::End);
        // dec
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_DEC));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::F64Load(ma(8, 3)));
        fx.op(I::LocalGet(1));
        fx.op(I::F64Load(ma(8, 3)));
        fx.op(I::F64Eq);
        fx.op(I::Return);
        fx.op(I::End);
        // str
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalTee(la));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(la));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(8, 0)));
        fx.op(I::LocalGet(1));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(8, 0)));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::I32Const(1));
        fx.op(I::Return);
        fx.op(I::End);
        // char: i64 scalar @8 (TAG_INT layout)
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_CHAR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::LocalGet(1));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::I64Eq);
        fx.op(I::Return);
        fx.op(I::End);
        // record: n @4, then (key strbox @8+8i, value box @12+8i) pairs.
        // Order-sensitive (Value::Rec is a Vec compare): both n must match, then
        // each key AND value compared positionally by recursing eq_raw.
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_REC));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalTee(la));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(la));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // key: load a[8+8i], b[8+8i] and recurse
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalGet(1));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        // value: load a[12+8i], b[12+8i] and recurse
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(12, 2)));
        fx.op(I::LocalGet(1));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(12, 2)));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::I32Const(1));
        fx.op(I::Return);
        fx.op(I::End);
        // list / tuple / flags: len @4, element boxes @8+4i. Order-sensitive
        // (Value::Lst/Tup/Flg are Vec compares). All three share this layout: a
        // flags box stores its name str boxes @8+4i, so structural recursion over
        // them matches the interpreter's `Flg(Vec<String>)` equality too.
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Eq);
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_TUP));
        fx.op(I::I32Eq);
        fx.op(I::I32Or);
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_FLG));
        fx.op(I::I32Eq);
        fx.op(I::I32Or);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalTee(la));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(la));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // element: load a[8+4i], b[8+4i] and recurse
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalGet(1));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::I32Const(1));
        fx.op(I::Return);
        fx.op(I::End);
        // variant: case-name strbox @4, payload box @8 (0 if none). Equal iff
        // case names match (recurse) and payloads match: both absent (0) is equal,
        // exactly one absent is unequal, else recurse on the two payload boxes.
        // Mirrors `Variant(a,p) == Variant(b,q) => a == b && p == q`.
        fx.op(I::LocalGet(ta));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        // case names
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        // payload presence: la = a.payload, i = b.payload
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalSet(la));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalSet(i));
        // both absent -> equal
        fx.op(I::LocalGet(la));
        fx.op(I::I32Eqz);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Eqz);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(1));
        fx.op(I::Return);
        fx.op(I::End);
        // exactly one absent -> unequal (la == 0 XOR i == 0)
        fx.op(I::LocalGet(la));
        fx.op(I::I32Eqz);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Eqz);
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        // both present -> recurse on payloads
        fx.op(I::LocalGet(la));
        fx.op(I::LocalGet(i));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::Return);
        fx.op(I::End);
        // closures (TAG_FN) and anything else unhandled: pointer identity,
        // matching the interpreter's `Rc::ptr_eq` for Closure/Macro.
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Eq);
        let t = em.ty_idx(vec![I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // len_raw(box) -> i32 (str or list)
    {
        let mut fx = FnCtx::new(1);
        let tg = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalTee(tg));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Eq);
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Eq);
        fx.op(I::I32Or);
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // head_h(list box) -> box
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(8, 2)));
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // tail_h(list box) -> list box   [locals: src=0, n, m, dst, i]
    {
        let mut fx = FnCtx::new(1);
        let n = fx.local(I32);
        let m = fx.local(I32);
        let dst = fx.local(I32);
        let i = fx.local(I32);
        // require a non-empty list
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalTee(n));
        fx.op(I::I32Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        // m = n - 1
        fx.op(I::LocalGet(n));
        fx.op(I::I32Const(1));
        fx.op(I::I32Sub);
        fx.op(I::LocalSet(m));
        // dst = alloc(8 + 4*m)
        fx.op(I::I32Const(8));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Const(2));
        fx.op(I::I32Shl);
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(dst));
        fx.op(I::LocalGet(dst));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(dst));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Store(ma(4, 2)));
        // for i in 0..m: dst[8+4i] = src[8+4(i+1)]
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(m));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // dst + 8 + 4*i
        fx.op(I::LocalGet(dst));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(2));
        fx.op(I::I32Shl);
        fx.op(I::I32Add);
        // value: src[8 + 4*(i+1)] = src + 12 + 4*i
        fx.op(I::LocalGet(0));
        fx.op(I::I32Const(12));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(2));
        fx.op(I::I32Shl);
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
        fx.op(I::LocalGet(dst));
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // strcat2(a, b) -> box   [locals la, lb, p]
    {
        let mut fx = FnCtx::new(2);
        let la = fx.local(I32);
        let lb = fx.local(I32);
        let p = fx.local(I32);
        for arg in [0u32, 1u32] {
            fx.op(I::LocalGet(arg));
            fx.op(I::I32Load(ma(0, 2)));
            fx.op(I::I32Const(TAG_STR));
            fx.op(I::I32Ne);
            fx.op(I::If(BlockType::Empty));
            fx.op(I::Unreachable);
            fx.op(I::End);
        }
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(la));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(lb));
        fx.op(I::I32Const(8));
        fx.op(I::LocalGet(la));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(lb));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalTee(p));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(la));
        fx.op(I::LocalGet(lb));
        fx.op(I::I32Add);
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(la));
        fx.op(I::MemoryCopy {
            src_mem: 0,
            dst_mem: 0,
        });
        fx.op(I::LocalGet(p));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(la));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(1));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(lb));
        fx.op(I::MemoryCopy {
            src_mem: 0,
            dst_mem: 0,
        });
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // case_h(s, up) -> box   [locals l, p, i, c]
    {
        let mut fx = FnCtx::new(2);
        let l = fx.local(I32);
        let p = fx.local(I32);
        let i = fx.local(I32);
        let c = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(l));
        fx.op(I::I32Const(8));
        fx.op(I::LocalGet(l));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalTee(p));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(l));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(l));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(8, 0)));
        fx.op(I::LocalSet(c));
        fx.op(I::LocalGet(1));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(b'a' as i32));
        fx.op(I::I32GeU);
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(b'z' as i32));
        fx.op(I::I32LeU);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(32));
        fx.op(I::I32Sub);
        fx.op(I::LocalSet(c));
        fx.op(I::End);
        fx.op(I::Else);
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(b'A' as i32));
        fx.op(I::I32GeU);
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(b'Z' as i32));
        fx.op(I::I32LeU);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(c));
        fx.op(I::I32Const(32));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(c));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(p));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(c));
        fx.op(I::I32Store8(ma(8, 0)));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(p));
        let t = em.ty_idx(vec![I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // to_str(box) -> str box   [locals tag, n(i64), neg, buf, i]
    {
        let true_s = em.intern_str("true");
        let false_s = em.intern_str("false");
        // interned punctuation for the compound-value printers
        let str_lb = em.intern_str("[");
        let str_rb = em.intern_str("]");
        let str_lp = em.intern_str("(");
        let str_rp = em.intern_str(")");
        let str_lc = em.intern_str("{");
        let str_rc = em.intern_str("}");
        let str_comma = em.intern_str(", ");
        let str_colon = em.intern_str(": ");
        let str_cell = em.intern_str("cell(");
        let nan_s = em.intern_str("nan");
        let inf_s = em.intern_str("inf");
        let ninf_s = em.intern_str("-inf");
        let mut fx = FnCtx::new(1);
        let tag = fx.local(I32);
        let n = fx.local(I64);
        let neg = fx.local(I32);
        let buf = fx.local(I32);
        let i = fx.local(I32);
        // extra locals for the string-quoting branch
        let s_src = fx.local(I32);
        let s_len = fx.local(I32);
        let s_out = fx.local(I32);
        let s_oi = fx.local(I32);
        let s_ci = fx.local(I32);
        let s_byte = fx.local(I32);
        // extra locals for the compound-value branches
        let c_n = fx.local(I32);
        let c_i = fx.local(I32);
        let c_acc = fx.local(I32);
        let c_base = fx.local(I32);
        let c_elem = fx.local(I32);
        let c_key = fx.local(I32);
        let c_val = fx.local(I32);
        // extra locals for the float and char branches (which reuse `buf`
        // as their output buffer and `i` as its write index)
        let d_xf = fx.local(F64);
        let d_e = fx.local(I32);
        let d_k = fx.local(I32);
        let d_last = fx.local(I32);
        let d_t = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(tag));
        // out[s_oi] = <const byte>; s_oi += 1
        let put_c = |fx: &mut FnCtx, out: u32, oi: u32, b: i32| {
            fx.op(I::LocalGet(out));
            fx.op(I::LocalGet(oi));
            fx.op(I::I32Add);
            fx.op(I::I32Const(b));
            fx.op(I::I32Store8(ma(0, 0)));
            fx.op(I::LocalGet(oi));
            fx.op(I::I32Const(1));
            fx.op(I::I32Add);
            fx.op(I::LocalSet(oi));
        };
        // string: quote + escape to match print_value's `{s:?}`. Escapes
        // `"` `\` `\n` `\t` `\r`; other bytes (incl. UTF-8 continuation) pass
        // through. Rust escapes other control/non-printable codepoints too, so
        // strings containing them still diverge from the oracle (kept SKIP);
        // the common printable cases now agree.
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        // s_len = box@4 ; s_src = box+8
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(s_len));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(s_src));
        // s_out = alloc(s_len*2 + 2)  (worst case: every byte -> 2 bytes, + 2 quotes)
        fx.op(I::LocalGet(s_len));
        fx.op(I::I32Const(2));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(2));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(s_out));
        // out[0] = '"' ; s_oi = 1 ; s_ci = 0
        fx.op(I::LocalGet(s_out));
        fx.op(I::I32Const(b'"' as i32));
        fx.op(I::I32Store8(ma(0, 0)));
        fx.op(I::I32Const(1));
        fx.op(I::LocalSet(s_oi));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(s_ci));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        // if s_ci >= s_len break
        fx.op(I::LocalGet(s_ci));
        fx.op(I::LocalGet(s_len));
        fx.op(I::I32GeS);
        fx.op(I::BrIf(1));
        // s_byte = load8(s_src + s_ci)
        fx.op(I::LocalGet(s_src));
        fx.op(I::LocalGet(s_ci));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(0, 0)));
        fx.op(I::LocalSet(s_byte));
        // escape ladder
        fx.op(I::LocalGet(s_byte));
        fx.op(I::I32Const(b'"' as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        put_c(&mut fx, s_out, s_oi, b'\\' as i32);
        put_c(&mut fx, s_out, s_oi, b'"' as i32);
        fx.op(I::Else);
        fx.op(I::LocalGet(s_byte));
        fx.op(I::I32Const(b'\\' as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        put_c(&mut fx, s_out, s_oi, b'\\' as i32);
        put_c(&mut fx, s_out, s_oi, b'\\' as i32);
        fx.op(I::Else);
        fx.op(I::LocalGet(s_byte));
        fx.op(I::I32Const(b'\n' as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        put_c(&mut fx, s_out, s_oi, b'\\' as i32);
        put_c(&mut fx, s_out, s_oi, b'n' as i32);
        fx.op(I::Else);
        fx.op(I::LocalGet(s_byte));
        fx.op(I::I32Const(b'\t' as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        put_c(&mut fx, s_out, s_oi, b'\\' as i32);
        put_c(&mut fx, s_out, s_oi, b't' as i32);
        fx.op(I::Else);
        fx.op(I::LocalGet(s_byte));
        fx.op(I::I32Const(b'\r' as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        put_c(&mut fx, s_out, s_oi, b'\\' as i32);
        put_c(&mut fx, s_out, s_oi, b'r' as i32);
        fx.op(I::Else);
        // default: copy the byte verbatim
        fx.op(I::LocalGet(s_out));
        fx.op(I::LocalGet(s_oi));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(s_byte));
        fx.op(I::I32Store8(ma(0, 0)));
        fx.op(I::LocalGet(s_oi));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(s_oi));
        fx.op(I::End); // \r
        fx.op(I::End); // \t
        fx.op(I::End); // \n
        fx.op(I::End); // backslash
        fx.op(I::End); // quote
        // s_ci += 1 ; continue
        fx.op(I::LocalGet(s_ci));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(s_ci));
        fx.op(I::Br(0));
        fx.op(I::End); // loop
        fx.op(I::End); // block
        // closing quote, then box
        put_c(&mut fx, s_out, s_oi, b'"' as i32);
        fx.op(I::LocalGet(s_out));
        fx.op(I::LocalGet(s_oi));
        fx.op(I::Call(em.h.box_str));
        fx.op(I::Return);
        fx.op(I::End);
        // bool: static "true"/"false"
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_BOOL));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(true_s as i32));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::I32Const(false_s as i32));
        fx.op(I::Return);
        fx.op(I::End);
        // ---- compound values: recurse via to_str, matching print_value ----
        let seq_strcat2 = em.h.strcat2;
        let seq_to_str = em.h.to_str;
        // list: [e0, e1, ...]
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        to_str_seq(
            &mut fx, 0, c_n, c_i, c_acc, c_base, c_elem, str_lb, str_rb, str_comma,
            4, seq_strcat2, seq_to_str, true,
        );
        fx.op(I::End);
        // tuple: (e0, e1, ...)
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_TUP));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        to_str_seq(
            &mut fx, 0, c_n, c_i, c_acc, c_base, c_elem, str_lp, str_rp, str_comma,
            4, seq_strcat2, seq_to_str, true,
        );
        fx.op(I::End);
        // flags: {a, b, ...} (names are str boxes, appended verbatim)
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_FLG));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        to_str_seq(
            &mut fx, 0, c_n, c_i, c_acc, c_base, c_elem, str_lc, str_rc, str_comma,
            4, seq_strcat2, seq_to_str, false,
        );
        fx.op(I::End);
        // record: {k0: v0, k1: v1, ...}
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_REC));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(str_lc as i32));
        fx.op(I::LocalSet(c_acc));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(c_n));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(c_base));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(c_i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(c_i));
        fx.op(I::LocalGet(c_n));
        fx.op(I::I32GeS);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(c_i));
        fx.op(I::I32Const(0));
        fx.op(I::I32GtS);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(c_acc));
        fx.op(I::I32Const(str_comma as i32));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::LocalSet(c_acc));
        fx.op(I::End);
        // key = load(base + 8*i)  (a str box, appended verbatim)
        fx.op(I::LocalGet(c_base));
        fx.op(I::LocalGet(c_i));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(c_key));
        // val = load(base + 8*i + 4)
        fx.op(I::LocalGet(c_base));
        fx.op(I::LocalGet(c_i));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Const(4));
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(c_val));
        fx.op(I::LocalGet(c_acc));
        fx.op(I::LocalGet(c_key));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::LocalSet(c_acc));
        fx.op(I::LocalGet(c_acc));
        fx.op(I::I32Const(str_colon as i32));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::LocalSet(c_acc));
        fx.op(I::LocalGet(c_acc));
        fx.op(I::LocalGet(c_val));
        fx.op(I::Call(seq_to_str));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::LocalSet(c_acc));
        fx.op(I::LocalGet(c_i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(c_i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(c_acc));
        fx.op(I::I32Const(str_rc as i32));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::Return);
        fx.op(I::End);
        // variant: name  or  name(payload)
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(c_key)); // case-name str box
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalSet(c_val)); // payload box (0 if none)
        fx.op(I::LocalGet(c_val));
        fx.op(I::I32Const(0));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(c_key));
        fx.op(I::I32Const(str_lp as i32));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::LocalSet(c_acc));
        fx.op(I::LocalGet(c_acc));
        fx.op(I::LocalGet(c_val));
        fx.op(I::Call(seq_to_str));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::LocalSet(c_acc));
        fx.op(I::LocalGet(c_acc));
        fx.op(I::I32Const(str_rp as i32));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::Return);
        fx.op(I::End);
        // no payload: the bare case name
        fx.op(I::LocalGet(c_key));
        fx.op(I::Return);
        fx.op(I::End);
        // cell: cell(inner)
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_CELL));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(c_val));
        fx.op(I::I32Const(str_cell as i32));
        fx.op(I::LocalGet(c_val));
        fx.op(I::Call(seq_to_str));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::LocalSet(c_acc));
        fx.op(I::LocalGet(c_acc));
        fx.op(I::I32Const(str_rp as i32));
        fx.op(I::Call(seq_strcat2));
        fx.op(I::Return);
        fx.op(I::End);
        // float: format_dec's indicative six-significant-digit text
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_DEC));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        to_str_dec(
            &mut fx,
            em.h.alloc,
            em.h.box_str,
            nan_s,
            inf_s,
            ninf_s,
            d_xf,
            d_e,
            d_k,
            d_last,
            d_t,
            buf,
            i,
        );
        fx.op(I::End);
        // char: single-quoted with the common `{c:?}` escapes
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_CHAR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        to_str_char(&mut fx, em.h.alloc, em.h.box_str, d_k, d_t, buf, i);
        fx.op(I::End);
        // anything but int from here traps
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::LocalSet(n));
        fx.op(I::I32Const(32));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(buf));
        fx.op(I::I32Const(32));
        fx.op(I::LocalSet(i));
        fx.op(I::LocalGet(n));
        fx.op(I::I64Const(0));
        fx.op(I::I64LtS);
        fx.op(I::LocalSet(neg));
        fx.op(I::LocalGet(neg));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I64Const(0));
        fx.op(I::LocalGet(n));
        fx.op(I::I64Sub);
        fx.op(I::LocalSet(n));
        fx.op(I::End);
        // digits, least significant first (unsigned ops so |i64::MIN| works)
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Sub);
        fx.op(I::LocalSet(i));
        fx.op(I::LocalGet(buf));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::LocalGet(n));
        fx.op(I::I64Const(10));
        fx.op(I::I64RemU);
        fx.op(I::I32WrapI64);
        fx.op(I::I32Const(b'0' as i32));
        fx.op(I::I32Add);
        fx.op(I::I32Store8(ma(0, 0)));
        fx.op(I::LocalGet(n));
        fx.op(I::I64Const(10));
        fx.op(I::I64DivU);
        fx.op(I::LocalSet(n));
        fx.op(I::LocalGet(n));
        fx.op(I::I64Const(0));
        fx.op(I::I64Ne);
        fx.op(I::BrIf(0));
        fx.op(I::End);
        fx.op(I::LocalGet(neg));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Sub);
        fx.op(I::LocalSet(i));
        fx.op(I::LocalGet(buf));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Const(b'-' as i32));
        fx.op(I::I32Store8(ma(0, 0)));
        fx.op(I::End);
        fx.op(I::LocalGet(buf));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Const(32));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Sub);
        fx.op(I::Call(em.h.box_str));
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // rec_get(rec, key) -> box   returns the value box for `key`, or 0 if the
    // record has no such field.   [locals n=2, i=3, base=4]
    {
        let mut fx = FnCtx::new(2);
        let n = fx.local(I32);
        let i = fx.local(I32);
        let base = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_REC));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(n));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(n));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // base = rec + 8*i ; field key @ ma(8), value @ ma(12)
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalSet(base));
        fx.op(I::LocalGet(base));
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalGet(1));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(base));
        fx.op(I::I32Load(ma(12, 2)));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::I32Const(0));
        let t = em.ty_idx(vec![I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // as_f64(box) -> f64   coerces an int or dec box to f64; traps otherwise.
    // Mirrors the interpreter's `want_num` widening of ints in mixed arithmetic.
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(F64)));
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::F64ConvertI64S);
        fx.op(I::Else);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_DEC));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::F64Load(ma(8, 3)));
        fx.op(I::End);
        let t = em.ty_idx(vec![I32], vec![F64]);
        em.bodies.push((t, fx.finish()));
    }

    // arith_raw(a, b, op) -> box   op: 0=add 1=sub 2=mul 3=div 4=rem.
    // Matches the interpreter `arith`: both ints → checked i64 (trap on
    // overflow / div-0 / INT_MIN÷-1); otherwise both widened to f64.
    // [locals: xf=3, yf=4 (f64)]
    {
        let mut fx = FnCtx::new(3);
        let xf = fx.local(F64);
        let yf = fx.local(F64);
        // both int?
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Eq);
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Eq);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Result(I32)));
        // ---- int path: the shared checked-arithmetic core (arith_int), so
        // the boxed and typed (goal 5) paths cannot drift apart
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::LocalGet(1));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::LocalGet(2));
        fx.op(I::Call(em.h.arith_int));
        fx.op(I::Call(em.h.box_int));
        fx.op(I::Else);
        // ---- float path
        fx.op(I::LocalGet(0));
        fx.op(I::Call(em.h.as_f64));
        fx.op(I::LocalSet(xf));
        fx.op(I::LocalGet(1));
        fx.op(I::Call(em.h.as_f64));
        fx.op(I::LocalSet(yf));
        fx.op(I::LocalGet(2));
        fx.op(I::I32Const(0));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(F64)));
        fx.op(I::LocalGet(xf));
        fx.op(I::LocalGet(yf));
        fx.op(I::F64Add);
        fx.op(I::Else);
        fx.op(I::LocalGet(2));
        fx.op(I::I32Const(1));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(F64)));
        fx.op(I::LocalGet(xf));
        fx.op(I::LocalGet(yf));
        fx.op(I::F64Sub);
        fx.op(I::Else);
        fx.op(I::LocalGet(2));
        fx.op(I::I32Const(2));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(F64)));
        fx.op(I::LocalGet(xf));
        fx.op(I::LocalGet(yf));
        fx.op(I::F64Mul);
        fx.op(I::Else);
        fx.op(I::LocalGet(2));
        fx.op(I::I32Const(3));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(F64)));
        fx.op(I::LocalGet(xf));
        fx.op(I::LocalGet(yf));
        fx.op(I::F64Div);
        fx.op(I::Else);
        // rem: xf - trunc(xf/yf)*yf  (matches Rust f64 `%`)
        fx.op(I::LocalGet(xf));
        fx.op(I::LocalGet(xf));
        fx.op(I::LocalGet(yf));
        fx.op(I::F64Div);
        fx.op(I::F64Trunc);
        fx.op(I::LocalGet(yf));
        fx.op(I::F64Mul);
        fx.op(I::F64Sub);
        fx.op(I::End); // op == 3
        fx.op(I::End); // op == 2
        fx.op(I::End); // op == 1
        fx.op(I::End); // op == 0
        fx.op(I::Call(em.h.box_dec));
        fx.op(I::End); // int vs float
        let t = em.ty_idx(vec![I32, I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // cmp_raw(a, b) -> i32 in {-1, 0, 1}   total order over strings (byte
    // lexicographic), chars (by codepoint) and numbers (widened to f64); traps
    // on NaN/non-comparable, matching the interpreter's `compare`.
    // [locals: la=2, lb=3, n=4, i=5, ca=6, cb=7 (i32)]
    {
        let mut fx = FnCtx::new(2);
        let la = fx.local(I32);
        let lb = fx.local(I32);
        let n = fx.local(I32);
        let i = fx.local(I32);
        let ca = fx.local(I32);
        let cb = fx.local(I32);
        // both char? order by codepoint (interpreter: `Char(x).cmp(Char(y))`)
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_CHAR));
        fx.op(I::I32Eq);
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_CHAR));
        fx.op(I::I32Eq);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::LocalGet(1));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::I64LtU);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(-1));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::LocalGet(1));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::I64GtU);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(1));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::I32Const(0));
        fx.op(I::Return);
        fx.op(I::End);
        // both str?
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Eq);
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Eq);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Result(I32)));
        // ---- string lexicographic compare
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(la));
        fx.op(I::LocalGet(1));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(lb));
        // n = min(la, lb)
        fx.op(I::LocalGet(la));
        fx.op(I::LocalGet(lb));
        fx.op(I::I32LtU);
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::LocalGet(la));
        fx.op(I::Else);
        fx.op(I::LocalGet(lb));
        fx.op(I::End);
        fx.op(I::LocalSet(n));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(n));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(8, 0)));
        fx.op(I::LocalSet(ca));
        fx.op(I::LocalGet(1));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Add);
        fx.op(I::I32Load8U(ma(8, 0)));
        fx.op(I::LocalSet(cb));
        fx.op(I::LocalGet(ca));
        fx.op(I::LocalGet(cb));
        fx.op(I::I32LtU);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(-1));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::LocalGet(ca));
        fx.op(I::LocalGet(cb));
        fx.op(I::I32GtU);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(1));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End); // loop
        fx.op(I::End); // block
        // equal prefix: shorter string is less
        fx.op(I::LocalGet(la));
        fx.op(I::LocalGet(lb));
        fx.op(I::I32LtU);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(-1));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::LocalGet(la));
        fx.op(I::LocalGet(lb));
        fx.op(I::I32GtU);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(1));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::I32Const(0));
        fx.op(I::Else);
        // ---- numeric compare: the shared cmp_f64 core (widened to f64;
        // traps on NaN), so the boxed and typed (goal 5) paths cannot drift
        fx.op(I::LocalGet(0));
        fx.op(I::Call(em.h.as_f64));
        fx.op(I::LocalGet(1));
        fx.op(I::Call(em.h.as_f64));
        fx.op(I::Call(em.h.cmp_f64));
        fx.op(I::End); // str vs numeric
        let t = em.ty_idx(vec![I32, I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // neg_raw(box) -> box   negates an int (wrapping, as the interpreter's `-n`)
    // or a dec; traps on anything else.
    {
        let mut fx = FnCtx::new(1);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_INT));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::I64Const(0));
        fx.op(I::LocalGet(0));
        fx.op(I::I64Load(ma(8, 3)));
        fx.op(I::I64Sub);
        fx.op(I::Call(em.h.box_int));
        fx.op(I::Else);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Const(TAG_DEC));
        fx.op(I::I32Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(0));
        fx.op(I::F64Load(ma(8, 3)));
        fx.op(I::F64Neg);
        fx.op(I::Call(em.h.box_dec));
        fx.op(I::End);
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // arith_int(a: i64, b: i64, op: i32) -> i64 — the checked integer
    // arithmetic core (op: 0=add 1=sub 2=mul 3=div 4=rem): trap on overflow /
    // div-0 / INT_MIN÷-1, exactly the interpreter's checked_* semantics. The
    // boxed arith_raw and the goal-5 typed scalar path both call this, so the
    // two representations share one copy of the semantics.
    // [locals: ia=3, ib=4, r=5 (i64)]
    {
        let mut fx = FnCtx::new(3);
        let ia = fx.local(I64);
        let ib = fx.local(I64);
        let r = fx.local(I64);
        fx.op(I::LocalGet(0));
        fx.op(I::LocalSet(ia));
        fx.op(I::LocalGet(1));
        fx.op(I::LocalSet(ib));
        // op == 0 : add
        fx.op(I::LocalGet(2));
        fx.op(I::I32Const(0));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(I64)));
        fx.op(I::LocalGet(ia));
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Add);
        fx.op(I::LocalSet(r));
        // overflow: ((r^ia) & (r^ib)) <s 0
        fx.op(I::LocalGet(r));
        fx.op(I::LocalGet(ia));
        fx.op(I::I64Xor);
        fx.op(I::LocalGet(r));
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Xor);
        fx.op(I::I64And);
        fx.op(I::I64Const(0));
        fx.op(I::I64LtS);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(r));
        fx.op(I::Else);
        // op == 1 : sub
        fx.op(I::LocalGet(2));
        fx.op(I::I32Const(1));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(I64)));
        fx.op(I::LocalGet(ia));
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Sub);
        fx.op(I::LocalSet(r));
        // overflow: ((ia^ib) & (ia^r)) <s 0
        fx.op(I::LocalGet(ia));
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Xor);
        fx.op(I::LocalGet(ia));
        fx.op(I::LocalGet(r));
        fx.op(I::I64Xor);
        fx.op(I::I64And);
        fx.op(I::I64Const(0));
        fx.op(I::I64LtS);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(r));
        fx.op(I::Else);
        // op == 2 : mul
        fx.op(I::LocalGet(2));
        fx.op(I::I32Const(2));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(I64)));
        fx.op(I::LocalGet(ia));
        fx.op(I::I64Eqz);
        fx.op(I::If(BlockType::Result(I64)));
        fx.op(I::I64Const(0));
        fx.op(I::Else);
        // trap on ia==-1 && ib==INT_MIN (the one case r/ia would itself trap)
        fx.op(I::LocalGet(ia));
        fx.op(I::I64Const(-1));
        fx.op(I::I64Eq);
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Const(i64::MIN));
        fx.op(I::I64Eq);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(ia));
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Mul);
        fx.op(I::LocalSet(r));
        // overflow if r / ia != ib
        fx.op(I::LocalGet(r));
        fx.op(I::LocalGet(ia));
        fx.op(I::I64DivS);
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Ne);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(r));
        fx.op(I::End);
        fx.op(I::Else);
        // op == 3 : div
        fx.op(I::LocalGet(2));
        fx.op(I::I32Const(3));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Result(I64)));
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(ia));
        fx.op(I::I64Const(i64::MIN));
        fx.op(I::I64Eq);
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Const(-1));
        fx.op(I::I64Eq);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(ia));
        fx.op(I::LocalGet(ib));
        fx.op(I::I64DivS);
        fx.op(I::Else);
        // op == 4 : rem
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Eqz);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(ia));
        fx.op(I::I64Const(i64::MIN));
        fx.op(I::I64Eq);
        fx.op(I::LocalGet(ib));
        fx.op(I::I64Const(-1));
        fx.op(I::I64Eq);
        fx.op(I::I32And);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(ia));
        fx.op(I::LocalGet(ib));
        fx.op(I::I64RemS);
        fx.op(I::End); // op == 3
        fx.op(I::End); // op == 2
        fx.op(I::End); // op == 1
        fx.op(I::End); // op == 0
        let t = em.ty_idx(vec![I64, I64, I32], vec![I64]);
        em.bodies.push((t, fx.finish()));
    }

    // cmp_f64(x: f64, y: f64) -> i32 in {-1, 0, 1}; traps on NaN (the
    // interpreter's "values are not comparable"). The numeric tail of
    // cmp_raw, shared with the goal-5 typed scalar path.
    {
        let mut fx = FnCtx::new(2);
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(1));
        fx.op(I::F64Lt);
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::I32Const(-1));
        fx.op(I::Else);
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(1));
        fx.op(I::F64Gt);
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::I32Const(1));
        fx.op(I::Else);
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(1));
        fx.op(I::F64Eq);
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::I32Const(0));
        fx.op(I::Else);
        // unordered (NaN) — not comparable
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::End);
        let t = em.ty_idx(vec![F64, F64], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // ---- 5.1 persistent region: allocator + deep-copy write barrier
    //
    // Resource/functor components hold resource state that must survive the
    // per-call arena reset. That state lives in a PERSISTENT region below the
    // arena floor: global `persist_g` bumps up from `heap_base`, capped at the
    // arena floor (global `floor_g`); the arena grows above the floor and is
    // reset each post-return. A non-resource component has floor == heap_base
    // (zero reserve), so these helpers are emitted but never called.
    let floor_g = 2 + em.info.value_defs.len() as u32;
    let persist_g = 3 + em.info.value_defs.len() as u32;

    // persist_alloc(n) -> ptr  [param0=n, r=1, end=2]
    {
        let mut fx = FnCtx::new(1);
        let r = fx.local(I32);
        let end = fx.local(I32);
        fx.op(I::GlobalGet(persist_g));
        fx.op(I::LocalSet(r));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Const(7));
        fx.op(I::I32Add);
        fx.op(I::I32Const(-8));
        fx.op(I::I32And);
        fx.op(I::LocalSet(0));
        fx.op(I::LocalGet(r));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(end));
        // trap if the fixed persistent reserve is exhausted
        fx.op(I::LocalGet(end));
        fx.op(I::GlobalGet(floor_g));
        fx.op(I::I32GtU);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        fx.op(I::LocalGet(end));
        fx.op(I::GlobalSet(persist_g));
        fx.op(I::LocalGet(r));
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // persist(box) -> box  [param0=box, tg=1, sz=2, pbase=3, pcount=4, new=5, i=6, off=7]
    //
    // Interned/already-persistent nodes (box < arena_floor) are returned as-is.
    // An arena box is copied whole into the persistent region, then each of its
    // child pointer words is re-persisted recursively (a null child persists to
    // null, since 0 < arena_floor). `persist` is self-recursive via em.h.persist.
    {
        let mut fx = FnCtx::new(1);
        let tg = fx.local(I32);
        let sz = fx.local(I32);
        let pbase = fx.local(I32);
        let pcount = fx.local(I32);
        let new = fx.local(I32);
        let i = fx.local(I32);
        let off = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::GlobalGet(floor_g));
        fx.op(I::I32LtU);
        fx.op(I::If(BlockType::Result(I32)));
        fx.op(I::LocalGet(0)); // interned / already persistent
        fx.op(I::Else);
        // defaults: flat 16-byte box, no children (INT/DEC/CHAR)
        fx.op(I::I32Const(16));
        fx.op(I::LocalSet(sz));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(pbase));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(pcount));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(tg));
        // TAG_FN: closures in resource state unsupported
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_FN));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Unreachable);
        fx.op(I::End);
        // TAG_STR: sz = 8 + len; no children (bytes inline)
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_STR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(sz));
        fx.op(I::End);
        // TAG_LIST / TAG_TUP / TAG_FLG: pbase=8, pcount=n, sz=8+4n
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Eq);
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_TUP));
        fx.op(I::I32Eq);
        fx.op(I::I32Or);
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_FLG));
        fx.op(I::I32Eq);
        fx.op(I::I32Or);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(pcount));
        fx.op(I::I32Const(8));
        fx.op(I::LocalSet(pbase));
        fx.op(I::LocalGet(pcount));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(sz));
        fx.op(I::End);
        // TAG_REC: pbase=8, pcount=2n, sz=8+8n
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_REC));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Const(2));
        fx.op(I::I32Mul);
        fx.op(I::LocalSet(pcount));
        fx.op(I::I32Const(8));
        fx.op(I::LocalSet(pbase));
        fx.op(I::LocalGet(pcount));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(sz));
        fx.op(I::End);
        // TAG_VAR: pbase=4, pcount=2 (case ptr + payload; null payload persists to null), sz=12
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(4));
        fx.op(I::LocalSet(pbase));
        fx.op(I::I32Const(2));
        fx.op(I::LocalSet(pcount));
        fx.op(I::I32Const(12));
        fx.op(I::LocalSet(sz));
        fx.op(I::End);
        // TAG_CELL: pbase=4, pcount=1, sz=8
        fx.op(I::LocalGet(tg));
        fx.op(I::I32Const(TAG_CELL));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(4));
        fx.op(I::LocalSet(pbase));
        fx.op(I::I32Const(1));
        fx.op(I::LocalSet(pcount));
        fx.op(I::I32Const(8));
        fx.op(I::LocalSet(sz));
        fx.op(I::End);
        // new = persist_alloc(sz); memory.copy(new, box, sz)
        fx.op(I::LocalGet(sz));
        fx.op(I::Call(em.h.persist_alloc));
        fx.op(I::LocalSet(new));
        fx.op(I::LocalGet(new));
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(sz));
        fx.op(I::MemoryCopy { src_mem: 0, dst_mem: 0 });
        // for i in 0..pcount: new[pbase+4i] = persist(box[pbase+4i])
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(i));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(i));
        fx.op(I::LocalGet(pcount));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(pbase));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalSet(off));
        // dst = new + off
        fx.op(I::LocalGet(new));
        fx.op(I::LocalGet(off));
        fx.op(I::I32Add);
        // val = persist(box[off])
        fx.op(I::LocalGet(0));
        fx.op(I::LocalGet(off));
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::Call(em.h.persist));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(i));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(i));
        fx.op(I::Br(0));
        fx.op(I::End); // loop
        fx.op(I::End); // block
        fx.op(I::LocalGet(new));
        fx.op(I::End); // if box<floor
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    Ok(())
}
