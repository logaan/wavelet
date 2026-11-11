use super::*;

pub(super) fn emit_function_exports(
    em: &mut Emitter<'_>,
    imports: &ResourceImports,
    take: &mut impl FnMut() -> u32,
) -> Result<Vec<(String, u32)>, String> {
    let info = em.info;
    let mut exports = Vec::new();
    let mut post_returns = Vec::new();
    for sig in &info.exports {
        let (params, mut fx, representation) = prepare_export_call(em, sig, imports)?;
        let results = lower_export_result(em, &mut fx, sig, representation, imports)?;
        let ty = em.ty_idx(params, results.clone());
        em.bodies.push((ty, fx.finish()));
        let interface = if is_external_iface(&sig.iface) {
            external_versioned_in(&sig.iface, em.deps)
        } else {
            versioned_iface(&info.package, &sig.iface)
        };
        let name = format!("{interface}#{}", sig.name);
        post_returns.push((name.clone(), results));
        exports.push((name, take()));
    }
    let arena_floor_g = 2 + info.value_defs.len() as u32;
    for (name, fresults) in post_returns {
        let mut fx = FnCtx::new(fresults.len() as u32);
        fx.op(I::GlobalGet(arena_floor_g));
        fx.op(I::GlobalSet(0));
        for i in 0..info.value_defs.len() {
            fx.op(I::I32Const(0));
            fx.op(I::GlobalSet(1 + i as u32));
        }
        let t = em.ty_idx(fresults, vec![]);
        em.bodies.push((t, fx.finish()));
        exports.push((format!("cabi_post_{name}"), take()));
    }

    Ok(exports)
}

fn classify_resource(imports: &ResourceImports, t: &str) -> Option<(String, bool)> {
    let user_res_intrinsics = &imports.resources;
    if let Some(inner) = t.strip_prefix("borrow<").and_then(|x| x.strip_suffix('>'))
        && user_res_intrinsics.contains_key(inner)
    {
        return Some((inner.to_string(), true));
    }
    let bare = t
        .strip_prefix("own<")
        .and_then(|x| x.strip_suffix('>'))
        .unwrap_or(t);
    if user_res_intrinsics.contains_key(bare) {
        return Some((bare.to_string(), false));
    }
    None
}

fn prepare_export_call(
    em: &mut Emitter<'_>,
    sig: &FuncSig,
    imports: &ResourceImports,
) -> Result<(Vec<ValType>, FnCtx, Repr), String> {
    let user_res_intrinsics = &imports.resources;
    let (fidx, _, fsig) = em
        .funcs
        .get(&sig.name)
        .cloned()
        .ok_or(format!("export `{}` has no Def Fn", sig.name))?;
    let mut fparams = Vec::new();
    let mut lifted: Vec<(WitTy, u32, Option<(String, bool)>)> = Vec::new();
    for (_, t) in &sig.params {
        let ty = wit_ty(t, &em.type_env)?;
        let res = classify_resource(imports, t);
        lifted.push((ty.clone(), fparams.len() as u32, res));
        fparams.extend_from_slice(&flat_checked(&ty)?);
    }
    if fparams.len() > 16 {
        return Err(format!(
            "`{}` flattens to {} parameters; spilling >16 params to memory \
                 is not supported by the wasm backend yet",
            sig.name,
            fparams.len()
        ));
    }
    let mut fx = FnCtx::new(fparams.len() as u32);

    let typed_ok = fsig.params.len() == lifted.len();
    for (i, (ty, base, res)) in lifted.iter().enumerate() {
        if let Some((rname, is_borrow)) = res {
            fx.op(I::LocalGet(*base));
            if !is_borrow {
                let (_n, rep_i, _d) = user_res_intrinsics[rname];
                fx.op(I::Call(rep_i));
            }
            continue;
        }
        let slot = if typed_ok {
            fsig.params[i]
        } else {
            Repr::Boxed
        };
        match (slot, wit_scalar(ty)) {
            (Repr::Scalar(k), Some(wk)) if k == wk => {
                fx.op(I::LocalGet(*base));
                match ty {
                    WitTy::Bool => {
                        fx.op(I::I32Const(0));
                        fx.op(I::I32Ne);
                    }
                    WitTy::IntS(_) => fx.op(I::I64ExtendI32S),
                    WitTy::IntU(_) | WitTy::Char => fx.op(I::I64ExtendI32U),
                    WitTy::S64 | WitTy::F64 => {}
                    WitTy::F32 => fx.op(I::F64PromoteF32),
                    _ => unreachable!("wit_scalar admitted a non-scalar"),
                }
            }
            (Repr::Scalar(k), _) => {
                em.lift_flat(&mut fx, ty, *base)?;
                em.unbox_scalar(&mut fx, k);
            }
            (Repr::Boxed, _) => em.lift_flat(&mut fx, ty, *base)?,
            (Repr::Mem(_), _) => {
                return Err("internal: Mem def params are not emitted yet (5.3)".into());
            }
        }
    }
    fx.op(I::Call(fidx));

    let res_kind = if typed_ok { fsig.result } else { Repr::Boxed };
    Ok((fparams, fx, res_kind))
}

