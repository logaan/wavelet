use super::*;

pub fn dep_package_wit(arena: &Arena, info: &FileInfo) -> Result<String, String> {
    let mut out = format!("package {} {{\n", info.package);
    for iface in crate::wit::iface_order(&info.exports, !info.types.is_empty()) {
        out.push_str(&format!("  interface {iface} {{\n"));
        if iface == "api" {
            for (name, ty) in &info.types {
                out.push_str(&format!("    {}\n", type_decl(arena, name, *ty)?));
            }
        }
        for sig in info.exports.iter().filter(|s| s.iface == iface) {
            out.push_str(&format!("    {}\n", sig.to_wit()));
        }
        out.push_str("  }\n");
    }
    out.push_str("}\n");
    Ok(out)
}

pub(crate) fn dep_type_uses(
    texts: &[String],
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> Vec<(String, Vec<String>)> {
    const RESERVED: &[&str] = &[
        "bool",
        "u8",
        "u16",
        "u32",
        "u64",
        "s8",
        "s16",
        "s32",
        "s64",
        "f32",
        "f64",
        "char",
        "string",
        "list",
        "option",
        "result",
        "tuple",
        "own",
        "borrow",
        "record",
        "variant",
        "enum",
        "flags",
        "type",
        "func",
        "resource",
        "static",
        "constructor",
        "use",
    ];
    let local: std::collections::HashSet<&str> =
        info.types.iter().map(|(n, _)| n.as_str()).collect();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for text in texts {
        for tok in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
            if tok.is_empty()
                || tok == "_"
                || RESERVED.contains(&tok)
                || local.contains(tok)
                || !seen.insert(tok.to_string())
            {
                continue;
            }

            for imp in &info.imports {
                let Some(dep) = deps.get(&imp.package) else {
                    continue;
                };
                let Some((_, di, original)) = dep.type_ifaces.iter().find(|(n, _, _)| n == tok)
                else {
                    continue;
                };
                let path = versioned_iface(&dep.package, di);
                match out.iter_mut().find(|(p, _)| p == &path) {
                    Some((_, names)) => names.push(if original == tok {
                        tok.to_string()
                    } else {
                        format!("{original} as {tok}")
                    }),
                    None => out.push((
                        path,
                        vec![if original == tok {
                            tok.to_string()
                        } else {
                            format!("{original} as {tok}")
                        }],
                    )),
                }
                break;
            }
        }
    }
    out
}

