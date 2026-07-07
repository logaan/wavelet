//! Resource bodies: user `DefResource` classification and the functor `set`
//! resource implementation (ctor/add/contains/size/dtor), with their tests.

use super::*;

// ------------------------------------------------- functor `set` resource bodies
//
// Emit the core wasm functions that implement a `set` resource for ONE
// instantiation at element type `elem`. This is the
// "guest implements an exported resource" case; the bodies mirror the
// interpreter's `Value::Cell(Rc<RefCell<Value::Lst>>)` set, with structural
// `eq_raw` membership (the project's one hard rule).
//
// REP LAYOUT (mirrors the interpreter):
//   * A `set` rep is a pointer to a one-word mutable CELL: `[i32 list-ptr]`.
//     The mutable cell gives the resource a stable identity so a later
//     `contains`/`size` observes earlier `add`s — exactly `RefCell` semantics.
//   * The cell's word points at the existing boxed-list layout
//     `[TAG_LIST, len, elem-ptr…]` (TAG_LIST=3; `len` is the i32 word @4).
//   * Elements are stored as boxed values (the same heap boxes the rest of the
//     backend uses), so `eq_raw`/list iteration operate uniformly across any
//     element type (record / string / primitive).
//
// ABI (from summary 01, the THING TO GET RIGHT):
//   * constructor `() -> i32`: mint an OWN handle with `resource.new(cell)`.
//   * every method's param 0 (`self`, a `borrow`) arrives as the REP DIRECTLY —
//     i.e. the cell ptr we passed to `resource.new`. Do NOT call `resource.rep`
//     on it (that traps "unknown handle index"; `resource.rep` is for the
//     opposite direction). Use param 0 as the cell ptr verbatim.
//   * dtor `(i32 rep) -> ()`: safe no-op (bump allocator never frees).
//   * `contains`/`size` return a bare core i32 (the encoder's `canon lift` does
//     i32→bool / i32→u32), so no value-`lower` is needed on the result.

/// Routing data for one user-declared resource (4.5). `methods`/`statics` are
/// the member op-names; a method call converts the receiver handle to its rep
/// with `rep_import` before dispatch, a static call is a plain internal call.
#[derive(Clone)]
pub(crate) struct UserRes {
    pub(crate) methods: std::collections::HashSet<String>,
    pub(crate) statics: std::collections::HashSet<String>,
    /// core fn idx of the `[dtor]<name>` body (runs `Drop` or no-op).
    pub(crate) dtor: u32,
}

/// One `DefResource` form, classified for the backend: the constructor (`New`),
/// instance methods, statics, and destructor (`Drop`), each as `(params, body)`
/// arena node ids. Mirrors the interpreter's `defresource-MACRO` handling.
pub(crate) struct ResForm {
    pub(crate) name: String,
    pub(crate) ctor: Option<(NodeId, NodeId)>,
    pub(crate) methods: Vec<(String, NodeId, NodeId)>,
    pub(crate) statics: Vec<(String, NodeId, NodeId)>,
    pub(crate) drop_member: Option<(NodeId, NodeId)>,
}

/// Walk the file roots for `DefResource` forms, classifying each member the same
/// way the interpreter and WIT collector do (`New`/`Drop` keys, `Static Fn`
/// marker, otherwise an instance method).
pub(crate) fn user_resource_forms(arena: &Arena, roots: &[NodeId]) -> Result<Vec<ResForm>, String> {
    let mut out = Vec::new();
    for &root in roots {
        let Node::Tup(items) = arena.node(root) else {
            continue;
        };
        if items.len() < 3
            || !matches!(arena.node(items[0]), Node::Sym(s) if s == "defresource-MACRO")
        {
            continue;
        }
        let Node::Sym(name) = arena.node(items[1]) else {
            continue;
        };
        let Node::Rec(members) = arena.node(items[2]) else {
            continue;
        };
        let mut rf = ResForm {
            name: name.clone(),
            ctor: None,
            methods: Vec::new(),
            statics: Vec::new(),
            drop_member: None,
        };
        for (key, val) in members {
            let (is_static, fn_id) = match arena.node(*val) {
                Node::Tup(t)
                    if t.len() == 2
                        && matches!(arena.node(t[0]), Node::Sym(s) if s == "static-MACRO") =>
                {
                    (true, t[1])
                }
                _ => (false, *val),
            };
            let Node::Tup(fnitems) = arena.node(fn_id) else {
                return Err(format!("resource `{name}` member `{key}` must be an `Fn`"));
            };
            if fnitems.len() != 3
                || !matches!(arena.node(fnitems[0]), Node::Sym(s) if s == "fn-MACRO")
            {
                return Err(format!("resource `{name}` member `{key}` must be an `Fn`"));
            }
            let (params_id, body) = (fnitems[1], fnitems[2]);
            match key.as_str() {
                "New" => rf.ctor = Some((params_id, body)),
                "Drop" => rf.drop_member = Some((params_id, body)),
                other if is_static => rf.statics.push((other.to_string(), params_id, body)),
                other => rf.methods.push((other.to_string(), params_id, body)),
            }
        }
        out.push(rf);
    }
    Ok(out)
}

