use std::path::Path;

use wit_parser::{Function, Handle, Resolve, Type, TypeDefKind, UnresolvedPackageGroup};

use crate::emit::{Dep, TypeDef};
use crate::wit::FuncSig;

pub fn resolve_dep(deps_dir: &Path, package: &str) -> Result<Option<Dep>, String> {
    if !deps_dir.is_dir() {
        return Ok(None);
    }

    let mut resolve = Resolve::default();

    let mut groups: Vec<UnresolvedPackageGroup> = Vec::new();
    let entries =
        std::fs::read_dir(deps_dir).map_err(|e| format!("{}: {e}", deps_dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", deps_dir.display()))?;
        let path = entry.path();
        let group = if path.is_dir() {
            UnresolvedPackageGroup::parse_dir(&path)
                .map_err(|e| format!("{}: {e:#}", path.display()))?
        } else {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "wit" | "wasm" | "wat") {
                continue;
            }
            let contents =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            UnresolvedPackageGroup::parse(&path, &contents)
                .map_err(|e| format!("{}: {e:#}", path.display()))?
        };
        groups.push(group);
    }
    if groups.is_empty() {
        return Ok(None);
    }

    let main = groups.remove(0);
    resolve
        .push_groups(main, groups)
        .map_err(|e| format!("{}: {e:#}", deps_dir.display()))?;

    let (base, requested_version) = package
        .split_once('@')
        .map_or((package, None), |(base, version)| (base, Some(version)));
    let candidates: Vec<_> = resolve
        .packages
        .iter()
        .filter_map(|(id, pkg)| {
            let name = &pkg.name;
            let same_name = format!("{}:{}", name.namespace, name.name) == base;
            let same_version = requested_version.is_none_or(|version| {
                name.version
                    .as_ref()
                    .is_some_and(|v| v.to_string() == version)
            });
            (same_name && same_version).then_some(id)
        })
        .collect();
    let pkg_id = match candidates.as_slice() {
        [] => return Ok(None),
        [id] => *id,
        _ => {
            return Err(format!(
                "dependency `{package}` has multiple versions; specify an explicit version"
            ));
        }
    };
    let pkg = &resolve.packages[pkg_id];

    let full_package = pkg.name.to_string();

    let mut funcs = Vec::new();
    let mut types: Vec<(String, Vec<(String, String)>)> = Vec::new();
    let mut type_defs: Vec<(String, TypeDef)> = Vec::new();
    let mut aliases: Vec<(String, String)> = Vec::new();
    let mut type_ifaces: Vec<(String, String)> = Vec::new();

    for (iface_name, &iface_id) in &pkg.interfaces {
        let iface = &resolve.interfaces[iface_id];

        for (type_name, &type_id) in &iface.types {
            let tdef = &resolve.types[type_id];

            type_ifaces.push((type_name.clone(), iface_name.clone()));
            match &tdef.kind {
                TypeDefKind::Record(rec) => {
                    let fields = rec
                        .fields
                        .iter()
                        .map(|f| Ok((f.name.clone(), type_string(&resolve, &f.ty)?)))
                        .collect::<Result<Vec<_>, String>>()?;
                    types.push((type_name.clone(), fields));
                }
                TypeDefKind::Resource => {
                    type_defs.push((type_name.clone(), TypeDef::Resource));

                    funcs.push(FuncSig {
                        name: format!("[resource-drop]{type_name}"),
                        iface: iface_name.clone(),
                        params: vec![("self".to_string(), format!("own<{type_name}>"))],
                        result: None,
                    });
                }
                TypeDefKind::Enum(en) => {
                    let cases = en.cases.iter().map(|c| c.name.clone()).collect();
                    type_defs.push((type_name.clone(), TypeDef::Enum(cases)));
                }
                TypeDefKind::Flags(fl) => {
                    let names = fl.flags.iter().map(|f| f.name.clone()).collect();
                    type_defs.push((type_name.clone(), TypeDef::Flags(names)));
                }

                TypeDefKind::List(_)
                | TypeDefKind::Option(_)
                | TypeDefKind::Result(_)
                | TypeDefKind::Tuple(_)
                | TypeDefKind::Type(_)
                | TypeDefKind::Handle(_) => {
                    aliases.push((
                        type_name.clone(),
                        structural_type_string(&resolve, &tdef.kind)?,
                    ));
                }
                TypeDefKind::Variant(var) => {
                    let cases = var
                        .cases
                        .iter()
                        .map(|c| {
                            let pay = match &c.ty {
                                Some(t) => Some(type_string(&resolve, t)?),
                                None => None,
                            };
                            Ok((c.name.clone(), pay))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    type_defs.push((type_name.clone(), TypeDef::Variant(cases)));
                }
                other => {
                    return Err(format!(
                        "unsupported WIT type `{type_name}`: {}",
                        other.as_str()
                    ));
                }
            }
        }

        for (_fname, func) in &iface.functions {
            funcs.push(func_sig(&resolve, iface_name, func)?);
        }
    }

    let package_wit = package_wit_text(&resolve, pkg_id)?;

    Ok(Some(Dep {
        package: full_package,
        funcs,
        package_wit,
        types,
        type_defs,
        aliases,
        type_ifaces,
    }))
}

fn func_sig(resolve: &Resolve, iface_name: &str, func: &Function) -> Result<FuncSig, String> {
    if func.kind.is_async() {
        return Err(format!(
            "unsupported async WIT function `{iface_name}/{}`",
            func.name
        ));
    }
    let params = func
        .params
        .iter()
        .map(|p| Ok((p.name.clone(), type_string(resolve, &p.ty)?)))
        .collect::<Result<Vec<_>, String>>()?;
    let result = match &func.result {
        Some(t) => Some(type_string(resolve, t)?),
        None => None,
    };
    Ok(FuncSig {
        name: func.name.clone(),
        iface: iface_name.to_string(),
        params,
        result,
    })
}

fn type_string(resolve: &Resolve, ty: &Type) -> Result<String, String> {
    Ok(match ty {
        Type::Bool => "bool".into(),
        Type::U8 => "u8".into(),
        Type::U16 => "u16".into(),
        Type::U32 => "u32".into(),
        Type::U64 => "u64".into(),
        Type::S8 => "s8".into(),
        Type::S16 => "s16".into(),
        Type::S32 => "s32".into(),
        Type::S64 => "s64".into(),
        Type::F32 => "f32".into(),
        Type::F64 => "f64".into(),
        Type::Char => "char".into(),
        Type::String => "string".into(),
        Type::ErrorContext => return Err("unsupported WIT type: error-context".into()),
        Type::Id(id) => {
            let tdef = &resolve.types[*id];

            if let Some(name) = &tdef.name {
                name.clone()
            } else {
                structural_type_string(resolve, &tdef.kind)?
            }
        }
    })
}

fn structural_type_string(resolve: &Resolve, kind: &TypeDefKind) -> Result<String, String> {
    Ok(match kind {
        TypeDefKind::List(inner) => format!("list<{}>", type_string(resolve, inner)?),
        TypeDefKind::Option(inner) => format!("option<{}>", type_string(resolve, inner)?),
        TypeDefKind::Result(r) => match (&r.ok, &r.err) {
            (Some(o), Some(e)) => format!(
                "result<{}, {}>",
                type_string(resolve, o)?,
                type_string(resolve, e)?
            ),
            (Some(o), None) => format!("result<{}>", type_string(resolve, o)?),
            (None, Some(e)) => format!("result<_, {}>", type_string(resolve, e)?),
            (None, None) => "result".into(),
        },
        TypeDefKind::Tuple(t) => {
            let parts = t
                .types
                .iter()
                .map(|t| type_string(resolve, t))
                .collect::<Result<Vec<_>, _>>()?;
            format!("tuple<{}>", parts.join(", "))
        }
        TypeDefKind::Handle(Handle::Own(id)) | TypeDefKind::Handle(Handle::Borrow(id)) => {
            let inner = &resolve.types[*id];
            let name = inner
                .name
                .clone()
                .ok_or("anonymous resource handle in WIT dep")?;
            let kw = if matches!(kind, TypeDefKind::Handle(Handle::Own(_))) {
                "own"
            } else {
                "borrow"
            };
            format!("{kw}<{name}>")
        }
        TypeDefKind::Type(inner) => type_string(resolve, inner)?,
        other => {
            return Err(format!(
                "unsupported anonymous WIT type in dep: {}",
                other.as_str()
            ));
        }
    })
}

fn package_wit_text(resolve: &Resolve, pkg_id: wit_parser::PackageId) -> Result<String, String> {
    use wit_component::{Output, WitPrinter};

    let mut printer = WitPrinter::default();
    printer.emit_docs(false);

    let mut ids: Vec<wit_parser::PackageId> = vec![pkg_id];
    for (id, _) in resolve.packages.iter() {
        if id != pkg_id {
            ids.push(id);
        }
    }
    for (i, id) in ids.iter().enumerate() {
        if i > 0 {
            printer.output.newline();
            printer.output.newline();
        }
        printer
            .print_package(resolve, *id, false)
            .map_err(|e| format!("rendering WIT for dep package: {e:#}"))?;
    }
    Ok(printer.output.to_string())
}