pub(crate) fn synthesize_world_wit(
    arena: &Arena,
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> Result<String, String> {
    let mut out = format!("package {};\n\n", info.package);

    let mut ifaces = crate::wit::iface_order(&info.exports, !info.types.is_empty());

    for r in &info.resources {
        if let Some(iface) = &r.iface
            && !ifaces.contains(iface)
        {
            ifaces.push(iface.clone());
        }
    }

    let hoisted = crate::wit::hoisted_types(arena, info)?;
    if !hoisted.is_empty() {
        out.push_str("interface types {\n");
        let mut texts: Vec<String> = Vec::new();
        for name in &hoisted {
            let (_, ty) = info
                .types
                .iter()
                .find(|(n, _)| n == name)
                .expect("hoisted names come from info.types");
            let d = type_decl(arena, name, *ty)?;
            texts.push(d.clone());
            out.push_str(&format!("  {d}\n"));
        }

        let uses = dep_type_uses(&texts, info, deps);
        if !uses.is_empty() {
            let mut body = String::new();
            for (use_path, names) in &uses {
                body.push_str(&format!("  use {use_path}.{{{}}};\n", names.join(", ")));
            }
            for t in &texts {
                body.push_str(&format!("  {t}\n"));
            }
            let start = out.rfind("interface types {\n").expect("just pushed");
            out.truncate(start);
            out.push_str("interface types {\n");
            out.push_str(&body);
        }
        out.push_str("}\n\n");
    }

    for iface in ifaces.iter().filter(|i| !is_external_iface(i)) {
        let sigs: Vec<&FuncSig> = info.exports.iter().filter(|s| &s.iface == iface).collect();
        let used: Vec<&str> = info
            .functors
            .iter()
            .filter(|f| {
                let dotted = format!("{}.set", f.iface);
                sigs.iter().any(|s| {
                    s.result.as_deref() == Some(dotted.as_str())
                        || s.params.iter().any(|(_, t)| t == &dotted)
                })
            })
            .map(|f| f.iface.as_str())
            .collect();
        out.push_str(&format!("interface {iface} {{\n"));

        for funct in &used {
            out.push_str(&format!("  use {funct}.{{set as {funct}-handle}};\n"));
        }

        let mut texts: Vec<String> = Vec::new();
        for sig in &sigs {
            for (_, t) in &sig.params {
                texts.push(t.clone());
            }
            if let Some(r) = &sig.result {
                texts.push(r.clone());
            }
        }
        let mut api_decls: Vec<String> = Vec::new();
        if iface == "api" {
            if !hoisted.is_empty() {
                out.push_str(&format!("  use types.{{{}}};\n", hoisted.join(", ")));
            }
            for (name, ty) in info.types.iter().filter(|(n, _)| !hoisted.contains(n)) {
                let d = type_decl(arena, name, *ty)?;
                texts.push(d.clone());
                api_decls.push(d);
            }
        }
        for (use_path, names) in dep_type_uses(&texts, info, deps) {
            out.push_str(&format!("  use {use_path}.{{{}}};\n", names.join(", ")));
        }
        for d in &api_decls {
            out.push_str(&format!("  {d}\n"));
        }

        for r in info
            .resources
            .iter()
            .filter(|r| r.iface.as_deref() == Some(iface.as_str()))
        {
            out.push_str(&r.to_wit());
        }
        for sig in &sigs {
            let mut line = sig.to_wit();
            for funct in &used {
                line = line.replace(&format!("{funct}.set"), &format!("{funct}-handle"));
            }
            out.push_str(&format!("  {line}\n"));
        }
        out.push_str("}\n\n");
    }

    for f in &info.functors {
        let elem_iface = if hoisted.contains(&f.elem) {
            Some("types")
        } else if info.types.iter().any(|(n, _)| n == &f.elem) {
            Some("api")
        } else {
            None
        };
        out.push_str(crate::wit::functor_interface(arena, f, elem_iface)?.trim_start());
        out.push('\n');
    }

    out.push_str(&format!("world {} {{\n", info.world));
    for imp in &info.imports {
        if crate::wit::is_macro_only(imp) {
            continue;
        }
        let iface = import_iface(&imp.path);
        let dep = deps.get(&imp.package).ok_or(format!(
            "dependency `{}` is not in the build set",
            imp.package
        ))?;
        out.push_str(&format!(
            "  import {};\n",
            versioned_iface(&dep.package, &iface)
        ));
    }

    if !hoisted.is_empty() {
        out.push_str("  export types;\n");
    }
    for iface in &ifaces {
        if is_external_iface(iface) {
            out.push_str(&format!(
                "  export {};\n",
                external_versioned_in(iface, deps)
            ));
        } else {
            out.push_str(&format!("  export {iface};\n"));
        }
    }

    for f in &info.functors {
        out.push_str(&format!("  export {};\n", f.iface));
    }
    out.push_str("}\n");

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for dep in deps.values() {
        for block in split_package_blocks(&dep.package_wit) {
            let dup = package_block_name(block).is_some_and(|name| !seen.insert(name));
            if !dup {
                out.push_str(block);
            }
        }
    }
    Ok(out)
}

pub(crate) fn split_package_blocks(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut blocks = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    let mut end = i + 1;
                    if end < bytes.len() && bytes[end] == b'\n' {
                        end += 1;
                    }
                    blocks.push(&s[start..end]);
                    start = end;
                    i = end;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if start < s.len() {
        let tail = &s[start..];
        if !tail.trim().is_empty() {
            blocks.push(tail);
        }
    }
    blocks
}

pub(crate) fn package_block_name(block: &str) -> Option<String> {
    let rest = block.trim_start().strip_prefix("package ")?;
    let name: String = rest
        .chars()
        .take_while(|&c| c != '{' && c != ';' && !c.is_whitespace())
        .collect();
    if name.is_empty() { None } else { Some(name) }
}
