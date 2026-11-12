use super::{DATA_BASE, Emitter, PERSIST_RESERVE};
use wasm_encoder::{
    CodeSection, ConstExpr, DataSection, ElementSection, Elements, EntityType, ExportKind,
    ExportSection, FunctionSection, GlobalSection, GlobalType, ImportSection, MemorySection,
    MemoryType, Module, RefType, TableSection, TableType, TypeSection, ValType,
};

pub(super) fn serialize_module(em: &mut Emitter<'_>, exports: &[(String, u32)]) -> Vec<u8> {
    use ValType::I32;
    let info = em.info;
    let heap_base = {
        em.align8();
        DATA_BASE + em.data.len() as u32
    };

    let persist_reserve = if em.has_persist() { PERSIST_RESERVE } else { 0 };
    let arena_floor = heap_base + persist_reserve;
    let pages = (arena_floor as u64 >> 16) + 1;

    let mut module = Module::new();
    let mut ts = TypeSection::new();
    for (p, r) in &em.types {
        ts.ty().function(p.iter().copied(), r.iter().copied());
    }
    module.section(&ts);

    let mut is = ImportSection::new();
    for (m, f, t) in &em.imports {
        is.import(m, f, EntityType::Function(*t));
    }
    module.section(&is);

    let closure_base = em.imports.len() as u32 + em.bodies.len() as u32;

    let mut fs = FunctionSection::new();
    for (t, _) in &em.bodies {
        fs.function(*t);
    }
    for (t, _) in &em.closure_bodies {
        fs.function(*t);
    }
    module.section(&fs);

    if !em.closure_bodies.is_empty() {
        let mut tbl = TableSection::new();
        tbl.table(TableType {
            element_type: RefType::FUNCREF,
            minimum: em.closure_bodies.len() as u64,
            maximum: Some(em.closure_bodies.len() as u64),
            table64: false,
            shared: false,
        });
        module.section(&tbl);
    }

    let mut ms = MemorySection::new();
    ms.memory(MemoryType {
        minimum: pages,
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    module.section(&ms);

    let mut gs = GlobalSection::new();

    gs.global(
        GlobalType {
            val_type: I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(arena_floor as i32),
    );
    for _ in &info.value_defs {
        gs.global(
            GlobalType {
                val_type: I32,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i32_const(0),
        );
    }

    gs.global(
        GlobalType {
            val_type: ValType::I64,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i64_const(0),
    );

    gs.global(
        GlobalType {
            val_type: I32,
            mutable: false,
            shared: false,
        },
        &ConstExpr::i32_const(arena_floor as i32),
    );

    gs.global(
        GlobalType {
            val_type: I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(heap_base as i32),
    );
    module.section(&gs);

    let mut es = ExportSection::new();
    es.export("memory", ExportKind::Memory, 0);
    es.export("cabi_realloc", ExportKind::Func, em.h.realloc);
    for (name, idx) in exports {
        es.export(name, ExportKind::Func, *idx);
    }
    module.section(&es);

    if !em.closure_bodies.is_empty() {
        let idxs: Vec<u32> = (0..em.closure_bodies.len() as u32)
            .map(|k| closure_base + k)
            .collect();
        let mut els = ElementSection::new();
        els.active(
            Some(0),
            &ConstExpr::i32_const(0),
            Elements::Functions(idxs.into()),
        );
        module.section(&els);
    }

    let mut cs = CodeSection::new();
    for (_, f) in &em.bodies {
        cs.function(f);
    }
    for (_, f) in &em.closure_bodies {
        cs.function(f);
    }
    module.section(&cs);

    let mut ds = DataSection::new();
    ds.active(
        0,
        &ConstExpr::i32_const(DATA_BASE as i32),
        em.data.iter().copied(),
    );
    module.section(&ds);

    module.finish()
}
