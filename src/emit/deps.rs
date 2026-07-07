//! Dependency and interface resolution: collecting record/alias/type
//! definitions from deps and the local file, interface naming/versioning,
//! and dep-function lookup for cross-component calls.

use super::*;

/// Record types from a file's `DefType` forms: name → field (name, type-string).
/// Only record-shaped types are collected here; variants/flags go through
/// [`local_non_record_types`] (into `TypeEnv::defs`) and bare aliases (`list`,
/// `tuple`, …) into `TypeEnv::aliases`, so every `DefType` kind has a boundary
/// ABI — the layouts already exist (`WitTy::List`/`Tuple`/`Variant`/`Flags`).
pub(crate) fn record_types(arena: &Arena, types: &[(String, NodeId)]) -> Vec<(String, Vec<(String, String)>)> {
    let mut out = Vec::new();
    for (name, node) in types {
        if let Node::Rec(fields) = arena.node(*node) {
            let mut fs = Vec::with_capacity(fields.len());
            let mut ok = true;
            for (fname, fnode) in fields {
                match crate::wit::type_text(arena, *fnode) {
                    Ok(t) => fs.push((fname.clone(), t)),
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                out.push((name.clone(), fs));
            }
        }
    }
    out
}

/// Public: record types a dependency file defines, for the build driver to put
/// on its [`Dep`].
pub fn dep_record_types(arena: &Arena, info: &FileInfo) -> Vec<(String, Vec<(String, String)>)> {
    record_types(arena, &info.types)
}

/// Public: non-record named types a sibling dependency file defines —
/// variants/enums/flags as [`TypeDef`]s plus name → type-text aliases — so a
/// sibling `.wlt` dep carries the same type surface a parsed WIT dep does
/// (4.1/4.4: case constructors and alias expansion across the build set).
pub fn dep_non_record_types(
    arena: &Arena,
    info: &FileInfo,
) -> (Vec<(String, TypeDef)>, Vec<(String, String)>) {
    local_non_record_types(arena, &info.types)
}

/// Non-record local `DefType`s, split into the two `TypeEnv` channels:
///   * variants/flags become [`TypeDef`]s (keyed by name) — `Node::Lst` is a
///     `variant` (payload-less cases are an enum, the same as a variant with all
///     `None` payloads — and how `wit::type_decl` already renders them), and
///     `Node::Flg` is a `flags`. This mirrors what `witdep.rs` builds for *dep*
///     type_defs, so a local and an imported variant/flags lower identically.
///   * everything else `wit::type_text` can render — `list<…>`, `tuple<…>`,
///     `option<…>`, `result<…>`, or an alias to another named type — becomes an
///     *alias* (name → WIT type text), which `wit_ty` expands recursively.
///
/// Records are handled by [`record_types`] and skipped here. A `DefType` whose
/// body neither parses as a known kind nor renders to type text is left out
/// (any reference to it still surfaces the honest "not supported" error).
pub(crate) fn local_non_record_types(
    arena: &Arena,
    types: &[(String, NodeId)],
) -> (Vec<(String, TypeDef)>, Vec<(String, String)>) {
    let mut defs = Vec::new();
    let mut aliases = Vec::new();
    for (name, node) in types {
        match arena.node(*node) {
            Node::Rec(_) => {} // records: see `record_types`
            Node::Flg(names) => defs.push((name.clone(), TypeDef::Flags(names.clone()))),
            Node::Lst(cases) => {
                // A `[case …]` form is a variant; a payload carries as a Tup
                // `[head, payload…]` exactly as `wit::type_decl` reads it.
                let mut resolved = Vec::with_capacity(cases.len());
                let mut ok = true;
                for &c in cases {
                    match arena.node(c) {
                        Node::Sym(s) => resolved.push((s.clone(), None)),
                        Node::Tup(items) => {
                            let Some((&h, payload)) = items.split_first() else {
                                ok = false;
                                break;
                            };
                            let Node::Sym(case) = arena.node(h) else {
                                ok = false;
                                break;
                            };
                            // Multi-payload cases would need a tuple payload; the
                            // backend (like the variant ABI) carries one payload
                            // box, so only single-payload cases are supported.
                            match payload {
                                [] => resolved.push((case.clone(), None)),
                                [one] => match crate::wit::type_text(arena, *one) {
                                    Ok(t) => resolved.push((case.clone(), Some(t))),
                                    Err(_) => {
                                        ok = false;
                                        break;
                                    }
                                },
                                _ => {
                                    ok = false;
                                    break;
                                }
                            }
                        }
                        _ => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    // All-payload-less cases are a WIT `enum` (matching what
                    // `wit::type_decl` now synthesizes and what `witdep.rs`
                    // builds for dep enums); any payload makes it a `variant`.
                    // The flat ABI (a lone i32 discriminant) is identical.
                    if resolved.iter().all(|(_, p)| p.is_none()) {
                        let names = resolved.into_iter().map(|(n, _)| n).collect();
                        defs.push((name.clone(), TypeDef::Enum(names)));
                    } else {
                        defs.push((name.clone(), TypeDef::Variant(resolved)));
                    }
                }
            }
            // A bare alias: `list<…>`, `tuple<…>`, `option<…>`, `result<…>`, or a
            // name for another named type. Record its WIT type text for `wit_ty`.
            _ => {
                if let Ok(t) = crate::wit::type_text(arena, *node) {
                    aliases.push((name.clone(), t));
                }
            }
        }
    }
    (defs, aliases)
}

/// `"demo:shout/render"` → `"render"`; a bare package path means `api`.
pub(crate) fn import_iface(path: &str) -> String {
    match path.split_once('/') {
        Some((_, iface)) => iface.to_string(),
        None => "api".to_string(),
    }
}

/// The default version for an external interface whose package isn't resolved
/// to a [`Dep`] (so its pinned version is unknown). External WIT now comes from
/// `wit/deps`, so [`external_versioned_in`] supplies the real version; this is
/// only the fallback.
pub(crate) const WASI_VERSION: &str = "0.2.0";

/// An export/import that names an external WIT interface directly — e.g.
/// `wasi:http/incoming-handler` — rather than a local interface like `api`.
pub(crate) fn is_external_iface(iface: &str) -> bool {
    iface.contains(':')
}

/// Version an external interface path to the version we vendor:
/// `wasi:http/incoming-handler` → `wasi:http/incoming-handler@0.2.0`.
pub(crate) fn external_versioned(path: &str) -> String {
    format!("{path}@{WASI_VERSION}")
}

/// Version an external interface path (`ns:pkg/iface`) using the version of the
/// resolved [`Dep`] for its package, when one is in scope — the generic export
/// path, whose WIT comes from `wit/deps` at whatever version `wkg` pinned. Falls
/// back to [`external_versioned`] (the hardcoded WASI version) for the magic
/// http/cli path, which has no `Dep` for its vendored interfaces.
///
/// `ns:greet/greeter` with a dep `greet` at `acme:greet@0.1.0` → `…@0.1.0`.
pub(crate) fn external_versioned_in(path: &str, deps: &HashMap<String, Dep>) -> String {
    if let Some((pkg, _iface)) = path.split_once('/')
        && let Some(dep) = deps.get(pkg)
        && let Some((_base, ver)) = dep.package.split_once('@')
    {
        return format!("{path}@{ver}");
    }
    external_versioned(path)
}

/// `("demo:shout@0.1.0", "api")` → `"demo:shout/api@0.1.0"`
pub(crate) fn versioned_iface(pkg: &str, iface: &str) -> String {
    match pkg.split_once('@') {
        Some((base, ver)) => format!("{base}/{iface}@{ver}"),
        None => format!("{pkg}/{iface}"),
    }
}

/// The source-visible operation name a (possibly mangled) WIT function name is
/// reached by. A freestanding `f` is called as `f`; a resource operation is
/// called by its *bare op name*:
///
/// - `[constructor]res`      → `res`
/// - `[method]res.op`        → `op`
/// - `[static]res.op`        → `op`
/// - `[resource-drop]res`    → `drop-res`  (synthetic, see [`crate::witdep`])
///
/// So `r/body` resolves to `[method]outgoing-response.body`, `r/fields` to
/// `[constructor]fields`, and `r/drop-output-stream` to
/// `[resource-drop]output-stream`. Drop is spelled `drop-<res>` (not the bare
/// `<res>`) so it never collides with the resource's own constructor.
pub(crate) fn dep_func_op(name: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    if let Some(rest) = name.strip_prefix("[constructor]") {
        return Cow::Borrowed(rest);
    }
    if let Some(rest) = name.strip_prefix("[resource-drop]") {
        return Cow::Owned(format!("drop-{rest}"));
    }
    for prefix in ["[method]", "[static]"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            // `res.op` → `op`
            return Cow::Borrowed(rest.rsplit_once('.').map(|(_, op)| op).unwrap_or(rest));
        }
    }
    Cow::Borrowed(name)
}

/// The *resource-qualified* source name for a resource operation, used to
/// disambiguate when several resources in one interface share a bare op name
/// (e.g. `wasi:http/types` has both `outgoing-request.body` and
/// `outgoing-response.body`). Since a Wavelet qualified name is kebab-only (no
/// `.`), the qualifier joins with `-`:
///
/// - `[method]outgoing-response.body` → `outgoing-response-body`
/// - `[static]response-outparam.set`  → `response-outparam-set`
/// - `[constructor]fields`            → `fields` (same as the bare op)
///
/// A freestanding function or a drop has no qualified form (`None`).
pub(crate) fn dep_func_qualified(name: &str) -> Option<String> {
    if let Some(rest) = name.strip_prefix("[constructor]") {
        return Some(rest.to_string());
    }
    for prefix in ["[method]", "[static]"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            // `res.op` → `res-op`
            return Some(rest.replacen('.', "-", 1));
        }
    }
    None
}

