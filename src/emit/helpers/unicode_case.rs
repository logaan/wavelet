use super::*;
use ValType::I32;

mod scan;

#[derive(Clone, Copy)]
struct Table {
    address: i32,
    count: i32,
}

struct Data {
    upper: Table,
    lower: Table,
    context: Table,
    blob: i32,
    final_sigma: i32,
    expansion: i32,
}

impl Data {
    fn insert(em: &mut Emitter) -> Self {
        let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/unicode-case.bin"));
        let word = |offset| i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        em.align8();
        let base = (DATA_BASE + em.data.len() as u32) as i32;
        em.data.extend_from_slice(bytes);
        let upper = Table {
            address: base + 20,
            count: word(0),
        };
        let lower = Table {
            address: upper.address + upper.count * 12,
            count: word(4),
        };
        let context = Table {
            address: lower.address + lower.count * 12,
            count: word(8),
        };
        Self {
            upper,
            lower,
            context,
            blob: context.address + context.count * 12,
            final_sigma: word(12),
            expansion: word(16),
        }
    }
}

struct Locals {
    input: u32,
    end: u32,
    output: u32,
    destination: u32,
    next: u32,
    scalar: u32,
    preceding_cased: u32,
    class: u32,
    mapping: u32,
    source: u32,
    length: u32,
}

impl Locals {
    fn new(fx: &mut FnCtx) -> Self {
        Self {
            input: fx.local(I32),
            end: fx.local(I32),
            output: fx.local(I32),
            destination: fx.local(I32),
            next: fx.local(I32),
            scalar: fx.local(I32),
            preceding_cased: fx.local(I32),
            class: fx.local(I32),
            mapping: fx.local(I32),
            source: fx.local(I32),
            length: fx.local(I32),
        }
    }
}

fn initialize(fx: &mut FnCtx, h: &Helpers, data: &Data, l: &Locals) {
    fx.op(I::LocalGet(0));
    fx.op(I::I32Load(ma(0, 2)));
    fx.op(I::I32Const(TAG_STR));
    fx.op(I::I32Ne);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::Unreachable);
    fx.op(I::End);
    fx.op(I::LocalGet(0));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalTee(l.length));
    fx.op(I::I32Const(
        ((u32::MAX - 15) / data.expansion as u32) as i32,
    ));
    fx.op(I::I32GtU);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::Unreachable);
    fx.op(I::End);
    fx.op(I::LocalGet(0));
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::LocalTee(l.input));
    fx.op(I::LocalGet(l.length));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(l.end));
    fx.op(I::LocalGet(l.length));
    fx.op(I::I32Const(data.expansion));
    fx.op(I::I32Mul);
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::Call(h.alloc));
    fx.op(I::LocalTee(l.destination));
    fx.op(I::I32Const(TAG_STR));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(l.destination));
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(l.output));
}

fn select_mapping(fx: &mut FnCtx, h: &Helpers, data: &Data, l: &Locals) {
    fx.op(I::LocalGet(1));
    fx.op(I::If(BlockType::Result(I32)));
    lookup(fx, h, l.scalar, data.upper);
    fx.op(I::Else);
    lookup(fx, h, l.scalar, data.lower);
    fx.op(I::End);
    fx.op(I::LocalSet(l.mapping));
    fx.op(I::LocalGet(1));
    fx.op(I::I32Eqz);
    fx.op(I::LocalGet(l.scalar));
    fx.op(I::I32Const('Σ' as i32));
    fx.op(I::I32Eq);
    fx.op(I::I32And);
    fx.op(I::LocalGet(l.preceding_cased));
    fx.op(I::I32And);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(l.next));
    fx.op(I::LocalGet(l.end));
    fx.op(I::Call(h.unicode_following));
    fx.op(I::I32Eqz);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::I32Const(data.final_sigma));
    fx.op(I::LocalSet(l.mapping));
    fx.op(I::End);
    fx.op(I::End);
}

fn copy_mapping(fx: &mut FnCtx, data: &Data, l: &Locals) {
    fx.op(I::LocalGet(l.mapping));
    fx.op(I::If(BlockType::Empty));
    fx.op(I::I32Const(data.blob));
    fx.op(I::LocalGet(l.mapping));
    fx.op(I::I32Const(4));
    fx.op(I::I32ShrU);
    fx.op(I::I32Add);
    fx.op(I::LocalSet(l.source));
    fx.op(I::LocalGet(l.mapping));
    fx.op(I::I32Const(15));
    fx.op(I::I32And);
    fx.op(I::LocalSet(l.length));
    fx.op(I::Else);
    fx.op(I::LocalGet(l.input));
    fx.op(I::LocalSet(l.source));
    fx.op(I::LocalGet(l.next));
    fx.op(I::LocalGet(l.input));
    fx.op(I::I32Sub);
    fx.op(I::LocalSet(l.length));
    fx.op(I::End);
    fx.op(I::LocalGet(l.output));
    fx.op(I::LocalGet(l.source));
    fx.op(I::LocalGet(l.length));
    fx.op(I::MemoryCopy {
        src_mem: 0,
        dst_mem: 0,
    });
    fx.op(I::LocalGet(l.output));
    fx.op(I::LocalGet(l.length));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(l.output));
}

fn lookup(fx: &mut FnCtx, h: &Helpers, scalar: u32, table: Table) {
    fx.op(I::LocalGet(scalar));
    fx.op(I::I32Const(table.address));
    fx.op(I::I32Const(table.count));
    fx.op(I::Call(h.unicode_lookup));
}

fn begin_scan(fx: &mut FnCtx, cursor: u32, end: u32) {
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(cursor));
    fx.op(I::LocalGet(end));
    fx.op(I::I32GeU);
    fx.op(I::BrIf(1));
}

fn end_scan(fx: &mut FnCtx) {
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
}

pub(super) fn emit_case(em: &mut Emitter) {
    let data = Data::insert(em);
    let mut fx = FnCtx::new(2);
    let l = Locals::new(&mut fx);
    initialize(&mut fx, &em.h, &data, &l);
    begin_scan(&mut fx, l.input, l.end);
    fx.op(I::LocalGet(l.input));
    fx.op(I::Call(em.h.unicode_decode));
    fx.op(I::LocalSet(l.next));
    fx.op(I::LocalSet(l.scalar));
    select_mapping(&mut fx, &em.h, &data, &l);
    copy_mapping(&mut fx, &data, &l);
    lookup(&mut fx, &em.h, l.scalar, data.context);
    fx.op(I::LocalTee(l.class));
    fx.op(I::I32Const(2));
    fx.op(I::I32Ne);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(l.class));
    fx.op(I::I32Const(1));
    fx.op(I::I32Eq);
    fx.op(I::LocalSet(l.preceding_cased));
    fx.op(I::End);
    fx.op(I::LocalGet(l.next));
    fx.op(I::LocalSet(l.input));
    end_scan(&mut fx);
    fx.op(I::LocalGet(l.destination));
    fx.op(I::LocalGet(l.output));
    fx.op(I::LocalGet(l.destination));
    fx.op(I::I32Sub);
    fx.op(I::I32Const(8));
    fx.op(I::I32Sub);
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::LocalGet(l.destination));
    let t = em.ty_idx(vec![I32, I32], vec![I32]);
    em.bodies.push((t, fx.finish()));
    scan::emit_decode(em);
    scan::emit_lookup(em);
    scan::emit_following(em, data.context);
}