/// The five core functions implementing one `set` instantiation, by core
/// function index, plus the resource-intrinsic import indices their bodies
/// reference. Step 03 wires these into the export/import sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceFns {
    /// `[constructor]set` — core sig `() -> i32` (returns an OWN handle).
    pub ctor: u32,
    /// `[method]set.add` — core sig `(i32 self, <flat elem>) -> ()`.
    pub add: u32,
    /// `[method]set.contains` — core sig `(i32 self, <flat elem>) -> i32` (0/1).
    pub contains: u32,
    /// `[method]set.size` — core sig `(i32 self) -> i32` (u32 count).
    pub size: u32,
    /// `[dtor]set` — core sig `(i32 rep) -> ()` (no-op).
    pub dtor: u32,
    /// import idx of `[resource-new]set` `(i32 rep) -> i32 handle` (ctor uses it).
    pub new_import: u32,
    /// import idx of `[resource-rep]set` `(i32 handle) -> i32 rep`. The bodies do
    /// NOT call this (methods already receive the rep), but it is declared and
    /// carried so step 03 can wire the intrinsic table the encoder expects.
    pub rep_import: u32,
    /// import idx of `[resource-drop]set` `(i32 handle) -> ()`. Unused by the
    /// bodies; carried for the intrinsic table.
    pub drop_import: u32,
}

/// Build an empty boxed list `[TAG_LIST, 0]` (8 bytes), leaving its ptr on the
/// stack.
pub(crate) fn emit_empty_list_box(em: &mut Emitter, fx: &mut FnCtx) {
    let p = fx.local(ValType::I32);
    fx.op(I::I32Const(8));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(p));
    fx.op(I::LocalGet(p));
    fx.op(I::I32Const(TAG_LIST));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(p));
    fx.op(I::I32Const(0));
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::LocalGet(p));
}

/// Linear-scan the boxed list in local `list` for a box structurally-equal (via
/// `eq_raw`) to the box in local `needle`. Leaves an i32 0/1 on the stack: 1 if
/// present, else 0. Allocates two fresh i32 locals (`i`, `n`) internally.
pub(crate) fn emit_list_contains(em: &mut Emitter, fx: &mut FnCtx, list: u32, needle: u32) {
    let i = fx.local(ValType::I32);
    let n = fx.local(ValType::I32);
    // n = list.len  (@4)
    fx.op(I::LocalGet(list));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(n));
    // i = 0
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(i));
    // result accumulated as a block that returns i32: default 0, early-return 1
    fx.op(I::Block(BlockType::Result(ValType::I32)));
    fx.op(I::Loop(BlockType::Empty));
    // if i >= n: break out with 0
    fx.op(I::LocalGet(i));
    fx.op(I::LocalGet(n));
    fx.op(I::I32GeU);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::I32Const(0));
    fx.op(I::Br(2)); // br to the result block, yielding 0
    fx.op(I::End);
    // elem = list[8 + 4*i]
    fx.op(I::LocalGet(list));
    fx.op(I::LocalGet(i));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(8, 2)));
    // eq_raw(elem, needle)
    fx.op(I::LocalGet(needle));
    fx.op(I::Call(em.h.eq_raw));
    fx.op(I::If(BlockType::Empty));
    fx.op(I::I32Const(1));
    fx.op(I::Br(2)); // present → result block yields 1
    fx.op(I::End);
    // i += 1; continue
    fx.op(I::LocalGet(i));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(i));
    fx.op(I::Br(0)); // loop
    fx.op(I::End); // loop
    // unreachable fallthrough: the loop only exits via Br(2)
    fx.op(I::I32Const(0));
    fx.op(I::End); // block → i32 on stack
}