/// Resolve a source-visible op name to the dep's [`FuncSig`] in `iface`.
///
/// Matching is two-tier so that the common bare-op spelling stays terse while
/// collisions stay resolvable:
/// 1. An *exact* match — the mangled WIT name, the resource-qualified
///    `res-op` form ([`dep_func_qualified`]), or a freestanding name — wins
///    outright. This is unique by construction (WIT names are unique per
///    interface), so `outgoing-response-body` selects exactly that method.
/// 2. Otherwise the *bare* op name ([`dep_func_op`]) is tried. If two resources
///    share it, the call is ambiguous and the source must use the qualified
///    form instead.
pub(crate) fn resolve_dep_func<'a>(
    dep: &'a Dep,
    iface: &str,
    fname: &str,
) -> Result<&'a crate::wit::FuncSig, String> {
    let in_iface = || dep.funcs.iter().filter(|f| f.iface == iface);

    // Tier 1: an exact mangled-name / qualified-name / freestanding match.
    if let Some(f) = in_iface()
        .find(|f| f.name == fname || dep_func_qualified(&f.name).as_deref() == Some(fname))
    {
        return Ok(f);
    }

    // Tier 2: the bare op name, rejecting genuine collisions.
    let mut bare = in_iface().filter(|f| dep_func_op(&f.name) == *fname);
    let first = bare.next().ok_or(format!(
        "`{}` does not export `{fname}` in `{iface}`",
        dep.package
    ))?;
    if let Some(second) = bare.next() {
        return Err(format!(
            "`{fname}` is ambiguous in `{}/{iface}`: matches both `{}` and `{}`; \
             use the resource-qualified name (e.g. `{}`)",
            dep.package,
            first.name,
            second.name,
            dep_func_qualified(&first.name).unwrap_or_else(|| first.name.clone()),
        ));
    }
    Ok(first)
}

/// Whether `name` is a case of one of `dep`'s variant/enum types: `Some(true)`
/// for a payloaded variant case, `Some(false)` for a payload-less variant or
/// enum case, `None` when no type declares it (4.1).
pub(crate) fn dep_case(dep: &Dep, name: &str) -> Option<bool> {
    dep.type_defs.iter().find_map(|(_, def)| match def {
        TypeDef::Enum(cases) => cases.iter().any(|c| c == name).then_some(false),
        TypeDef::Variant(cases) => cases
            .iter()
            .find(|(c, _)| c == name)
            .map(|(_, p)| p.is_some()),
        _ => None,
    })
}
