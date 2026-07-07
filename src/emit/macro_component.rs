//! The strategy-B macro component (design.md §6.3): `emit_macro_component`
//! compiles a macro library's bodies to wasm, with the `mc_*` tree⇄form
//! adapters the `wavelet:meta/macros` boundary needs.

use super::*;

/// A macro definition collected from a macro-library file: the unsuffixed name,
/// its parameter names (bound to argument *forms*), the body form, and arity.
pub(crate) struct MacroDef {
    name: String,
    params: Vec<String>,
    body: NodeId,
}

/// The WIT for a produced macro component: the `wavelet:macro-guest` world
/// (exporting `wavelet:meta/macros`) plus the canonical `wavelet:meta` package
/// (`code` + `macros`), as a nested package block. Mirrors
/// `tools/macro-guest/wit/{world,deps/wavelet-meta/code}.wit`.
pub(crate) fn macro_component_wit() -> String {
    // Kept in sync with `wit/meta/code.wit` (pinned); the nested form lets a
    // single `push_str` carry both packages, as dep WIT does.
    "package wavelet:macro-guest@0.1.0;\n\
\n\
world macro-lib {\n\
  export wavelet:meta/macros@0.1.0;\n\
}\n\
\n\
package wavelet:meta@0.1.0 {\n\
  interface code {\n\
    type node-id = u32;\n\
    variant node {\n\
      bool-val(bool),\n\
      int-val(s64),\n\
      dec-val(f64),\n\
      char-val(char),\n\
      str-val(string),\n\
      sym(string),\n\
      qsym(tuple<string, string>),\n\
      tup(list<node-id>),\n\
      lst(list<node-id>),\n\
      rec(list<tuple<string, node-id>>),\n\
      flg(list<string>),\n\
    }\n\
    record tree {\n\
      nodes: list<node>,\n\
      root: node-id,\n\
      spans: list<tuple<u32, u32>>,\n\
    }\n\
  }\n\
  interface macros {\n\
    use code.{tree};\n\
    manifest: func() -> list<tuple<string, u32>>;\n\
    expand: func(name: string, args: tree) -> result<tree, string>;\n\
  }\n\
}\n"
    .to_string()
}

/// The `wavelet:meta` `node` variant as a backend [`WitTy`], for lifting an
/// incoming `tree` and lowering an outgoing one through the generic boundary
/// bridge. Mirrors `wit/meta/code.wit` exactly.
pub(crate) fn meta_node_wit_ty() -> WitTy {
    let nid = WitTy::IntU(4); // node-id = u32
    WitTy::Variant(vec![
        ("bool-val".into(), Some(WitTy::Bool)),
        ("int-val".into(), Some(WitTy::S64)),
        ("dec-val".into(), Some(WitTy::F64)),
        ("char-val".into(), Some(WitTy::Char)),
        ("str-val".into(), Some(WitTy::Str)),
        ("sym".into(), Some(WitTy::Str)),
        (
            "qsym".into(),
            Some(WitTy::Tuple(vec![WitTy::Str, WitTy::Str])),
        ),
        ("tup".into(), Some(WitTy::List(Box::new(nid.clone())))),
        ("lst".into(), Some(WitTy::List(Box::new(nid.clone())))),
        (
            "rec".into(),
            Some(WitTy::List(Box::new(WitTy::Tuple(vec![
                WitTy::Str,
                nid.clone(),
            ])))),
        ),
        ("flg".into(), Some(WitTy::List(Box::new(WitTy::Str)))),
    ])
}

/// The `wavelet:meta` `tree` record as a backend [`WitTy`].
pub(crate) fn meta_tree_wit_ty() -> WitTy {
    WitTy::Record(vec![
        ("nodes".into(), WitTy::List(Box::new(meta_node_wit_ty()))),
        ("root".into(), WitTy::IntU(4)),
        (
            "spans".into(),
            WitTy::List(Box::new(WitTy::Tuple(vec![WitTy::IntU(4), WitTy::IntU(4)]))),
        ),
    ])
}

/// Build a `wavelet:meta/macros` component from a macro-library file's forms
/// (design.md §6.3; **strategy B: compile the bodies**). The result is an
/// ordinary compiled component whose `manifest`/`expand` are compiled wasm —
/// no interpreter in the guest. Each macro body compiles like any function
/// (params bound to argument *forms* as boxes); `expand` converts the incoming
/// `tree` to box forms, dispatches to the compiled body, and converts the
/// result form back to a `tree`.
pub fn emit_macro_component(arena: &Arena, roots: &[NodeId]) -> Result<Vec<u8>, String> {
    let mut module = emit_macro_core_module(arena, roots)?;
    let wit = macro_component_wit();

    let mut resolve = wit_parser::Resolve::default();
    let pkg = resolve
        .push_str("wavelet-macro.wit", &wit)
        .map_err(|e| format!("internal: macro WIT did not parse: {e:#}\n--- WIT ---\n{wit}"))?;
    let world = resolve
        .select_world(&[pkg], Some("macro-lib"))
        .map_err(|e| format!("internal: macro world selection failed: {e:#}"))?;
    wit_component::embed_component_metadata(
        &mut module,
        &resolve,
        world,
        wit_component::StringEncoding::UTF8,
    )
    .map_err(|e| format!("embedding macro component metadata failed: {e:#}"))?;

    wit_component::ComponentEncoder::default()
        .validate(true)
        .module(&module)
        .map_err(|e| format!("componentizing the macro library failed: {e:#}"))?
        .encode()
        .map_err(|e| format!("encoding the macro-library component failed: {e:#}"))
}

// ----------------------------------------------- strategy-B macro component
//
// `emit_macro_core_module` builds the core wasm for a `wavelet:meta/macros`
// component whose `manifest`/`expand` are compiled (no interpreter in the
// guest). It mirrors `emit_core_module`'s assembly but is driven by a file's
// `DefMacro`s rather than its `Def`/`Export`s, and adds the compiled
// `tree`⇄`box` adapters the boundary needs.

