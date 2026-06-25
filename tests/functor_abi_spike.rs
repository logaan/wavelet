//! Step 01 — resource-export ABI spike (see `dev-notes/functor/plan/01-abi-spike.typ`).
//!
//! This is a SCRATCH test, left `#[ignore]`d so it never runs in CI. Its job is
//! to empirically pin the exact wit-component 0.251 ABI contract for a core wasm
//! module that *implements an exported resource* (`set` with constructor /
//! add / contains / size + dtor). It hand-authors the smallest such core module
//! with `wasm-encoder` 0.251, runs it through the SAME pipeline `emit_component`
//! uses (`embed_component_metadata` + `ComponentEncoder::default().validate(true)
//! .module(...).encode()`), instantiates the result via `wavelet::host::
//! HostComponent`, and calls the constructor + a method.
//!
//! The verified ABI surface this proves is written up in
//! `dev-notes/functor/summaries/01-abi.typ` for step 02.
//!
//! Run it explicitly with: `cargo test --test functor_abi_spike -- --ignored --nocapture`

use wasm_encoder::{
    CodeSection, EntityType, ExportKind, ExportSection, Function, FunctionSection, ImportSection,
    Instruction as I, MemorySection, MemoryType, Module, TypeSection, ValType,
};
use wavelet::host::{HostComponent, Val};

// The WIT world the guest exports. Mirrors what `wit::functor_interface`
// synthesizes for `Import {pkg: "wavelet:coll/set" elem: s32 as: xs}`: a
// `s32-set` interface holding the element-specialized `set` resource. Element
// type s32 keeps the flattened `T` a single i32 (no realloc / string fuss).
const WIT: &str = r#"package demo:app@0.1.0;

interface s32-set {
  resource set {
    constructor();
    add: func(value: s32);
    contains: func(value: s32) -> bool;
    size: func() -> u32;
  }
}

world app {
  export s32-set;
}
"#;

const IFACE: &str = "demo:app/s32-set@0.1.0";

/// Hand-assemble the smallest core module implementing the `set` resource.
///
/// The names/signatures here are exactly what we are trying to verify; if any
/// are wrong, `ComponentEncoder.validate(true)` rejects the module with a
/// diagnostic that tells us the right one.
fn core_module() -> Vec<u8> {
    use ValType::I32;

    let mut types = TypeSection::new();
    // type 0: () -> i32        (resource.new : rep -> handle ; also ctor result)
    types.ty().function(vec![I32], vec![I32]);
    // type 1: (i32) -> i32     (resource.rep : handle -> rep ; size : self -> u32)
    types.ty().function(vec![I32], vec![I32]);
    // type 2: (i32) -> ()      (resource.drop : handle -> () ; dtor : rep -> ())
    types.ty().function(vec![I32], vec![]);
    // type 3: () -> i32        (constructor : () -> own handle)
    types.ty().function(vec![], vec![I32]);
    // type 4: (i32, i32) -> () (add : self, value -> ())
    types.ty().function(vec![I32, I32], vec![]);
    // type 5: (i32, i32) -> i32 (contains : self, value -> bool)
    types.ty().function(vec![I32, I32], vec![I32]);

    let mut imports = ImportSection::new();
    // The resource intrinsics the encoder provides. Module string + field names
    // are the unknown we are verifying; these are the canonical-ABI guesses.
    // import 0: resource.new  (rep i32) -> (handle i32)
    imports.import("[export]demo:app/s32-set@0.1.0", "[resource-new]set", EntityType::Function(0));
    // import 1: resource.rep  (handle i32) -> (rep i32)
    imports.import("[export]demo:app/s32-set@0.1.0", "[resource-rep]set", EntityType::Function(1));
    // import 2: resource.drop (handle i32) -> ()
    imports.import("[export]demo:app/s32-set@0.1.0", "[resource-drop]set", EntityType::Function(2));

    let n_imports = 3u32;
    // func indices after imports:
    //   3 ctor, 4 add, 5 contains, 6 size, 7 dtor
    let mut funcs = FunctionSection::new();
    funcs.function(3); // ctor   () -> i32
    funcs.function(4); // add    (i32,i32) -> ()
    funcs.function(5); // contains (i32,i32) -> i32
    funcs.function(1); // size   (i32) -> i32
    funcs.function(2); // dtor   (i32) -> ()

    let mut mems = MemorySection::new();
    mems.memory(MemoryType {
        minimum: 1,
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });

    // ---- bodies
    // ctor: mint an `own<set>` handle over a constant rep (42) via
    // resource.new(42). The returned i32 is what the constructor's `own<set>`
    // result lowers to. SPIKE_RAW_CTOR=1 probes whether the constructor may
    // instead return the bare rep (skipping resource.new): it can't — see notes.
    let mut ctor = Function::new([]);
    if std::env::var("SPIKE_RAW_CTOR").is_ok() {
        ctor.instruction(&I::I32Const(42)); // bare rep, no resource.new
    } else {
        ctor.instruction(&I::I32Const(42));
        ctor.instruction(&I::Call(0)); // resource.new : rep -> own handle
    }
    ctor.instruction(&I::End);

    // add: no-op. params: (self handle, value). drop nothing.
    let mut add = Function::new([]);
    add.instruction(&I::End);

    // contains: always false. params (self, value) -> i32 bool.
    let mut contains = Function::new([]);
    contains.instruction(&I::I32Const(0));
    contains.instruction(&I::End);

    // size: for a guest-EXPORTED resource, the borrowed `self` arrives at the
    // core boundary already lowered to the resource's *rep* i32 (NOT a handle
    // index — the handle table lives host-side). So `self` IS the rep; return it
    // directly as the u32. (Calling resource.rep on it traps: rep != handle.)
    let mut size = Function::new([]);
    size.instruction(&I::LocalGet(0));
    size.instruction(&I::End);

    // dtor: receives the resource's *rep* i32 (param 0), NOT a handle. A real
    // emitter can make this a no-op (the bump allocator never frees). Here we
    // assert the contract empirically: trap (`unreachable`) unless the rep is
    // the 42 our constructor minted — so a successful host-side drop proves the
    // dtor fired and was handed the rep. params (rep) -> ().
    let mut dtor = Function::new([]);
    dtor.instruction(&I::LocalGet(0));
    dtor.instruction(&I::I32Const(42));
    dtor.instruction(&I::I32Ne);
    dtor.instruction(&I::If(wasm_encoder::BlockType::Empty));
    dtor.instruction(&I::Unreachable);
    dtor.instruction(&I::End); // end if
    dtor.instruction(&I::End); // end func

    let mut code = CodeSection::new();
    code.function(&ctor);
    code.function(&add);
    code.function(&contains);
    code.function(&size);
    code.function(&dtor);

    let mut exports = ExportSection::new();
    exports.export("memory", ExportKind::Memory, 0);
    // The canonical core export names — the primary unknown we are verifying.
    exports.export("demo:app/s32-set@0.1.0#[constructor]set", ExportKind::Func, n_imports);
    exports.export("demo:app/s32-set@0.1.0#[method]set.add", ExportKind::Func, n_imports + 1);
    exports.export("demo:app/s32-set@0.1.0#[method]set.contains", ExportKind::Func, n_imports + 2);
    exports.export("demo:app/s32-set@0.1.0#[method]set.size", ExportKind::Func, n_imports + 3);
    exports.export("demo:app/s32-set@0.1.0#[dtor]set", ExportKind::Func, n_imports + 4);

    let mut module = Module::new();
    module.section(&types);
    module.section(&imports);
    module.section(&funcs);
    module.section(&mems);
    module.section(&exports);
    module.section(&code);
    module.finish()
}

