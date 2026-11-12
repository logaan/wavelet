use super::*;

#[derive(Clone)]
pub(crate) struct UserRes {
    pub(crate) methods: std::collections::HashSet<String>,
    pub(crate) statics: std::collections::HashSet<String>,

    pub(crate) dtor: u32,
}

pub(crate) struct ResForm {
    pub(crate) name: String,
    pub(crate) ctor: Option<(NodeId, NodeId)>,
    pub(crate) methods: Vec<(String, NodeId, NodeId)>,
    pub(crate) statics: Vec<(String, NodeId, NodeId)>,
    pub(crate) drop_member: Option<(NodeId, NodeId)>,
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceFns {
    pub ctor: u32,

    pub add: u32,

    pub contains: u32,

    pub size: u32,

    pub dtor: u32,

    pub new_import: u32,

    pub rep_import: u32,

    pub drop_import: u32,
}

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

pub(crate) fn emit_list_contains(em: &mut Emitter, fx: &mut FnCtx, list: u32, needle: u32) {
    let i = fx.local(ValType::I32);
    let n = fx.local(ValType::I32);

    fx.op(I::LocalGet(list));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(n));

    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(i));

    fx.op(I::Block(BlockType::Result(ValType::I32)));
    fx.op(I::Loop(BlockType::Empty));

    fx.op(I::LocalGet(i));
    fx.op(I::LocalGet(n));
    fx.op(I::I32GeU);
    fx.op(I::If(BlockType::Empty));
    fx.op(I::I32Const(0));
    fx.op(I::Br(2));
    fx.op(I::End);

    fx.op(I::LocalGet(list));
    fx.op(I::LocalGet(i));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::I32Load(ma(8, 2)));

    fx.op(I::LocalGet(needle));
    fx.op(I::Call(em.h.eq_raw));
    fx.op(I::If(BlockType::Empty));
    fx.op(I::I32Const(1));
    fx.op(I::Br(2));
    fx.op(I::End);

    fx.op(I::LocalGet(i));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::LocalSet(i));
    fx.op(I::Br(0));
    fx.op(I::End);

    fx.op(I::I32Const(0));
    fx.op(I::End);
}

pub(crate) fn emit_list_append(em: &mut Emitter, fx: &mut FnCtx, old: u32, extra: u32) {
    let n = fx.local(ValType::I32);
    let new = fx.local(ValType::I32);
    let i = fx.local(ValType::I32);

    fx.op(I::LocalGet(old));
    fx.op(I::I32Load(ma(4, 2)));
    fx.op(I::LocalSet(n));

    fx.op(I::I32Const(8 + 4));
    fx.op(I::LocalGet(n));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::Call(em.h.alloc));
    fx.op(I::LocalSet(new));

    fx.op(I::LocalGet(new));
    fx.op(I::I32Const(TAG_LIST));
    fx.op(I::I32Store(ma(0, 2)));

    fx.op(I::LocalGet(new));
    fx.op(I::LocalGet(n));
    fx.op(I::I32Const(1));
    fx.op(I::I32Add);
    fx.op(I::I32Store(ma(4, 2)));

    fx.op(I::I32Const(0));
    fx.op(I::LocalSet(i));
    fx.op(I::Block(BlockType::Empty));
    fx.op(I::Loop(BlockType::Empty));
    fx.op(I::LocalGet(i));
    fx.op(I::LocalGet(n));
    fx.op(I::I32GeU);
    fx.op(I::BrIf(1));

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
    fx.op(I::End);
    fx.op(I::End);

    fx.op(I::LocalGet(new));
    fx.op(I::LocalGet(n));
    fx.op(I::I32Const(4));
    fx.op(I::I32Mul);
    fx.op(I::I32Add);
    fx.op(I::LocalGet(extra));
    fx.op(I::I32Store(ma(8, 2)));
    fx.op(I::LocalGet(new));
}