pub(crate) fn emit_macro_core_module(arena: &Arena, roots: &[NodeId]) -> Result<Vec<u8>, String> {
    use ValType::I32;

    // Collect the file's DefMacros: `Tup[defmacro-MACRO, name, {params}, body]`.
    let mut macros: Vec<MacroDef> = Vec::new();
    for &root in roots {
        let Node::Tup(items) = arena.node(root) else {
            continue;
        };
        if items.len() != 4 {
            continue;
        }
        let Node::Sym(h) = arena.node(items[0]) else {
            continue;
        };
        if h != "defmacro-MACRO" {
            continue;
        }
        let Node::Sym(name) = arena.node(items[1]) else {
            continue;
        };
        let params = param_names(arena, items[2])?;
        macros.push(MacroDef {
            name: name.clone(),
            params,
            body: items[3],
        });
    }

    // A minimal FileInfo: a macro library has no runtime defs/exports of its
    // own. `gensym` keys its counter global off `value_defs.len()` (here 0).
    let info = FileInfo {
        package: "wavelet:macro-guest@0.1.0".to_string(),
        package_path: "wavelet:macro-guest".to_string(),
        world: "macro-lib".to_string(),
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
        arena,
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

    // static boxes: false @16, true @24
    em.false_addr = DATA_BASE;
    em.put_i32(TAG_BOOL);
    em.put_i32(0);
    em.true_addr = DATA_BASE + 8;
    em.put_i32(TAG_BOOL);
    em.put_i32(1);

    // helper indices (no imports, so function index space starts at 0)
    let mut next = 0u32;
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
    emit_helpers(&mut em)?;

    // macro body functions (each compiles like a Fn over its param forms)
    for m in &macros {
        let idx = take();
        em.funcs.insert(
            m.name.clone(),
            (idx, m.params.clone(), FnSig::boxed(m.params.len())),
        );
    }
    let tree_to_form_idx = take();
    let count_idx = take();
    let sym_node_idx = take();
    let fill_idx = take();
    let form_to_tree_idx = take();
    let manifest_idx = take();
    let expand_idx = take();
    let expand_step_idx = take();
    // Make the in-macro `expand` builtin available while compiling the bodies.
    em.macro_expand_idx = Some(expand_step_idx);

    // bodies, in the same order their indices were assigned
    for m in &macros {
        let body = mc_macro_body(&mut em, m)?;
        em.bodies.push(body);
    }
    let b = mc_tree_to_form(&mut em)?;
    em.bodies.push(b);
    let b = mc_count_nodes(&mut em, count_idx)?;
    em.bodies.push(b);
    let b = mc_sym_node(&mut em)?;
    em.bodies.push(b);
    let b = mc_fill(&mut em, fill_idx, sym_node_idx)?;
    em.bodies.push(b);
    let b = mc_form_to_tree(&mut em, count_idx, fill_idx)?;
    em.bodies.push(b);
    let b = mc_manifest(&mut em, &macros)?;
    em.bodies.push(b);
    let b = mc_expand(&mut em, &macros, tree_to_form_idx, form_to_tree_idx)?;
    em.bodies.push(b);
    let b = mc_expand_step(&mut em, &macros)?;
    em.bodies.push(b);

    let exports: Vec<(String, u32)> = vec![
        (
            "wavelet:meta/macros@0.1.0#manifest".to_string(),
            manifest_idx,
        ),
        ("wavelet:meta/macros@0.1.0#expand".to_string(), expand_idx),
    ];

    // ---- assemble (no imports, no deps)
    let heap_base = {
        em.align8();
        DATA_BASE + em.data.len() as u32
    };
    let pages = (heap_base as u64 >> 16) + 1;
    let closure_base = em.bodies.len() as u32;

    let mut module = Module::new();
    let mut ts = TypeSection::new();
    for (p, r) in &em.types {
        ts.ty().function(p.iter().copied(), r.iter().copied());
    }
    module.section(&ts);

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
        &ConstExpr::i32_const(heap_base as i32),
    );
    // gensym counter (index 1, since there are no value defs)
    gs.global(
        GlobalType {
            val_type: ValType::I64,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i64_const(0),
    );
    // arena floor (index 2) + persistent bump pointer (index 3): a macro library
    // has no resources, so both are the heap base and the persist helpers (which
    // reference them) are dead code.
    gs.global(
        GlobalType {
            val_type: I32,
            mutable: false,
            shared: false,
        },
        &ConstExpr::i32_const(heap_base as i32),
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
    for (name, idx) in &exports {
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

    Ok(module.finish())
}

/// Compile a macro body to an internal `(box…) -> box` function: its parameters
/// bind to the argument *forms* (as boxes), exactly as `expand_once` binds them.
pub(crate) fn mc_macro_body(em: &mut Emitter, m: &MacroDef) -> Result<(u32, Function), String> {
    use ValType::I32;
    let n = m.params.len();
    let mut fx = FnCtx::new(n as u32);
    let mut scope = HashMap::new();
    for (i, p) in m.params.iter().enumerate() {
        scope.insert(p.clone(), Binding::boxed(i as u32));
    }
    fx.scopes.push(scope);
    em.expr(&mut fx, m.body, false)
        .map_err(|e| format!("in macro `{}`: {e}", m.name))?;
    let t = em.ty_idx(vec![I32; n], vec![I32]);
    Ok((t, fx.finish()))
}

/// `tree → box`: convert a lifted wire `tree` record box into the root form box
/// (the compile-time analogue of `meta::tree_to_arena` + `form_to_value`). Walks
/// the node table building a per-node index of form boxes; children precede
/// parents (`meta::arena_to_tree` guarantees it), so a `tup`/`lst`/`rec` node's
/// child ids are already built.
pub(crate) fn mc_tree_to_form(em: &mut Emitter) -> Result<(u32, Function), String> {
    use ValType::I32;
    let mut fx = FnCtx::new(1);
    let args = 0u32;
    let nodes = fx.local(I32);
    let n = fx.local(I32);
    let root_id = fx.local(I32);
    let idx = fx.local(I32);
    let k = fx.local(I32);
    let nodevar = fx.local(I32);
    let case = fx.local(I32);
    let payload = fx.local(I32);
    let formbox = fx.local(I32);
    let cs = fx.local(I32);
    let m = fx.local(I32);
    let e = fx.local(I32);
    let cid = fx.local(I32);
    let out = fx.local(I32);
    let tup = fx.local(I32);

    let nodes_key = em.intern_str("nodes") as i32;
    let root_key = em.intern_str("root") as i32;
    fx.op(I::LocalGet(args));
    fx.op(I::I32Const(nodes_key));
    fx.op(I::Call(em.h.rec_get));
    fx.op(I::LocalSet(nodes));
    fx.op(I::LocalGet(nodes));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(n));
    fx.op(I::LocalGet(args));
    fx.op(I::I32Const(root_key));
    fx.op(I::Call(em.h.rec_get));
    fx.op(I::Call(em.h.unbox_int));
    fx.op(I::I32WrapI64);
    fx.op(I::LocalSet(root_id));
    // idx = alloc((n+1)*4)  (+1 so n==0 never asks for a zero-byte block)
    fx.op(I::LocalGet(n));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(idx));

    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(k));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(k));
    fx.op(I::LocalGet(n));
    fx.op(I::I32GeU);
    fx.op(I::BrIf(1));
    // nodevar = nodes[k]
    fx.op(I::LocalGet(nodes));
    fx.op(I::LocalGet(k));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(8, 2)));
    fx.op(I::LocalSet(nodevar));
    fx.op(I::LocalGet(nodevar));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(case));
    fx.op(I::LocalGet(nodevar));
    fx.op(I::I32Load(ma(8, 2)));
    fx.op(I::LocalSet(payload));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(formbox));

    // scalar cases pass the payload box straight through (a bool/int/dec/str
    // wire payload is already exactly the form box).
    for c in ["bool-val", "int-val", "dec-val", "str-val"] {
        let s = em.intern_str(c) as i32;
        fx.op(I::LocalGet(case));
        fx.op(I::I32Const(s));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(payload));
        fx.op(I::LocalSet(formbox));
        fx.op(I::End);
    }
    // char-val: the wire payload is a `char`, lifted as a TAG_CHAR box already —
    // use it directly.
    {
        let s = em.intern_str("char-val") as i32;
        fx.op(I::LocalGet(case));
        fx.op(I::I32Const(s));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(payload));
        fx.op(I::LocalSet(formbox));
        fx.op(I::End);
    }
    // sym → payload-less TAG_VAR [case=str payload-box, 0]
    {
        let s = em.intern_str("sym") as i32;
        fx.op(I::LocalGet(case));
        fx.op(I::I32Const(s));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::I32Const(12));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(formbox));
        fx.op(I::LocalGet(formbox));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(formbox));
        fx.op(I::LocalGet(payload));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(formbox));
        fx.op(I::I32Const(0));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::End);
    }
    // qsym → payload-less TAG_VAR whose case is "alias/name"
    {
        let s = em.intern_str("qsym") as i32;
        let slash = em.intern_str("/") as i32;
        fx.op(I::LocalGet(case));
        fx.op(I::I32Const(s));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(payload));
        fx.op(I::I32Load(ma(8, 2))); // alias str
        fx.op(I::I32Const(slash));
        fx.op(I::Call(em.h.strcat2));
        fx.op(I::LocalGet(payload));
        fx.op(I::I32Load(ma(12, 2))); // name str
        fx.op(I::Call(em.h.strcat2));
        fx.op(I::LocalSet(cs));
        fx.op(I::I32Const(12));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(formbox));
        fx.op(I::LocalGet(formbox));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(formbox));
        fx.op(I::LocalGet(cs));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(formbox));
        fx.op(I::I32Const(0));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::End);
    }
    // tup / lst → sequence form box of child forms (by id)
    for (c, tag) in [("tup", TAG_TUP), ("lst", TAG_LIST)] {
        let s = em.intern_str(c) as i32;
        fx.op(I::LocalGet(case));
        fx.op(I::I32Const(s));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        // m = payload.len
        fx.op(I::LocalGet(payload));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(m));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(out));
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(tag));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(e));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(e));
        fx.op(I::LocalGet(m));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // cid = unbox(payload[e])
        fx.op(I::LocalGet(payload));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::Call(em.h.unbox_int));
        fx.op(I::I32WrapI64);
        fx.op(I::LocalSet(cid));
        // out[e] = idx[cid]
        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(idx));
        fx.op(I::LocalGet(cid));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(e));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(out));
        fx.op(I::LocalSet(formbox));
        fx.op(I::End);
    }
    // rec → record form box of child value forms (keys carried through)
    {
        let s = em.intern_str("rec") as i32;
        fx.op(I::LocalGet(case));
        fx.op(I::I32Const(s));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(payload));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(m));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(out));
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(TAG_REC));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(e));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(e));
        fx.op(I::LocalGet(m));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // tup = payload[e]  (a TAG_TUP [2, key-str, id-box])
        fx.op(I::LocalGet(payload));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalSet(tup));
        // key → out key slot (8 + 8e)
        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(tup));
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::I32Store(ma(8, 2)));
        // cid = unbox(tup[1]) ; val = idx[cid] → out val slot (12 + 8e)
        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(tup));
        fx.op(I::I32Load(ma(12, 2)));
        fx.op(I::Call(em.h.unbox_int));
        fx.op(I::I32WrapI64);
        fx.op(I::LocalSet(cid));
        fx.op(I::LocalGet(idx));
        fx.op(I::LocalGet(cid));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Store(ma(12, 2)));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(e));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(out));
        fx.op(I::LocalSet(formbox));
        fx.op(I::End);
    }
    // flg → flags form box [TAG_FLG, m, name str boxes…] (payload is a
    // list<string> of names, copied straight through)
    {
        let s = em.intern_str("flg") as i32;
        fx.op(I::LocalGet(case));
        fx.op(I::I32Const(s));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(payload));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(m));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(out));
        fx.op(I::LocalGet(out));
        fx.op(I::I32Const(TAG_FLG));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(e));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(e));
        fx.op(I::LocalGet(m));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(payload));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(e));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        fx.op(I::LocalGet(out));
        fx.op(I::LocalSet(formbox));
        fx.op(I::End);
    }
    // unmatched → trap
    fx.op(I::LocalGet(formbox));
    fx.op(I::I32Eqz);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::Unreachable);
    fx.op(I::End);
    // idx[k] = formbox
    fx.op(I::LocalGet(idx));
    fx.op(I::LocalGet(k));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::LocalGet(formbox));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(k));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(k));
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
    // return idx[root_id]
    fx.op(I::LocalGet(idx));
    fx.op(I::LocalGet(root_id));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(0, 2)));
    let t = em.ty_idx(vec![I32], vec![I32]);
    Ok((t, fx.finish()))
}