fn lower_export_result(
    em: &mut Emitter<'_>,
    fx: &mut FnCtx,
    sig: &FuncSig,
    res_kind: Repr,
    imports: &ResourceImports,
) -> Result<Vec<ValType>, String> {
    use ValType::I32;
    let user_res_intrinsics = &imports.resources;
    let fresults = match flat_result(sig, &em.type_env)? {
        FlatRes::None => {
            fx.op(I::Drop);
            vec![]
        }
        FlatRes::One(t)
            if classify_resource(imports, sig.result.as_deref().unwrap_or("")).is_some() =>
        {
            let (rname, _) = classify_resource(imports, sig.result.as_deref().unwrap()).unwrap();
            let (new_i, _r, _d) = user_res_intrinsics[&rname];
            if let Repr::Scalar(k) = res_kind {
                em.box_scalar(fx, k);
            }
            fx.op(I::Call(new_i));
            flat(&t)
        }
        FlatRes::One(t) => {
            match res_kind {
                Repr::Scalar(k) if wit_scalar(&t) == Some(k) => match t {
                    WitTy::Bool | WitTy::S64 | WitTy::F64 => {}
                    WitTy::IntS(_) | WitTy::IntU(_) | WitTy::Char => fx.op(I::I32WrapI64),
                    WitTy::F32 => fx.op(I::F32DemoteF64),
                    _ => unreachable!("wit_scalar admitted a non-scalar"),
                },

                Repr::Scalar(k) => {
                    em.box_scalar(fx, k);
                    em.lower(fx, &t)?;
                }
                Repr::Boxed => em.lower(fx, &t)?,
                Repr::Mem(mt) => {
                    let l = fx.local(I32);
                    fx.op(I::LocalSet(l));
                    let mty = em.mem_tys[mt as usize].clone();
                    em.load_from_mem(fx, &mty, l, 0)?;
                    em.lower(fx, &t)?;
                }
            }
            flat(&t)
        }
        FlatRes::Retptr => {
            let ty = wit_ty(sig.result.as_deref().unwrap(), &em.type_env)?;

            if matches!(res_kind, Repr::Mem(mt) if em.mem_tys[mt as usize] == ty) {
                return Ok(vec![I32]);
            }

            match res_kind {
                Repr::Boxed => {}
                Repr::Scalar(k) => em.box_scalar(fx, k),
                Repr::Mem(mt) => {
                    let l = fx.local(I32);
                    fx.op(I::LocalSet(l));
                    let mty = em.mem_tys[mt as usize].clone();
                    em.load_from_mem(fx, &mty, l, 0)?;
                }
            }
            let area = fx.local(I32);
            if matches!(
                ty,
                WitTy::Record(_)
                    | WitTy::Tuple(_)
                    | WitTy::Option(_)
                    | WitTy::Result(..)
                    | WitTy::Variant(_)
            ) {
                let rbox = fx.local(I32);
                fx.op(I::LocalSet(rbox));
                fx.op(I::I32Const(size_of(&ty) as i32));
                fx.op(I::Call(em.h.alloc));
                fx.op(I::LocalSet(area));
                em.store_to_mem(fx, &ty, rbox, area, 0)?;
                fx.op(I::LocalGet(area));
            } else {
                em.lower(fx, &ty)?;
                let lp = fx.local(I32);
                let ll = fx.local(I32);
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
            }
            vec![I32]
        }
    };
    Ok(fresults)
}
