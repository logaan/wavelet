use super::*;
use ValType::I32;

pub(super) fn emit_persist_alloc(em: &mut Emitter) {
    let floor_g = em.arena_floor_global();
    let persist_g = em.arena_floor_global() + 1;

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

pub(super) fn emit_persist(em: &mut Emitter) {
    let floor_g = em.arena_floor_global();

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
    fx.op(I::LocalGet(0));
    fx.op(I::Else);

    fx.op(I::I32Const(16));
    fx.op(I::LocalSet(sz));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(pbase));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(pcount));
    fx.op(I::LocalGet(0));
    fx.op(I::I32Load(ma(0, 2)));
    fx.op(I::LocalSet(tg));

    fx.op(I::LocalGet(tg));
    fx.op(I::I32Const(TAG_FN));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::Unreachable);
    fx.op(I::End);

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

    fx.op(I::LocalGet(sz));
    fx.op(I::Call(em.h.persist_alloc));
    fx.op(I::LocalSet(new));
    fx.op(I::LocalGet(new));
    fx.op(I::LocalGet(0));
    fx.op(I::LocalGet(sz));
    fx.op(I::MemoryCopy {
        src_mem: 0,
        dst_mem: 0,
    });

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

    fx.op(I::LocalGet(new));
    fx.op(I::LocalGet(off));
    fx.op(I::I32Add);

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
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::LocalGet(new));
    fx.op(I::End);
    let t = em.ty_idx(vec![I32], vec![I32]);
    em.bodies.push((t, fx.finish()));
}
