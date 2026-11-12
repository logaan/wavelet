use super::*;

pub(super) fn initialize_emitter<'a>(
    arena: &'a Arena,
    roots: &[NodeId],
    info: &'a FileInfo,
    deps: &'a HashMap<String, Dep>,
) -> Result<(Emitter<'a>, Vec<ResForm>), String> {
    let node_types_tbl = {
        let import_sigs = import_signatures(info, deps);
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

    let mut em = Emitter::new(arena, info, deps);
    em.type_env = type_env;
    em.local_cases = local_cases;
    em.node_types = node_types_tbl;

    Ok((em, user_res_forms))
}
