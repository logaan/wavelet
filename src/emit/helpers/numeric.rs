use super::*;
use ValType::{F64, I32, I64};

pub(super) fn emit_as_float(em: &mut Emitter) {
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

pub(super) fn emit_arithmetic(em: &mut Emitter) {
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

pub(super) fn emit_compare(em: &mut Emitter) {
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

pub(super) fn emit_negate(em: &mut Emitter) {
    let mut fx = FnCtx::new(1);
    fx.op(I::LocalGet(0));
    fx.op(I::I32Load(ma(0, 2)));
    fx.op(I::I32Const(TAG_INT));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Result(I32)));
    fx.op(I::I64Const(0));
    fx.op(I::LocalGet(0));
    fx.op(I::I64Load(ma(8, 3)));
    fx.op(I::I32Const(1));
    fx.op(I::Call(em.h.arith_int));
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

pub(super) fn emit_integer_arithmetic(em: &mut Emitter) {
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

pub(super) fn emit_float_compare(em: &mut Emitter) {
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
