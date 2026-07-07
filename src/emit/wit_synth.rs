use super::*;

// ----------------------------------------------------------- WIT synthesis

/// Render a dependency's nested-package WIT from its parsed surface.
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

/// The `use` clauses a local interface needs for the dep-defined type names
/// its rendered signatures/type declarations reference (4.3): each entry is a
/// versioned interface path (`acme:pts/types@0.3.1`) with the names to bring
/// in. Tokenizes the WIT texts and keeps identifiers that are not primitives,
/// WIT keywords, or locally-declared types, and that some imported dependency
/// declares (records, variants/enums/flags, aliases, resources alike).
pub(crate) fn dep_type_uses(
    texts: &[String],
    info: &FileInfo,
    deps: &HashMap<String, Dep>,
) -> Vec<(String, Vec<String>)> {
    /// primitives, type constructors, and declaration keywords that can appear
    /// in rendered WIT type text — never dep type names.
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
            // The first imported dependency declaring this name wins.
            for imp in &info.imports {
                let Some(dep) = deps.get(&imp.package) else {
                    continue;
                };
                let Some((_, di)) = dep.type_ifaces.iter().find(|(n, _)| n == tok) else {
                    continue;
                };
                let path = versioned_iface(&dep.package, di);
                match out.iter_mut().find(|(p, _)| p == &path) {
                    Some((_, names)) => names.push(tok.to_string()),
                    None => out.push((path, vec![tok.to_string()])),
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
    // A resource-only export (4.5) still needs its placement interface present.
    // External-interface resource exports are defined by the dependency's WIT and
    // never re-declared here, so only fold in *internal* placement interfaces.
    for r in &info.resources {
        if let Some(iface) = &r.iface
            && !is_external_iface(iface)
            && !ifaces.contains(iface)
        {
            ifaces.push(iface.clone());
        }
    }

    // Hoisted local types (4.7). An export that returns (or takes) a functor
    // handle makes its interface `use` the functor interface; when that
    // functor's element is a local record, the functor interface would `use`
    // the record back from `api` — a WIT interface cycle, which WIT cannot
    // express. Break it by hoisting the element record (and any local types
    // its declaration references, transitively) into a shared `types`
    // interface that both `api` and the functor interface `use`.
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
        // …their declarations may themselves reference dep types (4.3).
        // (Re-rendered inside the loop; collect first to emit uses on top.)
        let uses = dep_type_uses(&texts, info, deps);
        if !uses.is_empty() {
            // `use` lines must be re-emitted before the decls: rebuild.
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

    // External interfaces (e.g. wasi:http/incoming-handler, wasi:cli/run) are
    // defined by the dependency's WIT; we only export them by name, never
    // re-declare them here.
    for iface in ifaces.iter().filter(|i| !is_external_iface(i)) {
        // An export whose signature references a functor handle gets it as the
        // dotted `<funct-iface>.set` text (from `wit::functor_op_table`). WIT does
        // not accept an inline dotted type reference; the type must be `use`-d
        // from its interface and then named bare. Detect which functor interfaces
        // an interface's signatures reference and emit a `use <funct>.{set};` for
        // each, rewriting the dotted occurrences in the signatures to bare `set`.
        // (The functor interface is declared later in the same package; WIT `use`
        // resolves forward references within a package.)
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
        // Each functor interface names its resource `set`, so an interface that
        // references *two* functor handles (two instantiations, both returned or
        // taken by exports) would `use` two types both called `set` — a WIT
        // "defined more than once" collision. Alias each `use` to a per-functor
        // name (`set as <iface>-handle`) and rewrite the dotted `<iface>.set`
        // occurrences in the signatures to that alias. A single functor still
        // reads naturally; multiple instantiations no longer collide. (The alias
        // only renames the WIT type binding — the handle still lowers to one i32.)
        for funct in &used {
            out.push_str(&format!("  use {funct}.{{set as {funct}-handle}};\n"));
        }
        // Cross-package type references (4.3): a signature (or a local type
        // declaration) may name a type a dependency's interface defines. WIT
        // requires such names be brought into scope with a `use`, so collect
        // every dep-defined name the interface's text references and emit
        // `use <pkg>/<iface>@<ver>.{names};` per defining interface.
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
            // Hoisted element types (4.7) are declared in `types` and brought
            // back into scope here; the rest declare in place as before.
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
        // Exported user-declared resource blocks (4.5) land in their placement
        // interface. External-iface resources are defined by the dependency WIT
        // (filtered out above), so only internal placements reach here.
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

    // Functor instantiations stamp out a specialized, monomorphic interface each
    // (Steps 10–11), rendered from the SAME `SET_OPS` source as `wavelet wit`
    // (`wit::functor_interface`) so the WIT the encoder validates against and the
    // resource the wasm backend implements cannot drift.
    for f in &info.functors {
        // The element's declaring interface: `types` once hoisted (4.7), `api`
        // for an un-hoisted local type, none for a primitive element.
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
        // A pure macro import (§6.3) is compile-time only: it is resolved to a
        // macro component and run during expansion, contributing no runtime
        // import to the synthesized world. Skip it here (mirroring `build`'s
        // dep-resolution skip) so a file that uses foreign macros but no runtime
        // dependency from that package still synthesizes a valid world.
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
    // The hoisted `types` interface (4.7) is exported so the interfaces that
    // `use` it resolve in the encoded component.
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
    // Each functor instantiation exports its specialized interface (so the
    // encoder synthesizes the `[resource-new/rep/drop]set` intrinsics the core
    // module imports — they only appear when the world *exports* the resource).
    for f in &info.functors {
        out.push_str(&format!("  export {};\n", f.iface));
    }
    out.push_str("}\n");

    // Append each dep's nested-package WIT, but emit any given package only once.
    // A `wit/deps` dep carries its whole transitive closure (e.g. both the
    // `wasi:http` and `wasi:io/streams` deps render `wasi:io`, `wasi:clocks`,
    // …), so concatenating them verbatim would define a package twice.
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

/// Split a concatenation of top-level `package NAME { … }` blocks (and any
/// leading flat `package NAME;` lines) into individual block slices, splitting
/// on brace balance returning to zero. Text that isn't a braced package block
/// (e.g. a trailing `package x;` line) is returned as its own slice.
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
                    // include the trailing newline if present
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

/// The `ns:name@ver` of a `package NAME { … }` or `package NAME;` block, if it
/// starts with the `package` keyword.
pub(crate) fn package_block_name(block: &str) -> Option<String> {
    let rest = block.trim_start().strip_prefix("package ")?;
    let name: String = rest
        .chars()
        .take_while(|&c| c != '{' && c != ';' && !c.is_whitespace())
        .collect();
    if name.is_empty() { None } else { Some(name) }
}