/// Count the nodes a form contributes to a wire `tree` (one per node, plus its
/// children). Must agree with `mc_fill`'s assignment exactly.
pub(crate) fn mc_count_nodes(em: &mut Emitter, count_idx: u32) -> Result<(u32, Function), String> {
    use ValType::I32;
    let mut fx = FnCtx::new(1);
    let form = 0u32;
    let tag = fx.local(I32);
    let total = fx.local(I32);
    let m = fx.local(I32);
    let e = fx.local(I32);
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(0, 2)));
    fx.op(I::LocalSet(tag));
    fx.op(I::I32Const(1));
    fx.op(I::LocalSet(total));
    // TAG_FN, or a payloaded TAG_VAR, cannot appear in code → trap (kept in sync
    // with mc_fill).
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_FN));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::Unreachable);
    fx.op(I::End);
    // A payloaded TAG_VAR `name(p)` becomes a `tup[sym(name), p]` (2 nodes plus
    // the payload's subtree); a payload-less one is a single sym/qsym node.
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_VAR));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(8, 2)));
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(total));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(8, 2)));
    fx.op(I::Call(count_idx));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(total));
    fx.op(I::End);
    fx.op(I::End);
    // tup / lst: children at [form + 8 + 4e]
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_TUP));
    fx.op(I::I32Eq);
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_LIST));
    fx.op(I::I32Eq);
    fx.op(I::I32Or);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(m));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(e));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(e));
    fx.op(I::LocalGet(m));
    fx.op(I::I32GeU);
    fx.op(I::BrIf(1));
    fx.op(I::LocalGet(total));
    fx.op(I::LocalGet(form));
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(8, 2)));
    fx.op(I::Call(count_idx));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(total));
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(e));
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::End);
    // rec: values at [form + 12 + 8e]
    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_REC));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(m));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(e));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(e));
    fx.op(I::LocalGet(m));
    fx.op(I::I32GeU);
    fx.op(I::BrIf(1));
    fx.op(I::LocalGet(total));
    fx.op(I::LocalGet(form));
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(8));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(12, 2)));
    fx.op(I::Call(count_idx));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(total));
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(e));
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::End);
    fx.op(I::LocalGet(total));
    let t = em.ty_idx(vec![I32], vec![I32]);
    Ok((t, fx.finish()))
}

