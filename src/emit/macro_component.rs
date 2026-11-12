use super::*;

pub(crate) struct MacroDef {
    name: String,
    params: Vec<String>,
    body: NodeId,
}

pub(crate) fn macro_component_wit() -> String {
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

pub(crate) fn meta_node_wit_ty() -> WitTy {
    let nid = WitTy::IntU(4);
    WitTy::Variant(vec![
        ("bool-val".into(), Some(WitTy::Bool)),
        ("int-val".into(), Some(WitTy::S64)),
        ("dec-val".into(), Some(WitTy::F64)),
        ("char-val".into(), Some(WitTy::Char)),
        ("str-val".into(), Some(WitTy::Str)),
        ("sym".into(), Some(WitTy::Str)),
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

pub fn emit_macro_component(arena: &Arena, roots: &[NodeId]) -> Result<Vec<u8>, String> {
    emit_macro_component_with_optimizer(arena, roots, &mut Ok)
}

pub fn emit_macro_component_with_optimizer(
    arena: &Arena,
    roots: &[NodeId],
    optimizer: &mut dyn CoreOptimizer,
) -> Result<Vec<u8>, String> {
    component::encode(
        emit_macro_core_module(arena, roots)?,
        &macro_component_wit(),
        "macro-lib",
        optimizer,
    )
}

pub(crate) fn emit_macro_core_module(arena: &Arena, roots: &[NodeId]) -> Result<Vec<u8>, String> {
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

    let mut em = Emitter::new(arena, &info, &deps);
    em.macro_expand_idx = Some(0);
    let mut next = 0u32;
    let mut take = || {
        let i = next;
        next += 1;
        i
    };
    em.h = Helpers::reserve(&mut take);
    emit_helpers(&mut em)?;

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

    em.macro_expand_idx = Some(expand_step_idx);

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

    Ok(serialize_module(&mut em, &exports))
}

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

    for (c, tag) in [("tup", TAG_TUP), ("lst", TAG_LIST)] {
        let s = em.intern_str(c) as i32;
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

        fx.op(I::LocalGet(payload));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::Call(em.h.unbox_int));
        fx.op(I::I32WrapI64);
        fx.op(I::LocalSet(cid));

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

        fx.op(I::LocalGet(payload));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(4));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::LocalSet(tup));

        fx.op(I::LocalGet(out));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::LocalGet(tup));
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::I32Store(ma(8, 2)));

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

    fx.op(I::LocalGet(formbox));
    fx.op(I::I32Eqz);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::Unreachable);
    fx.op(I::End);

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

    fx.op(I::LocalGet(idx));
    fx.op(I::LocalGet(root_id));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(0, 2)));
    let t = em.ty_idx(vec![I32], vec![I32]);
    Ok((t, fx.finish()))
}

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

    fx.op(I::LocalGet(tag));
    fx.op(I::I32Const(TAG_FN));
    fx.op(I::I32Eq);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::Unreachable);
    fx.op(I::End);

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

pub(crate) fn mc_sym_node(em: &mut Emitter) -> Result<(u32, Function), String> {
    use ValType::I32;
    let mut fx = FnCtx::new(1);
    let node = fx.local(I32);
    let sym = em.intern_str("sym") as i32;
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
    fx.op(I::LocalGet(0));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(node));
    let ty = em.ty_idx(vec![I32], vec![I32]);
    Ok((ty, fx.finish()))
}

