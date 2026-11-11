use super::*;

pub(super) struct FunctionOrder {
    pub internal: Vec<String>,
    pub overloaded: Vec<String>,
}

pub(super) fn reserve_functions(
    em: &mut Emitter<'_>,
    roots: &[NodeId],
    user_res_forms: &[ResForm],
    imports: &ResourceImports,
    take: &mut impl FnMut() -> u32,
) -> Result<FunctionOrder, String> {
    let info = em.info;
    let arena = em.arena;
    let functor_intrinsics = &imports.functors;
    em.h = Helpers::reserve(take);

    for (inst, &(new_i, rep_i, drop_i)) in info.functors.iter().zip(functor_intrinsics) {
        let ctor = take();
        let add = take();
        let contains = take();
        let size = take();
        let dtor = take();
        em.functor_fns.insert(
            inst.alias.clone(),
            ResourceFns {
                ctor,
                add,
                contains,
                size,
                dtor,
                new_import: new_i,
                rep_import: rep_i,
                drop_import: drop_i,
            },
        );
    }

    for rf in user_res_forms {
        if let Some((pid, _)) = rf.ctor {
            let names = param_names(arena, pid)?;
            let n = names.len();
            let idx = take();
            em.funcs.insert(
                rf.name.clone(),
                (
                    idx,
                    names,
                    FnSig {
                        params: vec![Repr::Boxed; n],
                        result: Repr::Boxed,
                    },
                ),
            );
        }
        let mut methods = std::collections::HashSet::new();
        for (key, pid, _) in &rf.methods {
            let names = param_names(arena, *pid)?;
            let n = names.len();
            let idx = take();
            em.funcs.insert(
                format!("{}-{}", rf.name, key),
                (
                    idx,
                    names,
                    FnSig {
                        params: vec![Repr::Boxed; n],
                        result: Repr::Boxed,
                    },
                ),
            );
            methods.insert(key.clone());
        }
        let mut statics = std::collections::HashSet::new();
        for (key, pid, _) in &rf.statics {
            let names = param_names(arena, *pid)?;
            let n = names.len();
            let idx = take();
            em.funcs.insert(
                format!("{}-{}", rf.name, key),
                (
                    idx,
                    names,
                    FnSig {
                        params: vec![Repr::Boxed; n],
                        result: Repr::Boxed,
                    },
                ),
            );
            statics.insert(key.clone());
        }
        let dtor = take();
        em.user_res.insert(
            rf.name.clone(),
            UserRes {
                methods,
                statics,
                dtor,
            },
        );
    }

    let mut overload_order: Vec<String> = info.overload_bodies.keys().cloned().collect();
    overload_order.sort();

    let overloaded_origins: std::collections::HashSet<&String> = info
        .fn_defs
        .iter()
        .filter(|(_, members)| {
            members
                .iter()
                .all(|m| info.overload_bodies.values().any(|ob| ob == m))
        })
        .map(|(name, _)| name)
        .collect();

    let mut internal_order: Vec<String> = Vec::new();
    for &root in roots {
        if let Node::Tup(items) = arena.node(root)
            && items.len() >= 2
            && matches!(arena.node(items[0]), Node::Sym(s) if s == "def-MACRO")
            && let Node::Sym(name) = arena.node(items[1])
            && info.defs.contains_key(name)
            && !overloaded_origins.contains(name)
            && !internal_order.contains(name)
        {
            internal_order.push(name.clone());
        }
    }
    for (i, (name, _)) in info.value_defs.iter().enumerate() {
        em.value_globals.insert(name.clone(), 1 + i as u32);
    }
    for name in &internal_order {
        let (params_id, body) = info.defs[name];
        let params = param_names(arena, params_id)?;
        let sig = em.def_sig(params_id, body);
        em.funcs.insert(name.clone(), (take(), params, sig));
    }

    for mangled in &overload_order {
        let (params_id, body) = info.overload_bodies[mangled];
        let params = param_names(arena, params_id)?;
        let sig = em.def_sig(params_id, body);
        em.funcs.insert(mangled.clone(), (take(), params, sig));
    }

    for name in &internal_order {
        let (_, body) = info.defs[name];
        scan_let_lambdas(arena, body, &em.node_types, &mut em.lambda_order);
    }
    for &node in &em.lambda_order.clone() {
        let idx = take();
        em.lambda_reserved.insert(node, idx);
    }

    Ok(FunctionOrder {
        internal: internal_order,
        overloaded: overload_order,
    })
}