/// Build the wire node box for a symbol whose name is the string box `case`:
/// a `sym(name)` node, or — when the name contains a `/` — a `qsym((alias,
/// name))` node (mirroring `value::sym_node`, which splits a `Variant` case on
/// `/`). Signature `(case-str) -> node-box`.
pub(crate) fn mc_sym_node(em: &mut Emitter) -> Result<(u32, Function), String> {
    use ValType::I32;
    let mut fx = FnCtx::new(1);
    let case = 0u32;
    let len = fx.local(I32);
    let i = fx.local(I32);
    let slash = fx.local(I32);
    let node = fx.local(I32);
    let alias = fx.local(I32);
    let name = fx.local(I32);
    let tup = fx.local(I32);
    let start = fx.local(I32);
    let sublen = fx.local(I32);
    let j = fx.local(I32);

    fx.op(I::LocalGet(case));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(len));
    fx.op(I::I32Const(-1));
    fx.op(I::LocalSet(slash));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(i));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(i));
    fx.op(I::LocalGet(len));
    fx.op(I::I32GeU);
    fx.op(I::BrIf(1));
    fx.op(I::LocalGet(case));
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::LocalGet(i));
    fx.op(I::I32Add);
    fx.op(I::I32Load8U(ma(0, 0)));
    fx.op(I::I32Const('/' as i32));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(i));
    fx.op(I::LocalSet(slash));
    fx.op(I::Br(2)); // first '/' found → exit the scan
    fx.op(I::End);
    fx.op(I::LocalGet(i));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(i));
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);

    let sym = em.intern_str("sym") as i32;
    let qsym = em.intern_str("qsym") as i32;
    fx.op(I::LocalGet(slash));
    fx.op(I::I32Const(-1));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Result(I32)));
    // sym(name): payload is the whole case string box
    fx.op(I::I32Const(12));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(node));
    fx.op(I::LocalGet(node));
    fx.op(I::I32Const(TAG_VAR));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(node));
    fx.op(I::I32Const(sym));
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::LocalGet(node));
    fx.op(I::LocalGet(case));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(node));
    fx.op(I::Else);
    // qsym((alias, name)): split at the slash
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(start));
    fx.op(I::LocalGet(slash));
    fx.op(I::LocalSet(sublen));
    emit_substr(em, &mut fx, case, start, sublen, alias, j);
    fx.op(I::LocalGet(slash));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(start));
    fx.op(I::LocalGet(len));
    fx.op(I::LocalGet(slash));
    fx.op(I::I32Sub);
    fx.op(I::I32Const(1));
    fx.op(I::I32Sub);
    fx.op(I::LocalSet(sublen));
    emit_substr(em, &mut fx, case, start, sublen, name, j);
    // tup = [TAG_TUP, 2, alias, name]
    fx.op(I::I32Const(16));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(tup));
    fx.op(I::LocalGet(tup));
    fx.op(I::I32Const(TAG_TUP));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(tup));
    fx.op(I::I32Const(2));
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::LocalGet(tup));
    fx.op(I::LocalGet(alias));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(tup));
    fx.op(I::LocalGet(name));
    fx.op(I::I32Store(ma(12, 2)));
    // node = [TAG_VAR, "qsym", tup]
    fx.op(I::I32Const(12));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(node));
    fx.op(I::LocalGet(node));
    fx.op(I::I32Const(TAG_VAR));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(node));
    fx.op(I::I32Const(qsym));
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::LocalGet(node));
    fx.op(I::LocalGet(tup));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(node));
    fx.op(I::End);
    let t = em.ty_idx(vec![I32], vec![I32]);
    Ok((t, fx.finish()))
}

