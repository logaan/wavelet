use super::*;
use std::collections::HashSet;

pub(crate) fn scope_dependency_types(
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> HashMap<String, Dep> {
    let targets = dependency_alias_targets(deps);
    let mut identities: HashMap<String, HashSet<String>> = HashMap::new();
    for (package, dep) in deps {
        for (_, iface, original) in &dep.type_ifaces {
            if !info
                .imports
                .iter()
                .any(|import| import.package == *package && import_iface(&import.path) == *iface)
            {
                continue;
            }
            identities
                .entry(original.clone())
                .or_default()
                .insert(resolve_alias_identity(
                    &crate::witdep::type_identity(&dep.package, iface, original),
                    &targets,
                ));
        }
    }
    deps.iter()
        .map(|(package, dep)| {
            let names: HashMap<_, _> = dep
                .type_ifaces
                .iter()
                .map(|(internal, iface, original)| {
                    (
                        internal.clone(),
                        crate::witdep::type_identity(&dep.package, iface, original),
                    )
                })
                .collect();
            let rename = |name: &str| names.get(name).cloned().unwrap_or_else(|| name.to_owned());
            let rewrite = |text: &str| rewrite_type_names(text, &names);
            let mut scoped = Dep {
                package: dep.package.clone(),
                package_wit: dep.package_wit.clone(),
                funcs: dep
                    .funcs
                    .iter()
                    .map(|sig| FuncSig {
                        name: sig.name.clone(),
                        iface: sig.iface.clone(),
                        params: sig
                            .params
                            .iter()
                            .map(|(name, ty)| (name.clone(), rewrite(ty)))
                            .collect(),
                        result: sig.result.as_deref().map(rewrite),
                    })
                    .collect(),
                types: dep
                    .types
                    .iter()
                    .map(|(name, fields)| {
                        (
                            rename(name),
                            fields
                                .iter()
                                .map(|(field, ty)| (field.clone(), rewrite(ty)))
                                .collect(),
                        )
                    })
                    .collect(),
                type_defs: dep
                    .type_defs
                    .iter()
                    .map(|(name, def)| {
                        let def = match def {
                            TypeDef::Variant(cases) => TypeDef::Variant(
                                cases
                                    .iter()
                                    .map(|(case, payload)| {
                                        (case.clone(), payload.as_deref().map(rewrite))
                                    })
                                    .collect(),
                            ),
                            other => other.clone(),
                        };
                        (rename(name), def)
                    })
                    .collect(),
                aliases: dep
                    .aliases
                    .iter()
                    .filter_map(|(name, target)| {
                        let name = rename(name);
                        let target = rewrite(target);
                        (name != target).then_some((name, target))
                    })
                    .collect(),
                type_ifaces: dep
                    .type_ifaces
                    .iter()
                    .map(|(name, iface, original)| (rename(name), iface.clone(), original.clone()))
                    .collect(),
            };
            scoped.type_ifaces.sort();
            scoped.type_ifaces.dedup();
            let origins = scoped.type_ifaces.clone();
            for (canonical, iface, original) in origins {
                let mut aliases = Vec::new();
                if identities.get(&original).is_some_and(|ids| ids.len() == 1) {
                    aliases.push(original.clone());
                }
                for import in info.imports.iter().filter(|import| {
                    import.package == *package && import_iface(&import.path) == iface
                }) {
                    aliases.push(format!("{}-{original}", import.alias));
                }
                for alias in aliases {
                    if alias != canonical && !scoped.aliases.iter().any(|(name, _)| name == &alias)
                    {
                        scoped.aliases.push((alias.clone(), canonical.clone()));
                        scoped
                            .type_ifaces
                            .push((alias, iface.clone(), original.clone()));
                    }
                }
            }
            (package.clone(), scoped)
        })
        .collect()
}

fn rewrite_type_names(text: &str, names: &HashMap<String, String>) -> String {
    let mut result = String::new();
    let mut token = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '-' {
            token.push(c);
        } else {
            result.push_str(names.get(&token).unwrap_or(&token));
            token.clear();
            result.push(c);
        }
    }
    result.push_str(names.get(&token).unwrap_or(&token));
    result
}

fn dependency_alias_targets(deps: &HashMap<String, Dep>) -> HashMap<String, String> {
    let mut targets = HashMap::new();
    for dep in deps.values() {
        let names: HashMap<_, _> = dep
            .type_ifaces
            .iter()
            .map(|(internal, iface, original)| {
                (
                    internal.clone(),
                    crate::witdep::type_identity(&dep.package, iface, original),
                )
            })
            .collect();
        for (name, target) in &dep.aliases {
            let name = names.get(name).unwrap_or(name).clone();
            let target = rewrite_type_names(target, &names);
            if name != target {
                targets.insert(name, target);
            }
        }
    }
    targets
}

fn resolve_alias_identity(name: &str, targets: &HashMap<String, String>) -> String {
    let mut current = name;
    let mut seen = HashSet::new();
    while seen.insert(current) {
        let Some(target) = targets.get(current) else {
            break;
        };
        current = target;
    }
    current.to_owned()
}