pub(super) fn emit_function_bodies(
    em: &mut Emitter<'_>,
    user_res_forms: &[ResForm],
    imports: &ResourceImports,
    order: &FunctionOrder,
) -> Result<(), String> {
    let arena = em.arena;
    let info = em.info;
    let n_imports = em.imports.len() as u32;
    let functor_intrinsics = &imports.functors;
    let internal_order = &order.internal;
    let overload_order = &order.overloaded;
    emit_helpers(em)?;

    for (inst, &(new_i, rep_i, drop_i)) in info.functors.iter().zip(functor_intrinsics) {
        let elem = wit_ty(&inst.elem, &em.type_env).map_err(|e| {
            format!(
                "functor `set` over `{}` (alias `{}`): {e}",
                inst.elem, inst.alias
            )
        })?;
        let fns = emit_set_resource(em, inst, &elem, new_i, rep_i, drop_i)?;
        debug_assert_eq!(
            em.functor_fns.get(&inst.alias).copied(),
            Some(fns),
            "reserved functor `set` indices must match the emitted body positions"
        );
    }

    for rf in user_res_forms {
        let mut member_bodies: Vec<(String, NodeId)> = Vec::new();
        if let Some((_pid, body)) = rf.ctor {
            member_bodies.push((rf.name.clone(), body));
        }
        for (key, _pid, body) in rf.methods.iter().chain(rf.statics.iter()) {
            member_bodies.push((format!("{}-{}", rf.name, key), *body));
        }
        for (name, body) in member_bodies {
            emit_defined_body(em, &name, body)?;
        }
        {
            let mut fx = FnCtx::new(1);
            if let Some((pid, body)) = rf.drop_member {
                let mut scope = HashMap::new();
                if let Some(pn) = param_names(arena, pid)?.first() {
                    scope.insert(pn.clone(), Binding::new(0, Repr::Boxed));
                }
                fx.scopes.push(scope);
                em.expr_repr(&mut fx, body, Repr::Boxed, false)
                    .map_err(|e| format!("in resource `{}` Drop: {e}", rf.name))?;
                fx.op(I::Drop);
            }
            let t = em.ty_idx(vec![ValType::I32], vec![]);
            em.bodies.push((t, fx.finish()));
        }
    }

    for name in internal_order {
        emit_defined_body(em, name, info.defs[name].1)?;
    }
    for name in overload_order {
        emit_defined_body(em, name, info.overload_bodies[name].1)?;
    }

    for node in em.lambda_order.clone() {
        let (t, f) = em
            .lambda_stash
            .remove(&node)
            .ok_or_else(|| format!("internal: lifted lambda body for node {node} missing"))?;
        debug_assert_eq!(
            n_imports + em.bodies.len() as u32,
            em.lambda_reserved[&node],
            "lifted lambda body position must match its reserved index"
        );
        em.bodies.push((t, f));
    }

    Ok(())
}

fn emit_defined_body(em: &mut Emitter<'_>, name: &str, body: NodeId) -> Result<(), String> {
    let (_, params, sig) = em.funcs[name].clone();
    let mut fx = FnCtx::new(params.len() as u32);
    let scope = params
        .iter()
        .enumerate()
        .map(|(i, name)| (name.clone(), Binding::new(i as u32, sig.params[i])))
        .collect();
    fx.scopes.push(scope);
    em.expr_repr(&mut fx, body, sig.result, true)
        .map_err(|e| format!("in `{name}`: {e}"))?;
    let t = em.ty_idx(sig.param_vts(), vec![repr_vt(sig.result)]);
    em.bodies.push((t, fx.finish()));
    Ok(())
}

fn scan_let_lambdas(
    arena: &Arena,
    id: NodeId,
    node_types: &crate::check::NodeTypes,
    out: &mut Vec<NodeId>,
) {
    let is_fn_arrow = |n: NodeId| -> bool {
        matches!(arena.node(n), Node::Tup(items)
            if !items.is_empty()
                && matches!(arena.node(items[0]), Node::Sym(s) if s == "fn-MACRO"))
            && matches!(node_types.get(&n), Some(crate::check::Type::Fn(..)))
    };
    if let Node::Tup(items) = arena.node(id)
        && items.len() >= 3
        && matches!(arena.node(items[0]), Node::Sym(s) if s == "let-MACRO")
        && let Node::Rec(fields) = arena.node(items[1])
    {
        for (_, v) in fields {
            if is_fn_arrow(*v) {
                out.push(*v);
            }
        }
    }
    match arena.node(id) {
        Node::Tup(items) | Node::Lst(items) => {
            for &c in items {
                scan_let_lambdas(arena, c, node_types, out);
            }
        }
        Node::Rec(fields) => {
            for (_, c) in fields {
                scan_let_lambdas(arena, *c, node_types, out);
            }
        }
        _ => {}
    }
}