/// `box → wire`, recursively: emit `form`'s subtree into `nodes` (a list box,
/// elements at +8) using a post-order id cursor (`cur`, a 4-byte cell), and
/// return this form's assigned node id. Children are emitted before parents.
pub(crate) fn mc_fill(em: &mut Emitter, fill_idx: u32, sym_node_idx: u32) -> Result<(u32, Function), String> {
    use ValType::I32;
    let mut fx = FnCtx::new(3);
    let form = 0u32;
    let nodes = 1u32;
    let cur = 2u32;
    let tag = fx.local(I32);
    let id = fx.local(I32);
    let headid = fx.local(I32);
    let pid = fx.local(I32);
    let m = fx.local(I32);
    let e = fx.local(I32);
    let plist = fx.local(I32);
    let node = fx.local(I32);
    let tupb = fx.local(I32);

    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(0, 2)));
    fx.op(I::LocalSet(tag));

    // helper to assign & advance the cursor into `id`
    let bump = |fx: &mut FnCtx| {
        fx.op(I::LocalGet(cur));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::LocalSet(id));
        fx.op(I::LocalGet(cur));
        fx.op(I::LocalGet(id));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::I32Store(ma(0, 2)));
    };

    // Build `node` = [TAG_VAR, case-str, payload] then store at nodes[id].
    // Scalars: bool/int/dec/str → wire case with the form box as payload.
    for (tagk, case) in [
        (TAG_BOOL, "bool-val"),
        (TAG_INT, "int-val"),
        (TAG_DEC, "dec-val"),
        (TAG_STR, "str-val"),
    ] {
        let caddr = em.intern_str(case) as i32;
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(tagk));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        bump(&mut fx);
        fx.op(I::I32Const(12));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(node));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(caddr));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::LocalGet(form));
        fx.op(I::I32Store(ma(8, 2)));
        store_node(&mut fx, nodes, id, node);
        fx.op(I::LocalGet(id));
        fx.op(I::Return);
        fx.op(I::End);
    }
    // char (TAG_CHAR): wire `char-val`, payload = the char box itself, which
    // the boundary's `lower(char)` (= unbox_char) reads.
    {
        let caddr = em.intern_str("char-val") as i32;
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_CHAR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        bump(&mut fx);
        fx.op(I::I32Const(12));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(node));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(caddr));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::LocalGet(form));
        fx.op(I::I32Store(ma(8, 2)));
        store_node(&mut fx, nodes, id, node);
        fx.op(I::LocalGet(id));
        fx.op(I::Return);
        fx.op(I::End);
    }
    // TAG_VAR: a payload-less variant is a symbol (`sym`/`qsym` via mc_sym_node);
    // a payloaded variant `name(p)` mirrors `value_to_form` as a 1-argument call
    // `tup[sym(name), p]`.
    {
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(form));
        fx.op(I::I32Load(ma(8, 2))); // the variant's payload (0 if none)
        fx.op(I::If(BlockType::Empty));
        // payloaded → tup[ sym-node(name), fill(payload) ]
        bump(&mut fx);
        fx.op(I::LocalGet(id));
        fx.op(I::LocalSet(headid));
        fx.op(I::LocalGet(form));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::Call(sym_node_idx));
        fx.op(I::LocalSet(node));
        store_node(&mut fx, nodes, headid, node);
        fx.op(I::LocalGet(form));
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalGet(nodes));
        fx.op(I::LocalGet(cur));
        fx.op(I::Call(fill_idx));
        fx.op(I::LocalSet(pid));
        bump(&mut fx);
        // plist = [box_int(headid), box_int(pid)]
        fx.op(I::I32Const(16));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(plist));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Const(2));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(plist));
        fx.op(I::LocalGet(headid));
        fx.op(I::I64ExtendI32U);
        fx.op(I::Call(em.h.box_int));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(plist));
        fx.op(I::LocalGet(pid));
        fx.op(I::I64ExtendI32U);
        fx.op(I::Call(em.h.box_int));
        fx.op(I::I32Store(ma(12, 2)));
        fx.op(I::I32Const(12));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(node));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(em.intern_str("tup") as i32));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Store(ma(8, 2)));
        store_node(&mut fx, nodes, id, node);
        fx.op(I::LocalGet(id));
        fx.op(I::Return);
        fx.op(I::Else);
        // payload-less → sym / qsym
        bump(&mut fx);
        fx.op(I::LocalGet(form));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::Call(sym_node_idx));
        fx.op(I::LocalSet(node));
        store_node(&mut fx, nodes, id, node);
        fx.op(I::LocalGet(id));
        fx.op(I::Return);
        fx.op(I::End);
        fx.op(I::End);
    }
    // tup / lst: fill children first, then a wire node whose payload is the
    // list of child ids (as int boxes).
    for (tagk, case) in [(TAG_TUP, "tup"), (TAG_LIST, "lst")] {
        let caddr = em.intern_str(case) as i32;
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(tagk));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(form));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(m));
        // kids = int-box list of child ids
        fx.op(I::LocalGet(m));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(plist));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(plist));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(e));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(e));
        fx.op(I::LocalGet(m));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // plist[e] = box_int(fill(form[e]))
        fx.op(I::LocalGet(plist));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(form));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalGet(nodes));
        fx.op(I::LocalGet(cur));
        fx.op(I::Call(fill_idx));
        fx.op(I::I64ExtendI32U);
        fx.op(I::Call(em.h.box_int));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(e));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        bump(&mut fx);
        fx.op(I::I32Const(12));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(node));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(caddr));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Store(ma(8, 2)));
        store_node(&mut fx, nodes, id, node);
        fx.op(I::LocalGet(id));
        fx.op(I::Return);
        fx.op(I::End);
    }
    // rec: payload = list of (key-str, child-id) tuples
    {
        let caddr = em.intern_str("rec") as i32;
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_REC));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(form));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(m));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(plist));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(plist));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(e));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(e));
        fx.op(I::LocalGet(m));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        // tupb = [TAG_TUP, 2, key, box_int(fill(value))]
        fx.op(I::I32Const(16));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(tupb));
        fx.op(I::LocalGet(tupb));
        fx.op(I::I32Const(TAG_TUP));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(tupb));
        fx.op(I::I32Const(2));
        fx.op(I::I32Store(ma(4, 2)));
        // key at [form + 8 + 8e]
        fx.op(I::LocalGet(tupb));
        fx.op(I::LocalGet(form));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::I32Store(ma(8, 2)));
        // value id box at tupb[12]
        fx.op(I::LocalGet(tupb));
        fx.op(I::LocalGet(form));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(12, 2)));
        fx.op(I::LocalGet(nodes));
        fx.op(I::LocalGet(cur));
        fx.op(I::Call(fill_idx));
        fx.op(I::I64ExtendI32U);
        fx.op(I::Call(em.h.box_int));
        fx.op(I::I32Store(ma(12, 2)));
        // plist[e] = tupb
        fx.op(I::LocalGet(plist));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(tupb));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(e));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        bump(&mut fx);
        fx.op(I::I32Const(12));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(node));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(caddr));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Store(ma(8, 2)));
        store_node(&mut fx, nodes, id, node);
        fx.op(I::LocalGet(id));
        fx.op(I::Return);
        fx.op(I::End);
    }
    // flg: a leaf wire node whose payload is the list<string> of names.
    {
        let caddr = em.intern_str("flg") as i32;
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_FLG));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(form));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::LocalSet(m));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Const(8));
        fx.op(I::I32Add);
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(plist));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Const(TAG_LIST));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(plist));
        fx.op(I::LocalGet(m));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::I32Const(0));
        fx.op(I::LocalSet(e));
        fx.op(I::Block(BlockType::Empty));
        fx.op(I::Loop(BlockType::Empty));
        fx.op(I::LocalGet(e));
        fx.op(I::LocalGet(m));
        fx.op(I::I32GeU);
        fx.op(I::BrIf(1));
        fx.op(I::LocalGet(plist));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(form));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(1));
        fx.op(I::I32Add);
        fx.op(I::LocalSet(e));
        fx.op(I::Br(0));
        fx.op(I::End);
        fx.op(I::End);
        bump(&mut fx);
        fx.op(I::I32Const(12));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(node));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::I32Const(caddr));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(node));
        fx.op(I::LocalGet(plist));
        fx.op(I::I32Store(ma(8, 2)));
        store_node(&mut fx, nodes, id, node);
        fx.op(I::LocalGet(id));
        fx.op(I::Return);
        fx.op(I::End);
    }
    // anything else → trap
    fx.op(I::Unreachable);
    let t = em.ty_idx(vec![I32, I32, I32], vec![I32]);
    Ok((t, fx.finish()))
}

