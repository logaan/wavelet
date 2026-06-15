//! Resolve an `Import` against external WIT vendored under a project's
//! `wit/deps` directory (populated by `wkg`, see `dev-notes/decouple-wasi.md`).
//!
//! This is the generic, registry-fed counterpart to a sibling-`.wvl`
//! dependency: instead of synthesizing a [`Dep`] from another Wavelet file in
//! the build set, we parse a WIT package with `wit-parser` and project it into
//! the *same* [`Dep`] shape the emitter already consumes. The emitter therefore
//! sees one uniform dependency structure regardless of where the interface came
//! from. This is the only source of external (host, `wasi:*`) WIT: host imports
//! and exports resolve through it just like a third party's package.

use std::path::Path;

use wit_parser::{
    Function, Handle, Resolve, Type, TypeDefKind, UnresolvedPackageGroup,
};

use crate::emit::{Dep, TypeDef};
use crate::wit::FuncSig;

/// Try to resolve `package` (a versionless `ns:name`) against `<deps_dir>`.
///
/// `deps_dir` is a project's `wit/deps` directory. Each entry in it is a WIT
/// package — either `ns-name.wit` / `ns-name.wasm` or a `ns-name/` directory of
/// `.wit` files. We parse them all into one [`Resolve`] (so cross-package type
/// references resolve), then locate the package whose name matches `package` and
/// build a [`Dep`] from its interfaces.
///
/// Returns `Ok(None)` when no `wit/deps` directory exists or it holds no package
/// matching `package` — the caller treats that as "not found here" and reports
/// its usual unsatisfied-import error. Returns `Err` only when a `wit/deps`
/// entry exists but fails to parse.
pub fn resolve_dep(deps_dir: &Path, package: &str) -> Result<Option<Dep>, String> {
    if !deps_dir.is_dir() {
        return Ok(None);
    }

    let mut resolve = Resolve::default();

    // Parse *every* entry in `wit/deps` into an unresolved package group first,
    // then push them all together so cross-package references resolve. The deps
    // `wkg` fetches are interdependent (`wasi:http` uses `wasi:io`, `wasi:clocks`,
    // …), so pushing each directory on its own fails — a lone `wasi:clocks`
    // package can't find `wasi:io`. `push_groups` topologically sorts the whole
    // set and resolves the cross-references in one shot.
    let mut groups: Vec<UnresolvedPackageGroup> = Vec::new();
    let entries = std::fs::read_dir(deps_dir)
        .map_err(|e| format!("{}: {e}", deps_dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", deps_dir.display()))?;
        let path = entry.path();
        let group = if path.is_dir() {
            UnresolvedPackageGroup::parse_dir(&path)
                .map_err(|e| format!("{}: {e:#}", path.display()))?
        } else {
            // A single-file package; skip lock files / hidden files.
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "wit" | "wasm" | "wat") {
                continue;
            }
            let contents = std::fs::read_to_string(&path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            UnresolvedPackageGroup::parse(&path, &contents)
                .map_err(|e| format!("{}: {e:#}", path.display()))?
        };
        groups.push(group);
    }
    if groups.is_empty() {
        return Ok(None);
    }
    // `push_groups` takes one "main" group plus the rest as deps, but sorts the
    // whole lot internally — which one is "main" is immaterial here.
    let main = groups.remove(0);
    resolve
        .push_groups(main, groups)
        .map_err(|e| format!("{}: {e:#}", deps_dir.display()))?;

    // Find the package whose `namespace:name` matches the (versionless) import.
    let pkg_id = resolve.packages.iter().find_map(|(id, pkg)| {
        let name = &pkg.name;
        if format!("{}:{}", name.namespace, name.name) == package {
            Some(id)
        } else {
            None
        }
    });
    let Some(pkg_id) = pkg_id else {
        return Ok(None);
    };
    let pkg = &resolve.packages[pkg_id];

    // Full package id including version, e.g. `acme:greet@0.1.0`.
    let full_package = pkg.name.to_string();

    let mut funcs = Vec::new();
    let mut types: Vec<(String, Vec<(String, String)>)> = Vec::new();
    let mut type_defs: Vec<(String, TypeDef)> = Vec::new();

    for (iface_name, &iface_id) in &pkg.interfaces {
        let iface = &resolve.interfaces[iface_id];

        // Named types this interface defines, so the emitter can lay out values
        // we pass to / receive from it. Records land in `types`; enum/variant/
        // flags in `type_defs` (the generic-bridge kinds).
        for (type_name, &type_id) in &iface.types {
            let tdef = &resolve.types[type_id];
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
                    // An opaque host resource: carried as a handle. Recording it
                    // in `type_defs` lets the boundary `TypeEnv` resolve a bare
                    // reference (a param typed just `name`, not `own<name>`) to a
                    // handle through the generic path.
                    type_defs.push((type_name.clone(), TypeDef::Resource));
                    // The component model gives every resource an implicit
                    // `[resource-drop]name` import (it is not a WIT `function`).
                    // Synthesize a `FuncSig` for it — `own<name> -> ()` — so the
                    // generic bridge can lower a drop call like any other op
                    // (reached from source by the bare op name `name`).
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
                _ => {}
            }
        }

        for (_fname, func) in &iface.functions {
            funcs.push(func_sig(&resolve, iface_name, func)?);
        }
    }

    let package_wit = package_wit_text(&resolve, pkg_id)?;

    Ok(Some(Dep { package: full_package, funcs, package_wit, types, type_defs }))
}

