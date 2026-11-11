use super::*;
use ValType::{F64, I32, I64};

pub(super) fn emit_equality(em: &mut Emitter) {
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

pub(super) fn emit_length(em: &mut Emitter) {
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

pub(super) fn emit_head(em: &mut Emitter) {
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

pub(super) fn emit_tail(em: &mut Emitter) {
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

pub(super) fn emit_concat(em: &mut Emitter) {
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

pub(super) fn emit_case(em: &mut Emitter) {
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

pub(super) fn emit_record_get(em: &mut Emitter) {
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