/// `nodes[id] = node` (elements at +8 in the list box).
pub(crate) fn store_node(fx: &mut FnCtx, nodes: u32, id: u32, node: u32) {
    fx.op(I::LocalGet(nodes));
    fx.op(I::LocalGet(id));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::LocalGet(node));
    fx.op(I::I32Store(ma(8, 2)));
}

/// `box → tree`: flatten a form box into a wire `tree` record box (the inverse
/// of `mc_tree_to_form`). Sizes the node table via `count_nodes`, then fills it
/// with `fill`.
pub(crate) fn mc_form_to_tree(
    em: &mut Emitter,
    count_idx: u32,
    fill_idx: u32,
) -> Result<(u32, Function), String> {
    use ValType::I32;
    let mut fx = FnCtx::new(1);
    let form = 0u32;
    let count = fx.local(I32);
    let nodes = fx.local(I32);
    let cur = fx.local(I32);
    let root = fx.local(I32);
    let spans = fx.local(I32);
    let zspan = fx.local(I32);
    let e = fx.local(I32);
    let tree = fx.local(I32);

    fx.op(I::LocalGet(form));
    fx.op(I::Call(count_idx));
    fx.op(I::LocalSet(count));
    // nodes list box
    fx.op(I::LocalGet(count));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(nodes));
    fx.op(I::LocalGet(nodes));
    fx.op(I::I32Const(TAG_LIST));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(nodes));
    fx.op(I::LocalGet(count));
    fx.op(I::I32Store(ma(4, 2)));
    // cursor cell
    fx.op(I::I32Const(4));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(cur));
    fx.op(I::LocalGet(cur));
    fx.op(I::I32Const(0));
    fx.op(I::I32Store(ma(0, 2)));
    // root = fill(form, nodes, cur)
    fx.op(I::LocalGet(form));
    fx.op(I::LocalGet(nodes));
    fx.op(I::LocalGet(cur));
    fx.op(I::Call(fill_idx));
    fx.op(I::LocalSet(root));
    // zspan = [TAG_TUP, 2, box_int(0), box_int(0)]
    fx.op(I::I32Const(16));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(zspan));
    fx.op(I::LocalGet(zspan));
    fx.op(I::I32Const(TAG_TUP));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(zspan));
    fx.op(I::I32Const(2));
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::LocalGet(zspan));
    fx.op(I::I64Const(0));
    fx.op(I::Call(em.h.box_int));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(zspan));
    fx.op(I::I64Const(0));
    fx.op(I::Call(em.h.box_int));
    fx.op(I::I32Store(ma(12, 2)));
    // spans list (count copies of zspan)
    fx.op(I::LocalGet(count));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Const(8));
    fx.op(I::I32Add);
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(spans));
    fx.op(I::LocalGet(spans));
    fx.op(I::I32Const(TAG_LIST));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(spans));
    fx.op(I::LocalGet(count));
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(e));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(e));
    fx.op(I::LocalGet(count));
    fx.op(I::I32GeU);
    fx.op(I::BrIf(1));
    fx.op(I::LocalGet(spans));
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::LocalGet(zspan));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(e));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(e));
    fx.op(I::Br(0));
    fx.op(I::End);
    fx.op(I::End);
    // tree record box {nodes, root, spans}
    let k_nodes = em.intern_str("nodes") as i32;
    let k_root = em.intern_str("root") as i32;
    let k_spans = em.intern_str("spans") as i32;
    fx.op(I::I32Const(8 + 8 * 3));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(tree));
    fx.op(I::LocalGet(tree));
    fx.op(I::I32Const(TAG_REC));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(tree));
    fx.op(I::I32Const(3));
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::LocalGet(tree));
    fx.op(I::I32Const(k_nodes));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(tree));
    fx.op(I::LocalGet(nodes));
    fx.op(I::I32Store(ma(12, 2)));
    fx.op(I::LocalGet(tree));
    fx.op(I::I32Const(k_root));
    fx.op(I::I32Store(ma(16, 2)));
    fx.op(I::LocalGet(tree));
    fx.op(I::LocalGet(root));
    fx.op(I::I64ExtendI32U);
    fx.op(I::Call(em.h.box_int));
    fx.op(I::I32Store(ma(20, 2)));
    fx.op(I::LocalGet(tree));
    fx.op(I::I32Const(k_spans));
    fx.op(I::I32Store(ma(24, 2)));
    fx.op(I::LocalGet(tree));
    fx.op(I::LocalGet(spans));
    fx.op(I::I32Store(ma(28, 2)));
    fx.op(I::LocalGet(tree));
    let t = em.ty_idx(vec![I32], vec![I32]);
    Ok((t, fx.finish()))
}