/// Project one parsed WIT [`Function`] into the [`FuncSig`] the emitter expects.
fn func_sig(
    resolve: &Resolve,
    iface_name: &str,
    func: &Function,
) -> Result<FuncSig, String> {
    let params = func
        .params
        .iter()
        .map(|p| Ok((p.name.clone(), type_string(resolve, &p.ty)?)))
        .collect::<Result<Vec<_>, String>>()?;
    let result = match &func.result {
        Some(t) => Some(type_string(resolve, t)?),
        None => None,
    };
    Ok(FuncSig { name: func.name.clone(), iface: iface_name.to_string(), params, result })
}

/// Render a `wit-parser` [`Type`] as the WIT type *string* Wavelet's emitter
/// uses (e.g. `s32`, `string`, `list<u8>`, `option<string>`, a named record).
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
        Type::ErrorContext => "error-context".into(),
        Type::Id(id) => {
            let tdef = &resolve.types[*id];
            // A named type (record/variant/enum/resource/alias): refer to it by
            // name. Anonymous compound types (list/option/result/tuple/handle)
            // are rendered structurally.
            if let Some(name) = &tdef.name {
                name.clone()
            } else {
                structural_type_string(resolve, &tdef.kind)?
            }
        }
    })
}

/// Render a structural [`TypeDefKind`] (list/option/result/tuple/handle) as its
/// WIT type string — used both for anonymous compound types and for *named*
/// aliases of them (`type field-value = list<u8>`).
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

/// Emit a nested-package WIT string for the dependency and *all* the packages it
/// transitively pulls in, in `package ns:name@ver { … }` braced form so the block
/// slots into the synthesized world file.
///
/// We delegate to `wit-component`'s [`WitPrinter`] rather than hand-rolling the
/// WIT text. Real host WIT (`wasi:http` and friends) crosses interface and
/// package boundaries with `use` statements, type aliases, and re-exports that a
/// naive flattener gets wrong (e.g. it renders a `use`d `error-code` as the
/// self-referential `type error-code = error-code`). The printer handles all of
/// that correctly and stays in lockstep with the parser, so the encoder always
/// gets valid WIT.
///
/// Every package in `resolve` is printed in nested-braced form. `resolve` holds
/// exactly the project's `wit/deps`, so this is `pkg` plus its transitive
/// dependency packages (`wasi:http` → `wasi:io`, `wasi:clocks`, …); they are all
/// needed for the package to type-check at encode time.
fn package_wit_text(
    resolve: &Resolve,
    pkg_id: wit_parser::PackageId,
) -> Result<String, String> {
    use wit_component::{Output, WitPrinter};

    // Print the dependency package first, then every other package as a nested
    // `package … { … }` block. `print` renders its first argument with the
    // top-level `package …;` syntax, but we want *all* packages braced so they
    // can be appended after the synthesized world's own `package …; world …`.
    // So print each package individually with `print_package(_, _, is_main=false)`.
    let mut printer = WitPrinter::default();
    printer.emit_docs(false);

    // Order: the dep package first, then the rest in a stable order.
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