/// Build a NEW boxed list whose elements are the old list's elements followed by
/// the box in local `extra`. Leaves the new list-box ptr on the stack. Allocates
/// fresh i32 locals internally.
pub(crate) fn emit_list_append(em: &mut Emitter, fx: &mut FnCtx, old: u32, extra: u32) {
    let n = fx.local(ValType::I32); // old length
    let new = fx.local(ValType::I32); // new list ptr
    let i = fx.local(ValType::I32); // copy cursor
    // n = old.len (@4)
    fx.op(I::LocalGet(old));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(n));
    // new = alloc(8 + 4*(n+1))
    fx.op(I::I32Const(8 + 4));
    fx.op(I::LocalGet(n));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(new));
    // new.tag = TAG_LIST
    fx.op(I::LocalGet(new));
    fx.op(I::I32Const(TAG_LIST));
    fx.op(I::I32Store(ma(0, 2)));
    // new.len = n + 1
    fx.op(I::LocalGet(new));
    fx.op(I::LocalGet(n));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::I32Store(ma(4, 2)));
    // copy old elems: for i in 0..n: new[8+4i] = old[8+4i]
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(i));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(i));
    fx.op(I::LocalGet(n));
    fx.op(I::I32GeU);
    fx.op(I::BrIf(1)); // exit copy loop
    // new[8 + 4*i] = old[8 + 4*i]
    fx.op(I::LocalGet(new));
    fx.op(I::LocalGet(i));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::LocalGet(old));
    fx.op(I::LocalGet(i));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(8, 2)));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(i));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(i));
    fx.op(I::Br(0));
    fx.op(I::End); // loop
    fx.op(I::End); // block
    // new[8 + 4*n] = extra
    fx.op(I::LocalGet(new));
    fx.op(I::LocalGet(n));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::LocalGet(extra));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(new));
}