/// `manifest()` → `list<tuple<string, u32>>`: a constant list built from the
/// file's macros, lowered to a parked `(ptr, len)` (the canonical list return).
pub(crate) fn mc_manifest(em: &mut Emitter, macros: &[MacroDef]) -> Result<(u32, Function), String> {
    use ValType::I32;
    let mut fx = FnCtx::new(0);
    let n = macros.len();
    let lst = fx.local(I32);
    fx.op(I::I32Const(8 + 4 * n as i32));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(lst));
    fx.op(I::LocalGet(lst));
    fx.op(I::I32Const(TAG_LIST));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(lst));
    fx.op(I::I32Const(n as i32));
    fx.op(I::I32Store(ma(4, 2)));
    for (i, m) in macros.iter().enumerate() {
        let name_addr = em.intern_str(&m.name) as i32;
        let tup = fx.local(I32);
        fx.op(I::I32Const(16));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(tup));
        fx.op(I::LocalGet(tup));
        fx.op(I::I32Const(TAG_TUP));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(tup));
        fx.op(I::I32Const(2));
        fx.op(I::I32Store(ma(4, 2)));
        fx.op(I::LocalGet(tup));
        fx.op(I::I32Const(name_addr));
        fx.op(I::I32Store(ma(8, 2)));
        fx.op(I::LocalGet(tup));
        fx.op(I::I64Const(m.params.len() as i64));
        fx.op(I::Call(em.h.box_int));
        fx.op(I::I32Store(ma(12, 2)));
        fx.op(I::LocalGet(lst));
        fx.op(I::LocalGet(tup));
        fx.op(I::I32Store(ma(8 + 4 * i as u64, 2)));
    }
    // lower list<tuple<string,u32>> → (ptr,len) parked in an 8-byte area
    let list_ty = WitTy::List(Box::new(WitTy::Tuple(vec![WitTy::Str, WitTy::IntU(4)])));
    let lp = fx.local(I32);
    let ll = fx.local(I32);
    let area = fx.local(I32);
    fx.op(I::LocalGet(lst));
    em.lower(&mut fx, &list_ty)?;
    fx.op(I::LocalSet(ll));
    fx.op(I::LocalSet(lp));
    fx.op(I::I32Const(8));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalTee(area));
    fx.op(I::LocalGet(lp));
    fx.op(I::I32Store(ma(0, 2)));
    fx.op(I::LocalGet(area));
    fx.op(I::LocalGet(ll));
    fx.op(I::I32Store(ma(4, 2)));
    fx.op(I::LocalGet(area));
    let t = em.ty_idx(vec![], vec![I32]);
    Ok((t, fx.finish()))
}

/// `expand(name, args)` → `result<tree, string>`: lift the args tree to forms,
/// dispatch to the named compiled macro body (binding the call's argument
/// forms), convert the result form back to a tree, and lower `result<tree,
/// string>` through a return area.
pub(crate) fn mc_expand(
    em: &mut Emitter,
    macros: &[MacroDef],
    tree_to_form_idx: u32,
    form_to_tree_idx: u32,
) -> Result<(u32, Function), String> {
    use ValType::I32;
    let tree_ty = meta_tree_wit_ty();
    let result_ty = WitTy::Result(Box::new(tree_ty.clone()), Box::new(WitTy::Str));

    let mut fparams: Vec<ValType> = Vec::new();
    fparams.extend_from_slice(&flat_checked(&WitTy::Str)?);
    let args_base = fparams.len() as u32;
    fparams.extend_from_slice(&flat_checked(&tree_ty)?);

    let mut fx = FnCtx::new(fparams.len() as u32);
    // lift name (str) and args (tree) into boxes
    em.lift_flat(&mut fx, &WitTy::Str, 0)?;
    let name_box = fx.local(I32);
    fx.op(I::LocalSet(name_box));
    em.lift_flat(&mut fx, &tree_ty, args_base)?;
    let args_box = fx.local(I32);
    fx.op(I::LocalSet(args_box));
    // call form (the whole call tup) and its argument count
    let call = fx.local(I32);
    fx.op(I::LocalGet(args_box));
    fx.op(I::Call(tree_to_form_idx));
    fx.op(I::LocalSet(call));
    let nargs = fx.local(I32);
    fx.op(I::LocalGet(call));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::I32Const(1));
    fx.op(I::I32Sub);
    fx.op(I::LocalSet(nargs));

    let res = fx.local(I32);
    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(res));
    let tupb = fx.local(I32);
    let e = fx.local(I32);

    for m in macros {
        let name_addr = em.intern_str(&m.name) as i32;
        let arity = m.params.len();
        let fidx = em.funcs[&m.name].0;
        fx.op(I::LocalGet(name_box));
        fx.op(I::I32Const(name_addr));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        // equal-arity fast path
        fx.op(I::LocalGet(nargs));
        fx.op(I::I32Const(arity as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        for j in 1..=arity {
            fx.op(I::LocalGet(call));
            fx.op(I::I32Load(ma(8 + 4 * j as u64, 2)));
        }
        fx.op(I::Call(fidx));
        fx.op(I::Call(form_to_tree_idx));
        em.wrap_variant(&mut fx, "ok");
        fx.op(I::LocalSet(res));
        fx.op(I::Else);
        if arity == 1 {
            // a 1-param macro given several args binds the whole args tuple
            // (`expand_once`'s rule): build TAG_TUP of call[1..].
            fx.op(I::LocalGet(nargs));
            fx.op(I::I32Const(4));
            fx.op(I::I32Mul);
            fx.op(I::I32Const(8));
            fx.op(I::I32Add);
            fx.op(I::Call(em.h.alloc));
            fx.op(I::LocalSet(tupb));
            fx.op(I::LocalGet(tupb));
            fx.op(I::I32Const(TAG_TUP));
            fx.op(I::I32Store(ma(0, 2)));
            fx.op(I::LocalGet(tupb));
            fx.op(I::LocalGet(nargs));
            fx.op(I::I32Store(ma(4, 2)));
            fx.op(I::I32Const(0));
            fx.op(I::LocalSet(e));
            fx.op(I::Block(BlockType::Empty));
            fx.op(I::Loop(BlockType::Empty));
            fx.op(I::LocalGet(e));
            fx.op(I::LocalGet(nargs));
            fx.op(I::I32GeU);
            fx.op(I::BrIf(1));
            // tupb[e] = call[1+e]
            fx.op(I::LocalGet(tupb));
            fx.op(I::LocalGet(e));
            fx.op(I::I32Const(4));
            fx.op(I::I32Mul);
            fx.op(I::I32Add);
            fx.op(I::LocalGet(call));
            fx.op(I::LocalGet(e));
            fx.op(I::I32Const(1));
            fx.op(I::I32Add);
            fx.op(I::I32Const(4));
            fx.op(I::I32Mul);
            fx.op(I::I32Add);
            fx.op(I::I32Load(ma(8, 2)));
            fx.op(I::I32Store(ma(8, 2)));
            fx.op(I::LocalGet(e));
            fx.op(I::I32Const(1));
            fx.op(I::I32Add);
            fx.op(I::LocalSet(e));
            fx.op(I::Br(0));
            fx.op(I::End);
            fx.op(I::End);
            fx.op(I::LocalGet(tupb));
            fx.op(I::Call(fidx));
            fx.op(I::Call(form_to_tree_idx));
            em.wrap_variant(&mut fx, "ok");
            fx.op(I::LocalSet(res));
        } else {
            let msg =
                em.intern_str(&format!("macro `{}` expects {arity} arguments", m.name)) as i32;
            fx.op(I::I32Const(msg));
            em.wrap_variant(&mut fx, "err");
            fx.op(I::LocalSet(res));
        }
        fx.op(I::End);
        fx.op(I::End);
    }
    // unknown macro → err "unknown macro `<name>`"
    fx.op(I::LocalGet(res));
    fx.op(I::I32Eqz);
    fx.op(I::If(BlockType::Empty));
    let pre = em.intern_str("unknown macro `") as i32;
    let post = em.intern_str("`") as i32;
    fx.op(I::I32Const(pre));
    fx.op(I::LocalGet(name_box));
    fx.op(I::Call(em.h.strcat2));
    fx.op(I::I32Const(post));
    fx.op(I::Call(em.h.strcat2));
    em.wrap_variant(&mut fx, "err");
    fx.op(I::LocalSet(res));
    fx.op(I::End);
    // lower result<tree,string> into a return area
    let area = fx.local(I32);
    fx.op(I::I32Const(size_of(&result_ty) as i32));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(area));
    em.store_to_mem(&mut fx, &result_ty, res, area, 0)?;
    fx.op(I::LocalGet(area));
    let t = em.ty_idx(fparams, vec![I32]);
    Ok((t, fx.finish()))
}