pub(crate) fn mc_fill(
    em: &mut Emitter,
    fill_idx: u32,
    sym_node_idx: u32,
) -> Result<(u32, Function), String> {
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

    {
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(TAG_VAR));
        fx.op(I::I32Eq);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::LocalGet(form));
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::If(BlockType::Empty));

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

    for (tagk, case) in [(TAG_TUP, "tup"), (TAG_LIST, "lst")] {
        let caddr = em.intern_str(case) as i32;
        fx.op(I::LocalGet(tag));
        fx.op(I::I32Const(tagk));
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

        fx.op(I::I32Const(16));
        fx.op(I::Call(em.h.alloc));
        fx.op(I::LocalSet(tupb));
        fx.op(I::LocalGet(tupb));
        fx.op(I::I32Const(TAG_TUP));
        fx.op(I::I32Store(ma(0, 2)));
        fx.op(I::LocalGet(tupb));
        fx.op(I::I32Const(2));
        fx.op(I::I32Store(ma(4, 2)));

        fx.op(I::LocalGet(tupb));
        fx.op(I::LocalGet(form));
        fx.op(I::LocalGet(e));
        fx.op(I::I32Const(8));
        fx.op(I::I32Mul);
        fx.op(I::I32Add);
        fx.op(I::I32Load(ma(8, 2)));
        fx.op(I::I32Store(ma(8, 2)));

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

    fx.op(I::Unreachable);
    let t = em.ty_idx(vec![I32, I32, I32], vec![I32]);
    Ok((t, fx.finish()))
}

pub(crate) fn store_node(fx: &mut FnCtx, nodes: u32, id: u32, node: u32) {
    fx.op(I::LocalGet(nodes));
    fx.op(I::LocalGet(id));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::LocalGet(node));
    fx.op(I::I32Store(ma(8, 2)));
}

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

    fx.op(I::I32Const(4));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(cur));
    fx.op(I::LocalGet(cur));
    fx.op(I::I32Const(0));
    fx.op(I::I32Store(ma(0, 2)));

    fx.op(I::LocalGet(form));
    fx.op(I::LocalGet(nodes));
    fx.op(I::LocalGet(cur));
    fx.op(I::Call(fill_idx));
    fx.op(I::LocalSet(root));

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

pub(crate) fn mc_manifest(
    em: &mut Emitter,
    macros: &[MacroDef],
) -> Result<(u32, Function), String> {
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

    em.lift_flat(&mut fx, &WitTy::Str, 0)?;
    let name_box = fx.local(I32);
    fx.op(I::LocalSet(name_box));
    em.lift_flat(&mut fx, &tree_ty, args_base)?;
    let args_box = fx.local(I32);
    fx.op(I::LocalSet(args_box));

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

    let area = fx.local(I32);
    fx.op(I::I32Const(size_of(&result_ty) as i32));
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(area));
    em.store_to_mem(&mut fx, &result_ty, res, area, 0)?;
    fx.op(I::LocalGet(area));
    let t = em.ty_idx(fparams, vec![I32]);
    Ok((t, fx.finish()))
}

pub(crate) fn mc_expand_step(
    em: &mut Emitter,
    macros: &[MacroDef],
) -> Result<(u32, Function), String> {
    use ValType::I32;
    let mut fx = FnCtx::new(1);
    let form = 0u32;
    let head = fx.local(I32);
    let nargs = fx.local(I32);
    let tupb = fx.local(I32);
    let e = fx.local(I32);

    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(0, 2)));
    fx.op(I::I32Const(TAG_TUP));
    fx.op(I::I32Ne);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(form));
    fx.op(I::Return);
    fx.op(I::End);

    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::I32Eqz);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::LocalGet(form));
    fx.op(I::Return);
    fx.op(I::End);

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

    fx.op(I::LocalGet(form));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::I32Const(1));
    fx.op(I::I32Sub);
    fx.op(I::LocalSet(nargs));

    for m in macros {
        let name_macro = em.intern_str(&format!("{}-MACRO", m.name)) as i32;
        let arity = m.params.len();
        let fidx = em.funcs[&m.name].0;

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
            fx.op(I::LocalGet(form));
            fx.op(I::Return);
        }
        fx.op(I::End);
        fx.op(I::End);
    }

    fx.op(I::LocalGet(form));
    let t = em.ty_idx(vec![I32], vec![I32]);
    Ok((t, fx.finish()))
}
