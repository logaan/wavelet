use super::*;

pub(super) fn emit_resource_exports(
    em: &mut Emitter<'_>,
    user_res_forms: &[ResForm],
    imports: &ResourceImports,
    exports: &mut Vec<(String, u32)>,
    take: &mut impl FnMut() -> u32,
) -> Result<(), String> {
    use ValType::I32;
    let info = em.info;
    let deps = em.deps;
    let user_res_intrinsics = &imports.resources;
    for rf in user_res_forms {
        let Some(decl) = info.resources.iter().find(|r| r.name == rf.name) else {
            continue;
        };
        let Some(iface_path) = &decl.iface else {
            continue;
        };
        let iface = if is_external_iface(iface_path) {
            external_versioned_in(iface_path, deps)
        } else {
            versioned_iface(&info.package, iface_path)
        };
        let (new_i, _rep_i, _drop_i) = user_res_intrinsics[&rf.name];
        if let Some(ctor_params) = &decl.constructor {
            let ctor_idx = em.funcs[&rf.name].0;
            let mut fparams = Vec::new();
            let mut lifted: Vec<(WitTy, u32)> = Vec::new();
            for (_, t) in ctor_params {
                let ty = wit_ty(t, &em.type_env)?;
                lifted.push((ty.clone(), fparams.len() as u32));
                fparams.extend_from_slice(&flat_checked(&ty)?);
            }
            let mut fx = FnCtx::new(fparams.len() as u32);
            for (ty, base) in &lifted {
                em.lift_flat(&mut fx, ty, *base)?;
            }
            fx.op(I::Call(ctor_idx));
            fx.op(I::Call(new_i));
            let t = em.ty_idx(fparams, vec![I32]);
            em.bodies.push((t, fx.finish()));
            exports.push((format!("{iface}#[constructor]{}", rf.name), take()));
        }
        for (member, is_method) in decl
            .methods
            .iter()
            .map(|member| (member, true))
            .chain(decl.statics.iter().map(|member| (member, false)))
        {
            let name = format!("{}-{}", rf.name, member.name);
            emit_resource_callable(em, member, &name, new_i, is_method)?;
            let role = if is_method { "method" } else { "static" };
            exports.push((
                format!("{iface}#[{role}]{}.{}", rf.name, member.name),
                take(),
            ));
        }
        exports.push((
            format!("{iface}#[dtor]{}", rf.name),
            em.user_res[&rf.name].dtor,
        ));
    }

    for inst in &info.functors {
        let fns = em.functor_fns[&inst.alias];
        let iface = versioned_iface(&info.package, &inst.iface);
        exports.push((format!("{iface}#[constructor]set"), fns.ctor));
        exports.push((format!("{iface}#[method]set.add"), fns.add));
        exports.push((format!("{iface}#[method]set.contains"), fns.contains));
        exports.push((format!("{iface}#[method]set.size"), fns.size));
        exports.push((format!("{iface}#[dtor]set"), fns.dtor));
    }

    Ok(())
}

fn emit_resource_callable(
    em: &mut Emitter<'_>,
    sig: &FuncSig,
    name: &str,
    new_resource: u32,
    is_method: bool,
) -> Result<(), String> {
    use ValType::I32;
    let mut params = if is_method { vec![I32] } else { Vec::new() };
    let mut lifted = Vec::new();
    for (_, text) in &sig.params {
        let ty = wit_ty(text, &em.type_env)?;
        lifted.push((ty.clone(), params.len() as u32));
        params.extend_from_slice(&flat_checked(&ty)?);
    }
    let mut fx = FnCtx::new(params.len() as u32);
    if is_method {
        fx.op(I::LocalGet(0));
    }
    for (ty, base) in lifted {
        em.lift_flat(&mut fx, &ty, base)?;
    }
    fx.op(I::Call(em.funcs[name].0));
    let results = match flat_result(sig, &em.type_env)? {
        FlatRes::None => {
            fx.op(I::Drop);
            Vec::new()
        }
        FlatRes::One(WitTy::Handle) => {
            fx.op(I::Call(new_resource));
            vec![I32]
        }
        FlatRes::One(ty) => {
            em.lower(&mut fx, &ty)?;
            flat(&ty)
        }
        FlatRes::Retptr => {
            let ty = wit_ty(sig.result.as_deref().unwrap(), &em.type_env)?;
            let value = fx.local(I32);
            fx.op(I::LocalSet(value));
            let area = fx.local(I32);
            fx.op(I::I32Const(size_of(&ty) as i32));
            fx.op(I::Call(em.h.alloc));
            fx.op(I::LocalSet(area));
            em.store_to_mem(&mut fx, &ty, value, area, 0)?;
            fx.op(I::LocalGet(area));
            vec![I32]
        }
    };
    let ty = em.ty_idx(params, results);
    em.bodies.push((ty, fx.finish()));
    Ok(())
}
