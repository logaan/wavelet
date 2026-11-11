use super::*;

pub(super) struct ResourceImports {
    pub functors: Vec<(u32, u32, u32)>,
    pub resources: HashMap<String, (u32, u32, u32)>,
}

pub(super) fn register_imports(
    em: &mut Emitter<'_>,
    user_res_forms: &[ResForm],
) -> Result<ResourceImports, String> {
    let info = em.info;
    let deps = em.deps;
    let feats = features_of(em.arena, info);
    let mut n_imports = 0u32;
    let mut add_import =
        |em: &mut Emitter, module: &str, field: &str, p: Vec<ValType>, r: Vec<ValType>| {
            let t = em.ty_idx(p, r);
            em.imports.push((module.to_string(), field.to_string(), t));
            em.import_fn
                .insert((module.to_string(), field.to_string()), n_imports);
            n_imports += 1;
        };

    use ValType::I32;
    for (alias, fname) in &feats.dep_calls {
        if info.functors.iter().any(|f| &f.alias == alias) {
            continue;
        }

        if user_res_forms.iter().any(|rf| &rf.name == alias) {
            continue;
        }
        let imp = info
            .imports
            .iter()
            .find(|i| &i.alias == alias)
            .ok_or(format!("unknown import alias `{alias}`"))?;
        let dep = deps.get(&imp.package).ok_or(format!(
            "dependency `{}` is not in the build set",
            imp.package
        ))?;
        let iface = import_iface(&imp.path);

        let sig = match resolve_dep_func(dep, &iface, fname) {
            Ok(sig) => sig,
            Err(e) => {
                if dep_case(dep, fname).is_some() {
                    continue;
                }
                return Err(e);
            }
        };
        let mut p = Vec::new();
        for (_, t) in &sig.params {
            p.extend_from_slice(&flat_checked(&wit_ty(t, &em.type_env)?)?);
        }
        let r = match flat_result(sig, &em.type_env)? {
            FlatRes::None => vec![],
            FlatRes::One(t) => flat(&t),
            FlatRes::Retptr => {
                p.push(I32);
                vec![]
            }
        };
        let module = versioned_iface(&dep.package, &iface);

        if !em
            .import_fn
            .contains_key(&(module.clone(), sig.name.clone()))
        {
            add_import(em, &module, &sig.name, p, r);
        }
    }

    let mut functor_intrinsics: Vec<(u32, u32, u32)> = Vec::new();
    for inst in &info.functors {
        let module = format!("[export]{}", versioned_iface(&info.package, &inst.iface));
        add_import(em, &module, "[resource-new]set", vec![I32], vec![I32]);
        add_import(em, &module, "[resource-rep]set", vec![I32], vec![I32]);
        add_import(em, &module, "[resource-drop]set", vec![I32], vec![]);
        let new_i = em.import_idx(&module, "[resource-new]set");
        let rep_i = em.import_idx(&module, "[resource-rep]set");
        let drop_i = em.import_idx(&module, "[resource-drop]set");
        functor_intrinsics.push((new_i, rep_i, drop_i));
    }

    let mut user_res_intrinsics: HashMap<String, (u32, u32, u32)> = HashMap::new();
    for rf in user_res_forms {
        let Some(decl) = info.resources.iter().find(|r| r.name == rf.name) else {
            continue;
        };
        let Some(iface_path) = &decl.iface else {
            return Err(format!(
                "resource `{}` is declared but not exported; the wasm backend only \
                 supports exported resources yet",
                rf.name
            ));
        };
        let iface = if is_external_iface(iface_path) {
            external_versioned_in(iface_path, deps)
        } else {
            versioned_iface(&info.package, iface_path)
        };
        let module = format!("[export]{iface}");
        let nnew = format!("[resource-new]{}", rf.name);
        let nrep = format!("[resource-rep]{}", rf.name);
        let ndrop = format!("[resource-drop]{}", rf.name);
        add_import(em, &module, &nnew, vec![I32], vec![I32]);
        add_import(em, &module, &nrep, vec![I32], vec![I32]);
        add_import(em, &module, &ndrop, vec![I32], vec![]);
        let ni = em.import_idx(&module, &nnew);
        let ri = em.import_idx(&module, &nrep);
        let di = em.import_idx(&module, &ndrop);
        user_res_intrinsics.insert(rf.name.clone(), (ni, ri, di));
    }

    Ok(ResourceImports {
        functors: functor_intrinsics,
        resources: user_res_intrinsics,
    })
}
