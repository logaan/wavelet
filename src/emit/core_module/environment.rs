use super::*;

pub(super) fn initialize_emitter<'a>(
    arena: &'a Arena,
    roots: &[NodeId],
    info: &'a FileInfo,
    deps: &'a HashMap<String, Dep>,
) -> Result<(Emitter<'a>, Vec<ResForm>), String> {
    let node_types_tbl = {
        let mut import_sigs = crate::check::ImportSigs::new();
        for imp in &info.imports {
            if crate::wit::is_macro_only(imp) {
                continue;
            }
            if let Some(dep) = deps.get(&imp.package) {
                let iface = imp.path.split_once('/').map(|(_, i)| i).unwrap_or("api");
                import_sigs.extend(crate::wit::import_sigs_for(&imp.alias, &dep.funcs, iface));
            }
        }
        import_sigs.extend(crate::wit::functor_import_sigs(&info.functors));
        crate::check::node_types_with_dependencies(arena, roots, &import_sigs, deps, &info.imports)?
    };

    let mut type_env = TypeEnv::default();
    for (name, fields) in record_types(arena, &info.types) {
        type_env.records.insert(name, fields);
    }

    let (local_defs, local_aliases) = local_non_record_types(arena, &info.types);

    let mut local_cases: HashMap<String, bool> = HashMap::new();
    for (_name, def) in &local_defs {
        match def {
            TypeDef::Enum(cases) => {
                for c in cases {
                    local_cases.insert(c.clone(), false);
                }
            }
            TypeDef::Variant(cases) => {
                for (c, p) in cases {
                    local_cases.insert(c.clone(), p.is_some());
                }
            }
            _ => {}
        }
    }
    for (name, def) in local_defs {
        type_env.defs.insert(name, def);
    }
    for (name, target) in local_aliases {
        type_env.aliases.insert(name, target);
    }
    for dep in deps.values() {
        for (name, fields) in &dep.types {
            type_env
                .records
                .entry(name.clone())
                .or_insert_with(|| fields.clone());
        }
        for (name, def) in &dep.type_defs {
            type_env
                .defs
                .entry(name.clone())
                .or_insert_with(|| def.clone());
        }
        for (name, target) in &dep.aliases {
            type_env
                .aliases
                .entry(name.clone())
                .or_insert_with(|| target.clone());
        }
    }

    for inst in &info.functors {
        type_env
            .defs
            .entry("set".to_string())
            .or_insert(TypeDef::Resource);

        type_env
            .defs
            .entry(format!("{}.set", inst.iface))
            .or_insert(TypeDef::Resource);
    }

    let user_res_forms = user_resource_forms(arena, roots)?;
    for rf in &user_res_forms {
        type_env
            .defs
            .entry(rf.name.clone())
            .or_insert(TypeDef::Resource);
    }

    let mut em = Emitter {
        arena,
        info,
        deps,
        type_env,
        local_cases,
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
        node_types: node_types_tbl,
        mem_tys: Vec::new(),
    };

    em.false_addr = DATA_BASE;
    em.put_i32(TAG_BOOL);
    em.put_i32(0);
    em.true_addr = DATA_BASE + 8;
    em.put_i32(TAG_BOOL);
    em.put_i32(1);

    Ok((em, user_res_forms))
}