/// Run the core module through the real emit pipeline and return component bytes.
fn componentize() -> Result<Vec<u8>, String> {
    let mut module = core_module();

    let mut resolve = wit_parser::Resolve::default();
    let pkg = resolve
        .push_str("spike.wit", WIT)
        .map_err(|e| format!("WIT parse: {e:#}"))?;
    let world = resolve
        .select_world(&[pkg], Some("app"))
        .map_err(|e| format!("world select: {e:#}"))?;

    wit_component::embed_component_metadata(
        &mut module,
        &resolve,
        world,
        wit_component::StringEncoding::UTF8,
    )
    .map_err(|e| format!("embed metadata: {e:#}"))?;

    let component = wit_component::ComponentEncoder::default()
        .validate(true)
        .module(&module)
        .map_err(|e| format!("componentize: {e:#}"))?
        .encode()
        .map_err(|e| format!("encode: {e:#}"))?;

    if std::env::var("SPIKE_DUMP").is_ok() {
        std::fs::write("/tmp/spike_embedded_core.wasm", &module).unwrap();
        std::fs::write("/tmp/spike_component.wasm", &component).unwrap();
        eprintln!("DUMPED /tmp/spike_embedded_core.wasm and /tmp/spike_component.wasm");
    }
    Ok(component)
}

#[test]
#[ignore = "scratch ABI spike; run with --ignored"]
fn resource_export_abi_spike() {
    let bytes = componentize().expect("spike component should componentize + validate");

    let mut c = HostComponent::from_bytes(&bytes).expect("spike component should instantiate");

    // constructor() -> own<set>  (returns a Resource handle)
    let ctor_out = c
        .call_instance(IFACE, "[constructor]set", &[])
        .expect("constructor call should succeed");
    let handle = match &ctor_out[0] {
        Val::Resource(_) => ctor_out[0].clone(),
        other => panic!("constructor should return a resource, got {other:?}"),
    };

    // size(self) -> u32 ; our ctor minted rep 42, size returns the rep.
    let size_out = c
        .call_instance(IFACE, "[method]set.size", &[handle.clone()])
        .expect("size call should succeed");
    assert_eq!(size_out, vec![Val::U32(42)], "size should return the minted rep");

    // add(self, value) -> () ; no-op, must not trap.
    c.call_instance(IFACE, "[method]set.add", &[handle.clone(), Val::S32(7)])
        .expect("add call should succeed");

    // contains(self, value) -> bool ; always false.
    let has = c
        .call_instance(IFACE, "[method]set.contains", &[handle.clone(), Val::S32(7)])
        .expect("contains call should succeed");
    assert_eq!(has, vec![Val::Bool(false)], "contains should return false");

    // Drop the handle host-side: this runs the guest `[dtor]set`, which traps
    // (unreachable) unless it was handed the rep 42. A clean drop therefore
    // proves the dtor fires and receives the *rep* (not a handle).
    c.drop_resource(handle).expect("dropping the set handle should run the dtor cleanly");

    eprintln!("SPIKE OK: ctor returned a resource; size={size_out:?}; contains={has:?}; dtor ran on rep 42");
}
