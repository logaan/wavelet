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
    let mut entries = entries
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{}: {e}", deps_dir.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let group = if path.is_dir() {
            UnresolvedPackageGroup::parse_dir(&path)
                .map_err(|e| format!("{}: {e:#}", path.display()))?
        } else {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(ext, "wasm" | "wat") {
                resolve
                    .push_file(&path)
                    .map_err(|e| format!("{}: {e:#}", path.display()))?;
                continue;
            }
            if ext != "wit" {
                continue;
            }
            let contents =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            UnresolvedPackageGroup::parse(&path, &contents)
                .map_err(|e| format!("{}: {e:#}", path.display()))?
        };
        groups.push(group);
    }
    if !groups.is_empty() {
        let main = groups.remove(0);
        resolve
            .push_groups(main, groups)
            .map_err(|e| format!("{}: {e:#}", deps_dir.display()))?;
    }

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
    let mut collected = ResolvedTypes::default();
    let mut type_ifaces: Vec<(String, String, String)> = Vec::new();

    for (iface_name, &iface_id) in &pkg.interfaces {
        let iface = &resolve.interfaces[iface_id];

        for (type_name, &type_id) in &iface.types {
            let tdef = &resolve.types[type_id];

            let internal_name = named_type(&resolve, type_id)?;
            type_ifaces.push((internal_name.clone(), iface_name.clone(), type_name.clone()));
            collected.collect(&resolve, type_id)?;
            if matches!(tdef.kind, TypeDefKind::Resource) {
                funcs.push(FuncSig {
                    name: format!("[resource-drop]{type_name}"),
                    iface: iface_name.clone(),
                    params: vec![("self".to_owned(), format!("own<{internal_name}>"))],
                    result: None,
                });
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
        types: collected.records,
        type_defs: collected.defs,
        aliases: collected.aliases,
        type_ifaces,
    }))
}

#[derive(Default)]
struct ResolvedTypes {
    records: Vec<(String, Vec<(String, String)>)>,
    defs: Vec<(String, TypeDef)>,
    aliases: Vec<(String, String)>,
    seen: std::collections::HashSet<wit_parser::TypeId>,
}

impl ResolvedTypes {
    fn collect(&mut self, resolve: &Resolve, id: wit_parser::TypeId) -> Result<(), String> {
        if !self.seen.insert(id) {
            return Ok(());
        }
        let ty = &resolve.types[id];
        for child in referenced_types(&ty.kind) {
            if let Type::Id(id) = child {
                self.collect(resolve, id)?;
            }
        }
        if ty.name.is_none() {
            structural_type_string(resolve, &ty.kind)?;
            return Ok(());
        }
        let name = named_type(resolve, id)?;
        match &ty.kind {
            TypeDefKind::Record(record) => {
                let fields = record
                    .fields
                    .iter()
                    .map(|field| Ok((field.name.clone(), type_string(resolve, &field.ty)?)))
                    .collect::<Result<_, String>>()?;
                self.records.push((name, fields));
            }
            TypeDefKind::Resource => self.defs.push((name, TypeDef::Resource)),
            TypeDefKind::Enum(value) => self.defs.push((
                name,
                TypeDef::Enum(value.cases.iter().map(|case| case.name.clone()).collect()),
            )),
            TypeDefKind::Flags(value) => self.defs.push((
                name,
                TypeDef::Flags(value.flags.iter().map(|flag| flag.name.clone()).collect()),
            )),
            TypeDefKind::Variant(value) => {
                let cases = value
                    .cases
                    .iter()
                    .map(|case| {
                        Ok((
                            case.name.clone(),
                            case.ty
                                .as_ref()
                                .map(|ty| type_string(resolve, ty))
                                .transpose()?,
                        ))
                    })
                    .collect::<Result<_, String>>()?;
                self.defs.push((name, TypeDef::Variant(cases)));
            }
            TypeDefKind::List(_)
            | TypeDefKind::Option(_)
            | TypeDefKind::Result(_)
            | TypeDefKind::Tuple(_)
            | TypeDefKind::Type(_)
            | TypeDefKind::Handle(_) => {
                self.aliases
                    .push((name, structural_type_string(resolve, &ty.kind)?));
            }
            other => {
                return Err(format!(
                    "unsupported WIT type `{}`: {}",
                    ty.name.as_deref().unwrap(),
                    other.as_str()
                ));
            }
        }
        Ok(())
    }
}

fn referenced_types(kind: &TypeDefKind) -> Vec<Type> {
    match kind {
        TypeDefKind::Record(value) => value.fields.iter().map(|field| field.ty).collect(),
        TypeDefKind::Variant(value) => value.cases.iter().filter_map(|case| case.ty).collect(),
        TypeDefKind::Tuple(value) => value.types.clone(),
        TypeDefKind::Result(value) => value.ok.into_iter().chain(value.err).collect(),
        TypeDefKind::List(ty) | TypeDefKind::Option(ty) | TypeDefKind::Type(ty) => vec![*ty],
        TypeDefKind::Handle(Handle::Own(id) | Handle::Borrow(id)) => vec![Type::Id(*id)],
        _ => Vec::new(),
    }
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

pub(crate) fn type_identity(package: &str, iface: &str, name: &str) -> String {
    let identity = format!("{package}/{iface}/{name}");
    let mut result = String::from("wit-x");
    for byte in identity.bytes() {
        use std::fmt::Write;
        write!(result, "{byte:02x}").unwrap();
    }
    result
}

fn named_type(resolve: &Resolve, id: wit_parser::TypeId) -> Result<String, String> {
    let ty = &resolve.types[id];
    let name = ty
        .name
        .as_deref()
        .ok_or("anonymous resource handle in WIT dep")?;
    let wit_parser::TypeOwner::Interface(iface_id) = ty.owner else {
        return Err(format!("WIT type `{name}` is not owned by an interface"));
    };
    let iface = &resolve.interfaces[iface_id];
    let package = iface.package.ok_or("WIT interface has no package")?;
    let iface_name = iface.name.as_deref().ok_or("WIT interface has no name")?;
    Ok(type_identity(
        &resolve.packages[package].name.to_string(),
        iface_name,
        name,
    ))
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

            if tdef.name.is_some() {
                named_type(resolve, *id)?
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
            let name = named_type(resolve, *id)?;
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
