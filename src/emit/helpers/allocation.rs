use super::*;
use ValType::{F64, I32, I64};

pub(super) fn emit_alloc(em: &mut Emitter) {
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

pub(super) fn emit_realloc(em: &mut Emitter) {
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

pub(super) fn emit_box_int(em: &mut Emitter) {
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

pub(super) fn emit_box_bool(em: &mut Emitter) {
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

pub(super) fn emit_box_dec(em: &mut Emitter) {
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

pub(super) fn emit_box_str(em: &mut Emitter) {
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

pub(super) fn emit_truthy(em: &mut Emitter) {
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

pub(super) fn emit_unbox_int(em: &mut Emitter) {
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

pub(super) fn emit_unbox_char(em: &mut Emitter) {
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

pub(super) fn emit_unbox_dec(em: &mut Emitter) {
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