/// Emit the five `set` bodies for one instantiation at element type `elem`,
/// appending them to `em.bodies` and returning their indices in [`ResourceFns`].
///
/// The three resource-intrinsic import indices (`[resource-new/rep/drop]set`)
/// are passed in: imports must be declared before any helper/body index is
/// assigned (function index space is imports-first), so step 03 declares them in
/// the up-front import loop and hands the indices here. This keeps the function
/// purely additive over `em.bodies` and side-effect-free on the import section.
///
/// `inst` is accepted for symmetry / future per-instantiation specialisation
/// (e.g. distinct rep tags) but is not needed by the current bodies; the only
/// thing that varies is `elem`, which drives the flat param shape and boxing.
pub(crate) fn emit_set_resource(
    em: &mut Emitter,
    _inst: &crate::wit::FunctorInst,
    elem: &WitTy,
    new_import: u32,
    rep_import: u32,
    drop_import: u32,
) -> Result<ResourceFns, String> {
    use ValType::I32;
    // Flat core params of the element value (after `self`), in canonical-ABI flat
    // order. `flat_checked` rejects element types the backend can't flatten.
    let elem_flat = flat_checked(elem)?;
    let n_elem = elem_flat.len() as u32;

    // The function index of the next body we push. Bodies are emitted later (in
    // the same order) at `n_imports + position`; the caller assigns that base.
    let body_base = em.imports.len() as u32;
    let mut next_idx = body_base + em.bodies.len() as u32;
    let mut alloc_idx = || {
        let i = next_idx;
        next_idx += 1;
        i
    };

    // ---- constructor: () -> i32 (own handle)
    let ctor = alloc_idx();
    {
        let mut fx = FnCtx::new(0);
        let cell = fx.local(I32);
        // cell = persist_alloc(4): the set rep must survive the per-call arena
        // reset, so it lives in the persistent region with a stable identity.
        fx.op(I::I32Const(4));
        fx.op(I::Call(em.h.persist_alloc));
        fx.op(I::LocalSet(cell));
        // cell[0] = empty list box, deep-copied into the persistent region
        fx.op(I::LocalGet(cell));
        emit_empty_list_box(em, &mut fx);
        fx.op(I::Call(em.h.persist));
        fx.op(I::I32Store(ma(0, 2)));
        // resource.new(cell) -> handle ; return it
        fx.op(I::LocalGet(cell));
        fx.op(I::Call(new_import));
        let t = em.ty_idx(vec![], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // ---- add: (i32 self, <flat elem>) -> ()
    //   self IS the cell ptr (see ABI note). value flats start at local 1.
    let add = alloc_idx();
    {
        let mut params = vec![I32];
        params.extend_from_slice(&elem_flat);
        let mut fx = FnCtx::new(params.len() as u32);
        let list = fx.local(I32);
        let needle = fx.local(I32);
        // list = *self  (the cell's word)
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(list));
        // needle = box the flattened incoming value (param locals 1..1+n_elem)
        em.lift_flat(&mut fx, elem, 1)?;
        fx.op(I::LocalSet(needle));
        // if present → return (dedup-on-add by Value equality)
        emit_list_contains(em, &mut fx, list, needle);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Return);
        fx.op(I::End);
        // *self = persist(old list + needle): the grown list is built in the
        // arena, then deep-copied into the persistent region so it outlives the
        // reset (already-persistent elements are kept in place, only the new
        // needle is copied).
        fx.op(I::LocalGet(0));
        emit_list_append(em, &mut fx, list, needle);
        fx.op(I::Call(em.h.persist));
        fx.op(I::I32Store(ma(0, 2)));
        let _ = n_elem; // silence if elem flattens to zero (no such WitTy today)
        let t = em.ty_idx(params, vec![]);
        em.bodies.push((t, fx.finish()));
    }

    // ---- contains: (i32 self, <flat elem>) -> i32 (0/1)
    let contains = alloc_idx();
    {
        let mut params = vec![I32];
        params.extend_from_slice(&elem_flat);
        let mut fx = FnCtx::new(params.len() as u32);
        let list = fx.local(I32);
        let needle = fx.local(I32);
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(list));
        em.lift_flat(&mut fx, elem, 1)?;
        fx.op(I::LocalSet(needle));
        emit_list_contains(em, &mut fx, list, needle); // i32 0/1 on stack
        let t = em.ty_idx(params, vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // ---- size: (i32 self) -> i32 (u32 element count)
    let size = alloc_idx();
    {
        let mut fx = FnCtx::new(1);
        // *self -> list ptr ; load len @4
        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Load(ma(4, 2)));
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    // ---- dtor: (i32 rep) -> ()  — safe no-op (bump allocator never frees)
    let dtor = alloc_idx();
    {
        let fx = FnCtx::new(1);
        let t = em.ty_idx(vec![I32], vec![]);
        em.bodies.push((t, fx.finish()));
    }

    Ok(ResourceFns {
        ctor,
        add,
        contains,
        size,
        dtor,
        new_import,
        rep_import,
        drop_import,
    })
}

// ---------------------------------------------------------- set-resource tests
//
// Step 02 verification. We drive the REAL `emit_set_resource` bodies (not the
// hand-authored spike ones) through the SAME `embed_component_metadata` +
// `ComponentEncoder` pipeline `emit_component` uses, then instantiate via
// `HostComponent` and exercise ctor → add (incl. a duplicate) → size → contains.
// This proves the rep/list/eq_raw bodies dedup and answer membership correctly.
// It does NOT go through `emit_component` (that wiring is step 03), so it is a
// minimal hand-assembled module — but every `set` body is the production one.
#[cfg(test)]
mod set_resource_tests {
    use super::*;
    use crate::host::{HostComponent, Val};
    use crate::wit::{FunctorInst, FunctorKind};

    const IFACE: &str = "demo:app/s32-set@0.1.0";
    const EXPORT_MOD: &str = "[export]demo:app/s32-set@0.1.0";

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

    /// Stand up a minimal `Emitter` exactly as `emit_core_module` does up to the
    /// point `emit_set_resource` needs (static boxes, the three resource-intrinsic
    /// imports, helper indices, helper bodies), call `emit_set_resource`, and
    /// assemble a core module with the verified ABI export names.
    fn build_core(elem: &WitTy) -> Result<Vec<u8>, String> {
        use ValType::I32;

        // A FileInfo / deps with nothing in them: the set bodies are self-contained.
        let arena = Arena::new();
        let info = FileInfo {
            package: "demo:app@0.1.0".to_string(),
            package_path: "demo:app".to_string(),
            world: "app".to_string(),
            imports: Vec::new(),
            functors: Vec::new(),
            exports: Vec::new(),
            types: Vec::new(),
            defs: HashMap::new(),
            fn_defs: HashMap::new(),
            value_defs: Vec::new(),
            overload_bodies: HashMap::new(),
            resources: Vec::new(),
        };
        let deps: HashMap<String, Dep> = HashMap::new();

        let mut em = Emitter {
            arena: &arena,
            info: &info,
            deps: &deps,
            type_env: TypeEnv::default(),
            local_cases: HashMap::new(),
            data: Vec::new(),
            str_cache: HashMap::new(),
            types: Vec::new(),
            imports: Vec::new(),
            import_fn: HashMap::new(),
            h: Helpers::default(),
            funcs: HashMap::new(),
            value_globals: HashMap::new(),
            compiling_values: Vec::new(),
            bodies: Vec::new(),
            closure_bodies: Vec::new(),
        known_fn_names: Vec::new(),
        known_lambdas: Vec::new(),
        lambda_reserved: HashMap::new(),
        lambda_order: Vec::new(),
        lambda_stash: HashMap::new(),
            fn_wrappers: HashMap::new(),
            fn_box_cache: HashMap::new(),
            var_box_cache: HashMap::new(),
            false_addr: 0,
            true_addr: 0,
            macro_expand_idx: None,
            functor_fns: HashMap::new(),
        user_res: HashMap::new(),
            node_types: Default::default(),
            mem_tys: Vec::new(),
        };

        // static boxes: false @16, true @24 (same as emit_core_module).
        em.false_addr = DATA_BASE;
        em.put_i32(TAG_BOOL);
        em.put_i32(0);
        em.true_addr = DATA_BASE + 8;
        em.put_i32(TAG_BOOL);
        em.put_i32(1);

        // ---- imports: the three resource intrinsics, declared up front so the
        // function index space is imports-first (exactly emit_component's order).
        let mut n_imports = 0u32;
        let mut add_import = |em: &mut Emitter, field: &str, p: Vec<ValType>, r: Vec<ValType>| {
            let t = em.ty_idx(p, r);
            em.imports
                .push((EXPORT_MOD.to_string(), field.to_string(), t));
            em.import_fn
                .insert((EXPORT_MOD.to_string(), field.to_string()), n_imports);
            n_imports += 1;
        };
        add_import(&mut em, "[resource-new]set", vec![I32], vec![I32]);
        add_import(&mut em, "[resource-rep]set", vec![I32], vec![I32]);
        add_import(&mut em, "[resource-drop]set", vec![I32], vec![]);
        let new_i = em.import_idx(EXPORT_MOD, "[resource-new]set");
        let rep_i = em.import_idx(EXPORT_MOD, "[resource-rep]set");
        let drop_i = em.import_idx(EXPORT_MOD, "[resource-drop]set");

        // ---- helper indices (same order/assignment as emit_core_module).
        let mut next = n_imports;
        let mut take = || {
            let i = next;
            next += 1;
            i
        };
        em.h.alloc = take();
        em.h.realloc = take();
        em.h.box_int = take();
        em.h.box_bool = take();
        em.h.box_dec = take();
        em.h.box_str = take();
        em.h.truthy = take();
        em.h.unbox_int = take();
        em.h.unbox_char = take();
        em.h.unbox_dec = take();
        em.h.eq_raw = take();
        em.h.len_raw = take();
        em.h.head_h = take();
        em.h.tail_h = take();
        em.h.strcat2 = take();
        em.h.case_h = take();
        em.h.to_str = take();
        em.h.rec_get = take();
        em.h.as_f64 = take();
        em.h.arith_raw = take();
        em.h.cmp_raw = take();
        em.h.neg_raw = take();
        em.h.arith_int = take();
        em.h.cmp_f64 = take();
        em.h.persist_alloc = take();
        em.h.persist = take();

        // helper bodies (must precede our set bodies, matching index order).
        emit_helpers(&mut em)?;

        let inst = FunctorInst {
            kind: FunctorKind::Set,
            alias: "xs".to_string(),
            elem: "s32".to_string(),
            iface: "s32-set".to_string(),
        };
        let fns = emit_set_resource(&mut em, &inst, elem, new_i, rep_i, drop_i)?;

        // ---- assemble (mirror emit_core_module's section order, minus the
        // features the set bodies don't use: no closures, globals, value defs).
        let heap_base = {
            em.align8();
            DATA_BASE + em.data.len() as u32
        };
        // A set is a resource, so mirror emit_core_module's resource layout: a
        // persistent reserve below the arena floor, the arena above it.
        let arena_floor = heap_base + PERSIST_RESERVE;
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

        let mut fs = FunctionSection::new();
        for (t, _) in &em.bodies {
            fs.function(*t);
        }
        module.section(&fs);

        let mut ms = MemorySection::new();
        ms.memory(MemoryType {
            minimum: pages,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&ms);

        // Globals, mirroring emit_core_module's resource layout: global 0 = the
        // arena bump pointer (starts above the reserve), 1 = gensym counter,
        // 2 = arena floor, 3 = persistent bump pointer (starts at heap_base).
        let mut gs = GlobalSection::new();
        gs.global(
            GlobalType {
                val_type: I32,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i32_const(arena_floor as i32),
        );
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
        es.export(
            &format!("{IFACE}#[constructor]set"),
            ExportKind::Func,
            fns.ctor,
        );
        es.export(
            &format!("{IFACE}#[method]set.add"),
            ExportKind::Func,
            fns.add,
        );
        es.export(
            &format!("{IFACE}#[method]set.contains"),
            ExportKind::Func,
            fns.contains,
        );
        es.export(
            &format!("{IFACE}#[method]set.size"),
            ExportKind::Func,
            fns.size,
        );
        es.export(&format!("{IFACE}#[dtor]set"), ExportKind::Func, fns.dtor);
        module.section(&es);

        let mut cs = CodeSection::new();
        for (_, f) in &em.bodies {
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

        Ok(module.finish())
    }

    /// Run a core module through the real componentize pipeline.
    fn componentize(elem: &WitTy) -> Result<Vec<u8>, String> {
        let mut module = build_core(elem)?;

        let mut resolve = wit_parser::Resolve::default();
        let pkg = resolve
            .push_str("set.wit", WIT)
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

        if std::env::var("SET_DUMP").is_ok() {
            std::fs::write("/tmp/set_embedded_core.wasm", &module).unwrap();
        }

        wit_component::ComponentEncoder::default()
            .validate(true)
            .module(&module)
            .map_err(|e| format!("componentize: {e:#}"))?
            .encode()
            .map_err(|e| format!("encode: {e:#}"))
    }

    #[test]
    fn set_bodies_dedup_and_membership_s32() {
        let bytes = componentize(&WitTy::IntS(4)).expect("componentize + validate");
        let mut c = HostComponent::from_bytes(&bytes).expect("instantiate");

        // constructor() -> own<set>
        let ctor_out = c
            .call_instance(IFACE, "[constructor]set", &[])
            .expect("constructor call");
        let handle = match &ctor_out[0] {
            Val::Resource(_) => ctor_out[0].clone(),
            other => panic!("ctor should return a resource, got {other:?}"),
        };

        let size = |c: &mut HostComponent, h: &Val| -> u32 {
            match c
                .call_instance(IFACE, "[method]set.size", std::slice::from_ref(h))
                .unwrap()[..]
            {
                [Val::U32(n)] => n,
                ref other => panic!("size returned {other:?}"),
            }
        };
        let contains = |c: &mut HostComponent, h: &Val, v: i32| -> bool {
            match c
                .call_instance(IFACE, "[method]set.contains", &[h.clone(), Val::S32(v)])
                .unwrap()[..]
            {
                [Val::Bool(b)] => b,
                ref other => panic!("contains returned {other:?}"),
            }
        };
        let add = |c: &mut HostComponent, h: &Val, v: i32| {
            c.call_instance(IFACE, "[method]set.add", &[h.clone(), Val::S32(v)])
                .unwrap();
        };

        // fresh set is empty
        assert_eq!(size(&mut c, &handle), 0, "new set is empty");
        assert!(!contains(&mut c, &handle, 7), "empty set contains nothing");

        // add 7, 42, 7 (duplicate) → dedup keeps size 2
        add(&mut c, &handle, 7);
        add(&mut c, &handle, 42);
        add(&mut c, &handle, 7); // duplicate: must NOT grow the set
        assert_eq!(
            size(&mut c, &handle),
            2,
            "duplicate add is deduped by eq_raw"
        );

        // membership is exact
        assert!(contains(&mut c, &handle, 7), "7 is present");
        assert!(contains(&mut c, &handle, 42), "42 is present");
        assert!(!contains(&mut c, &handle, 100), "100 was never added");

        // a third distinct element grows the set; identity persists across calls
        add(&mut c, &handle, 100);
        assert_eq!(size(&mut c, &handle), 3, "distinct add grows the set");
        assert!(contains(&mut c, &handle, 100), "100 now present");

        // dropping the handle runs the no-op dtor cleanly.
        c.drop_resource(handle).expect("drop runs the no-op dtor");
    }
}