pub(crate) fn emit_set_resource(
    em: &mut Emitter,
    _inst: &crate::wit::FunctorInst,
    elem: &WitTy,
    new_import: u32,
    rep_import: u32,
    drop_import: u32,
) -> Result<ResourceFns, String> {
    use ValType::I32;

    let elem_flat = flat_checked(elem)?;
    let n_elem = elem_flat.len() as u32;

    let body_base = em.imports.len() as u32;
    let mut next_idx = body_base + em.bodies.len() as u32;
    let mut alloc_idx = || {
        let i = next_idx;
        next_idx += 1;
        i
    };

    let ctor = alloc_idx();
    {
        let mut fx = FnCtx::new(0);
        let cell = fx.local(I32);

        fx.op(I::I32Const(4));
        fx.op(I::Call(em.h.persist_alloc));
        fx.op(I::LocalSet(cell));

        fx.op(I::LocalGet(cell));
        emit_empty_list_box(em, &mut fx);
        fx.op(I::Call(em.h.persist));
        fx.op(I::I32Store(ma(0, 2)));

        fx.op(I::LocalGet(cell));
        fx.op(I::Call(new_import));
        let t = em.ty_idx(vec![], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    let add = alloc_idx();
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

        emit_list_contains(em, &mut fx, list, needle);
        fx.op(I::If(BlockType::Empty));
        fx.op(I::Return);
        fx.op(I::End);

        fx.op(I::LocalGet(0));
        emit_list_append(em, &mut fx, list, needle);
        fx.op(I::Call(em.h.persist));
        fx.op(I::I32Store(ma(0, 2)));
        let _ = n_elem;
        let t = em.ty_idx(params, vec![]);
        em.bodies.push((t, fx.finish()));
    }

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
        emit_list_contains(em, &mut fx, list, needle);
        let t = em.ty_idx(params, vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

    let size = alloc_idx();
    {
        let mut fx = FnCtx::new(1);

        fx.op(I::LocalGet(0));
        fx.op(I::I32Load(ma(0, 2)));
        fx.op(I::I32Load(ma(4, 2)));
        let t = em.ty_idx(vec![I32], vec![I32]);
        em.bodies.push((t, fx.finish()));
    }

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

    fn build_core(elem: &WitTy) -> Result<Vec<u8>, String> {
        use ValType::I32;

        let arena = Arena::new();
        let info = FileInfo {
            package: "demo:app@0.1.0".to_string(),
            package_path: "demo:app".to_string(),
            world: "app".to_string(),
            imports: Vec::new(),
            functors: vec![FunctorInst {
                kind: FunctorKind::Set,
                alias: "xs".to_string(),
                elem: "s32".to_string(),
                iface: "s32-set".to_string(),
            }],
            exports: Vec::new(),
            types: Vec::new(),
            defs: HashMap::new(),
            fn_defs: HashMap::new(),
            value_defs: Vec::new(),
            overload_bodies: HashMap::new(),
            resources: Vec::new(),
        };
        let deps: HashMap<String, Dep> = HashMap::new();

        let mut em = Emitter::new(&arena, &info, &deps);

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

        let mut next = n_imports;
        let mut take = || {
            let i = next;
            next += 1;
            i
        };
        em.h = Helpers::reserve(&mut take);

        emit_helpers(&mut em)?;

        let fns = emit_set_resource(&mut em, &info.functors[0], elem, new_i, rep_i, drop_i)?;
        let exports = [
            (format!("{IFACE}#[constructor]set"), fns.ctor),
            (format!("{IFACE}#[method]set.add"), fns.add),
            (format!("{IFACE}#[method]set.contains"), fns.contains),
            (format!("{IFACE}#[method]set.size"), fns.size),
            (format!("{IFACE}#[dtor]set"), fns.dtor),
        ];
        Ok(serialize_module(&mut em, &exports))
    }

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

        assert_eq!(size(&mut c, &handle), 0, "new set is empty");
        assert!(!contains(&mut c, &handle, 7), "empty set contains nothing");

        add(&mut c, &handle, 7);
        add(&mut c, &handle, 42);
        add(&mut c, &handle, 7);
        assert_eq!(
            size(&mut c, &handle),
            2,
            "duplicate add is deduped by eq_raw"
        );

        assert!(contains(&mut c, &handle, 7), "7 is present");
        assert!(contains(&mut c, &handle, 42), "42 is present");
        assert!(!contains(&mut c, &handle, 100), "100 was never added");

        add(&mut c, &handle, 100);
        assert_eq!(size(&mut c, &handle), 3, "distinct add grows the set");
        assert!(contains(&mut c, &handle, 100), "100 now present");

        c.drop_resource(handle).expect("drop runs the no-op dtor");
    }
}