/// The guest-internal one-step expander behind the in-macro `expand` builtin
/// (mirrors `builtins.rs` `expand`): given a form, if it is a call `(name-MACRO
/// …)` to one of this library's macros, run that macro's compiled body **once**
/// over the call's argument forms and return the result; otherwise return the
/// form unchanged. Signature `(form) -> form`.
pub(crate) fn mc_expand_step(em: &mut Emitter, macros: &[MacroDef]) -> Result<(u32, Function), String> {
    use ValType::I32;
    let mut fx = FnCtx::new(1);
    let form = 0u32;
    let head = fx.local(I32);
    let nargs = fx.local(I32);
    let tupb = fx.local(I32);
    let e = fx.local(I32);

    // Not a call tuple → unchanged.
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(0, 2)));
    fx.op(I::I32Const(TAG_TUP));
    fx.op(I::I32Ne);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(form));
    fx.op(I::Return);
    fx.op(I::End);
    // Empty tuple → unchanged.
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::I32Eqz);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(form));
    fx.op(I::Return);
    fx.op(I::End);
    // head = element 0; must be a payload-less symbol (TAG_VAR, payload 0).
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(8, 2)));
    fx.op(I::LocalSet(head));
    fx.op(I::LocalGet(head));
    fx.op(I::I32Load(ma(0, 2)));
    fx.op(I::I32Const(TAG_VAR));
    fx.op(I::I32Ne);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(form));
    fx.op(I::Return);
    fx.op(I::End);
    fx.op(I::LocalGet(head));
    fx.op(I::I32Load(ma(8, 2)));
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(form));
    fx.op(I::Return);
    fx.op(I::End);
    // nargs = len - 1
    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::I32Const(1));
    fx.op(I::I32Sub);
    fx.op(I::LocalSet(nargs));

    for m in macros {
        let name_macro = em.intern_str(&format!("{}-MACRO", m.name)) as i32;
        let arity = m.params.len();
        let fidx = em.funcs[&m.name].0;
        // head case string == "<name>-MACRO" ?
        fx.op(I::LocalGet(head));
        fx.op(I::I32Load(ma(4, 2)));
        fx.op(I::I32Const(name_macro));
        fx.op(I::Call(em.h.eq_raw));
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(nargs));
        fx.op(I::I32Const(arity as i32));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        for j in 1..=arity {
            fx.op(I::LocalGet(form));
            fx.op(I::I32Load(ma(8 + 4 * j as u64, 2)));
        }
        fx.op(I::Call(fidx));
        fx.op(I::Return);
        fx.op(I::Else);
        if arity == 1 {
            // a 1-param macro given several args binds the whole args tuple
            fx.op(I::LocalGet(nargs));
            fx.op(I::I32Const(4));
            fx.op(I::I32Mul);
            fx.op(I::I32Const(8));
            fx.op(I::I32Add);
            fx.op(I::Call(em.h.alloc));
            fx.op(I::LocalSet(tupb));
            fx.op(I::LocalGet(tupb));
            fx.op(I::I32Const(TAG_TUP));
            fx.op(I::I32Store(ma(0, 2)));
            fx.op(I::LocalGet(tupb));
            fx.op(I::LocalGet(nargs));
            fx.op(I::I32Store(ma(4, 2)));
            fx.op(I::I32Const(0));
            fx.op(I::LocalSet(e));
            fx.op(I::Block(BlockType::Empty));
            fx.op(I::Loop(BlockType::Empty));
            fx.op(I::LocalGet(e));
            fx.op(I::LocalGet(nargs));
            fx.op(I::I32GeU);
            fx.op(I::BrIf(1));
            fx.op(I::LocalGet(tupb));
            fx.op(I::LocalGet(e));
            fx.op(I::I32Const(4));
            fx.op(I::I32Mul);
            fx.op(I::I32Add);
            fx.op(I::LocalGet(form));
            fx.op(I::LocalGet(e));
            fx.op(I::I32Const(1));
            fx.op(I::I32Add);
            fx.op(I::I32Const(4));
            fx.op(I::I32Mul);
            fx.op(I::I32Add);
            fx.op(I::I32Load(ma(8, 2)));
            fx.op(I::I32Store(ma(8, 2)));
            fx.op(I::LocalGet(e));
            fx.op(I::I32Const(1));
            fx.op(I::I32Add);
            fx.op(I::LocalSet(e));
            fx.op(I::Br(0));
            fx.op(I::End);
            fx.op(I::End);
            fx.op(I::LocalGet(tupb));
            fx.op(I::Call(fidx));
            fx.op(I::Return);
        } else {
            // arity mismatch: leave the form unchanged (rare; the interpreter
            // would raise an eval error here).
            fx.op(I::LocalGet(form));
            fx.op(I::Return);
        }
        fx.op(I::End);
        fx.op(I::End);
    }
    // No matching macro → unchanged.
    fx.op(I::LocalGet(form));
    let t = em.ty_idx(vec![I32], vec![I32]);
    Ok((t, fx.finish()))
}
