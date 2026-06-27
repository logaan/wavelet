//! Resolve a `macros: true` import to a `.wasm` macro component and instantiate
//! it (design.md §6.3).
//!
//! A `macros: true` import (Step 4 — see [`crate::wit::ImportInfo`]) names a
//! *package* whose macros we want to run at compile time. To run them we need
//! the **compiled component** that exports `wavelet:meta/macros`, not just its
//! WIT. This module locates that binary on disk, loads it via the Step 2/3
//! runtime ([`crate::macros::MacroComponent`]), and caches the instance so a
//! package imported once is instantiated once per build.
//!
//! ## Resolution strategy (MVP: explicit local path, smallest viable first)
//!
//! Macro components are *executable artifacts*, and the project's existing
//! dependency machinery only knows how to fetch dependency **WIT** into
//! `wit/deps` (`wkg wit fetch`). `wkg` does **not** fetch components today, so
//! registry-fetch of a macro component is deferred (tracked in
//! `dev-notes/gaps.typ`). For now a project points an import at a locally built
//! macro component, resolved in this order:
//!
//! 1. **Explicit `from:` path** — `Import {pkg: "acme:html/dsl" macros: true
//!    from: "path/to/macros.wasm"}`. The path is taken relative to the project
//!    root (the parent of `src/`) when relative, so it is stable regardless of
//!    the build's working directory.
//! 2. **Conventional location** — `wit/macros/<ns>-<name>.wasm` under the
//!    project root, where `<ns>-<name>` is the import's package path with `:`
//!    and `/` mapped to `-` (e.g. `acme:html/dsl` → `acme-html-dsl.wasm`). This
//!    lets a project drop a macro component in a well-known place with no
//!    `from:` field.
//!
//! This is build-time-only, native-only infrastructure: like
//! `host`/`macros`/`emit`/`build`, it is gated `#[cfg(not(target_arch =
//! "wasm32"))]` in `lib.rs` and must never reach the docs-playground build.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::form::{Arena, Node, NodeId};
use crate::lexer::ReadError;
use crate::macros::MacroComponent;
use crate::reader::{FormHook, MacroTable};
use crate::wit::ImportInfo;

/// A per-build cache of instantiated macro components, keyed by the import's
/// package path (e.g. `acme:html/dsl`).
///
/// A package imported once is instantiated once: repeated lookups for the same
/// package id return the already-loaded [`MacroComponent`]. The cache is keyed
/// by *package path* (version-stripped, as carried in
/// [`ImportInfo::package`])`, so two imports of the same package — even under
/// different aliases — share one instance.
///
/// `root` is the project root used to resolve relative `from:` paths and the
/// conventional `wit/macros/` location. Construct with [`MacroResolver::new`].
#[derive(Default)]
pub struct MacroResolver {
    root: PathBuf,
    cache: HashMap<String, MacroComponent>,
}

impl MacroResolver {
    /// Create a resolver rooted at `root` (the project root — the parent of
    /// `src/`). Relative `from:` paths and the conventional `wit/macros/`
    /// location are resolved against it.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        MacroResolver { root: root.into(), cache: HashMap::new() }
    }

    /// Resolve and instantiate the macro component for a `macros: true` import,
    /// returning a mutable handle to the cached instance.
    ///
    /// The first call for a given package locates the `.wasm` (per the strategy
    /// in the module docs), loads it as a [`MacroComponent`] (which verifies it
    /// actually exports `wavelet:meta/macros`), and caches it under the import's
    /// package path. Later calls for the same package return the cached instance
    /// without re-instantiating.
    ///
    /// Errors are actionable: a `macros: true` import with no resolvable binary
    /// names the locations searched; a binary that isn't a macro component
    /// surfaces the [`MacroComponent`] interface-check error.
    pub fn resolve(&mut self, import: &ImportInfo) -> Result<&mut MacroComponent, String> {
        debug_assert!(
            import.macros,
            "resolve() is only for `macros: true` imports"
        );
        let key = import.package.clone();
        if !self.cache.contains_key(&key) {
            let path = self.locate(import)?;
            let comp = MacroComponent::from_file(&path).map_err(|e| {
                format!(
                    "import `{}` (macros): {} ({})",
                    import.path,
                    e,
                    path.display()
                )
            })?;
            self.cache.insert(key.clone(), comp);
        }
        Ok(self.cache.get_mut(&key).expect("just inserted"))
    }

    /// Locate the `.wasm` for a macro import on disk, trying the explicit
    /// `from:` path first, then the conventional `wit/macros/<ns>-<name>.wasm`.
    fn locate(&self, import: &ImportInfo) -> Result<PathBuf, String> {
        let mut tried = Vec::new();

        // 1. Explicit `from:` path, relative to the project root when relative.
        if let Some(from) = &import.from {
            let p = self.resolve_relative(from);
            if p.is_file() {
                return Ok(p);
            }
            tried.push(p.display().to_string());
        }

        // 2. Conventional `wit/macros/<ns>-<name>.wasm`.
        let conventional = self
            .root
            .join("wit")
            .join("macros")
            .join(format!("{}.wasm", import.package.replace([':', '/'], "-")));
        if conventional.is_file() {
            return Ok(conventional);
        }
        tried.push(conventional.display().to_string());

        Err(format!(
            "import `{}` is `macros: true` but no macro component was found. \
             Build the macro library to a component and either set `from: \
             \"<path>.wasm\"` on the import or place it at \
             `wit/macros/{}.wasm`. Searched: {}",
            import.path,
            import.package.replace([':', '/'], "-"),
            tried.join(", ")
        ))
    }

    /// Resolve a possibly-relative path against the project root. An absolute
    /// path is taken as-is.
    fn resolve_relative(&self, p: &str) -> PathBuf {
        let path = Path::new(p);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        }
    }

    /// Register the foreign macro arities for a single just-read top-level form,
    /// if it is an `Import {… macros: true}` (§6.3). Used by the reader hook in
    /// [`register_macro_imports`]: the import is resolved + instantiated, its
    /// `manifest()` is called, and each `(name, arity)` pair is registered as
    /// `<name>-MACRO` into `macros` — mirroring the reader's local-`DefMacro`
    /// registration so later TitleCase uses in the same file read with the right
    /// arity. Non-import forms and runtime (non-`macros`) imports are no-ops.
    ///
    /// Errors are surfaced as a [`ReadError`] tied to the import form's span, so
    /// a failure to instantiate the component or a trapping `manifest()` reads
    /// as an actionable read-time error rather than a generic reader failure.
    fn register_form(
        &mut self,
        arena: &Arena,
        id: NodeId,
        macros: &mut MacroTable,
    ) -> Result<(), ReadError> {
        let Some(import) = parse_macro_import(arena, id) else {
            return Ok(());
        };
        let span = arena.span(id);
        let comp = self
            .resolve(&import)
            .map_err(|msg| read_err(msg, span.0))?;
        let manifest = comp.manifest().map_err(|e| {
            read_err(
                format!("import `{}` (macros): manifest() failed: {e}", import.path),
                span.0,
            )
        })?;
        // Register each foreign macro under BOTH its qualified `Alias/Name` key
        // (always resolvable, even under collisions) and its bare `<name>-MACRO`
        // name (subject to collision detection in `MacroTable`). The import's
        // `as:` alias (§4 / `ImportInfo`) keys the qualified entries, so a
        // qualified TitleCase head `Dsl/Element` resolves to this import's macro
        // even when the bare name is ambiguous (§6.3).
        for (name, arity) in manifest {
            macros.register_foreign(&import.alias, format!("{name}-MACRO"), arity as usize);
        }
        Ok(())
    }
}

/// The foreign-macro half of the ahead-of-time expander (Step 7): given a macro
/// head name and a call form, run the owning component's `expand` and lift the
/// result back into a fresh arena.
///
/// This is the native implementation of [`crate::expand::ForeignExpander`], the
/// wasm-safe seam `expand.rs` calls through. It owns a [`MacroResolver`] (the
/// per-build cache of instantiated components) plus the file's `macros: true`
/// imports, parsed once up front. Macro components are *not* carried forward
/// from the read phase (the read-time resolver is local to `read_file_with_macros`
/// and dropped after reading — see Step 6's handoff), so a fresh
/// [`FileExpander`] re-resolves from the file's imports at expand time. Because
/// the resolver caches per package, re-resolution instantiates each component at
/// most once.
///
/// ## The args-tree contract (PINNED — shared with Steps 3/9)
///
/// `expand_call` ships the **whole call form** — a `tup` whose element 0 is the
/// macro head (`<name>-MACRO`) and elements 1.. are the argument forms — across
/// the boundary as the `args` `tree`. The guest reads `args.nodes[args.root]` as
/// a `tup` and indexes its arguments from element 1 (the fixture in
/// `tests/fixtures/macros/src/lib.rs` does exactly this). The head symbol still
/// carries its `-MACRO` suffix in the shipped tree; the macro `name` passed
/// alongside is the *unsuffixed* manifest name.
pub struct FileExpander {
    resolver: MacroResolver,
    /// The file's `macros: true` imports, parsed once.
    imports: Vec<ImportInfo>,
    /// Lazily-built map from unsuffixed macro name to the index in `imports` of
    /// the import whose component owns it. `None` for a name confirmed *not* to
    /// be owned by any imported macro component (so the expander stops probing).
    owners: HashMap<String, Option<usize>>,
    /// Same memoisation for qualified `alias/name` lookups, keyed by the
    /// `(alias, name)` pair so a repeated qualified call doesn't re-`manifest()`
    /// the component on every probe.
    alias_owners: HashMap<(String, String), Option<usize>>,
    /// The file's own local macros (`DefMacro`s), by unsuffixed name. On the
    /// native path these are expanded through a *compiled* component
    /// ([`local`]) — strategy B — rather than the interpreter, so the native
    /// production expander uses no `Interp::expand_once`. The wasm playground
    /// (which passes no [`FileExpander`]) keeps the interpreter local arm.
    local_names: std::collections::HashSet<String>,
    /// The file's `DefMacro`s compiled into a `wavelet:meta/macros` component,
    /// produced eagerly in [`for_file`] (so an emit failure is captured) but
    /// instantiated lazily in [`local_component`] (so a file that never expands
    /// a local macro never spins up a runtime).
    local_bytes: Option<Result<Vec<u8>, String>>,
    local: Option<MacroComponent>,
}

impl FileExpander {
    /// Build a [`FileExpander`] for a file, scanning `roots` for its
    /// `macros: true` imports. `root` is the project root used to resolve each
    /// import's `.wasm` (the parent of `src/`; see [`MacroResolver::new`]).
    ///
    /// Returns `None` only when the file has **neither** `macros: true` imports
    /// **nor** local `DefMacro`s — the common no-macro path, where the caller
    /// expands with no foreign capability and never instantiates a runtime. When
    /// the file defines local macros, the returned expander compiles them once
    /// (strategy B) so the native path expands them as wasm rather than through
    /// the interpreter.
    pub fn for_file(
        root: impl Into<PathBuf>,
        arena: &Arena,
        roots: &[NodeId],
    ) -> Option<FileExpander> {
        let imports: Vec<ImportInfo> = roots
            .iter()
            .filter_map(|&id| parse_macro_import(arena, id))
            .collect();
        let local_names: std::collections::HashSet<String> = roots
            .iter()
            .filter_map(|&id| local_macro_name(arena, id))
            .collect();
        if imports.is_empty() && local_names.is_empty() {
            return None;
        }
        // Compile the file's DefMacros into a component up front (in-process and
        // cheap) so any emit error is captured; instantiate only on first use.
        let local_bytes = if local_names.is_empty() {
            None
        } else {
            Some(crate::emit::emit_macro_component(arena, roots))
        };
        Some(FileExpander {
            resolver: MacroResolver::new(root),
            imports,
            owners: HashMap::new(),
            alias_owners: HashMap::new(),
            local_names,
            local_bytes,
            local: None,
        })
    }

    /// The instantiated local-macro component, built lazily from the bytes
    /// emitted in [`for_file`]. Surfaces an emit/instantiate failure as the macro
    /// author's error.
    fn local_component(&mut self) -> Result<&mut MacroComponent, String> {
        if self.local.is_none() {
            let bytes = match &self.local_bytes {
                Some(Ok(b)) => b,
                Some(Err(e)) => return Err(e.clone()),
                None => return Err("internal: no local macros to compile".to_string()),
            };
            self.local = Some(MacroComponent::from_bytes(bytes)?);
        }
        Ok(self.local.as_mut().expect("just built"))
    }

    /// Find the index of the import whose macro component publishes `name`,
    /// resolving and querying `manifest()` on demand and memoising the answer.
    ///
    /// Returns `Ok(Some(i))` if import `i` owns `name`, `Ok(None)` if no
    /// imported macro component publishes it, or `Err` if resolving/manifesting
    /// a macro component failed.
    fn owner_of(&mut self, name: &str) -> Result<Option<usize>, String> {
        if let Some(found) = self.owners.get(name) {
            return Ok(*found);
        }
        let mut found = None;
        for i in 0..self.imports.len() {
            // `resolve` is cached per package, so this re-instantiates nothing
            // already loaded during read or an earlier lookup.
            let import = &self.imports[i];
            let comp = self.resolver.resolve(import)?;
            let manifest = comp.manifest()?;
            if manifest.iter().any(|(n, _)| n == name) {
                found = Some(i);
                break;
            }
        }
        self.owners.insert(name.to_string(), found);
        Ok(found)
    }

    /// Find the import bound to `alias` that publishes `name`. Returns
    /// `Ok(Some(i))` if import `i` is aliased `alias` and its component owns
    /// `name`, `Ok(None)` otherwise. Used to route a qualified `Alias/Name`
    /// head to one specific import (bypassing bare-name ambiguity).
    fn owner_for_alias(&mut self, alias: &str, name: &str) -> Result<Option<usize>, String> {
        let key = (alias.to_string(), name.to_string());
        if let Some(found) = self.alias_owners.get(&key) {
            return Ok(*found);
        }
        let mut found = None;
        for i in 0..self.imports.len() {
            if self.imports[i].alias != alias {
                continue;
            }
            let import = &self.imports[i];
            let comp = self.resolver.resolve(import)?;
            let manifest = comp.manifest()?;
            if manifest.iter().any(|(n, _)| n == name) {
                found = Some(i);
                break;
            }
        }
        self.alias_owners.insert(key, found);
        Ok(found)
    }
}

/// The unsuffixed name of a top-level `DefMacro` form
/// (`Tup[defmacro-MACRO, Sym(name), {params}, body]`), or `None` for any other
/// form. Mirrors how `macrolib::manifest` reads a macro's name.
fn local_macro_name(arena: &Arena, id: NodeId) -> Option<String> {
    let Node::Tup(items) = arena.node(id) else { return None };
    if items.len() != 4 {
        return None;
    }
    let Node::Sym(head) = arena.node(items[0]) else { return None };
    if head != "defmacro-MACRO" {
        return None;
    }
    match arena.node(items[1]) {
        Node::Sym(name) => Some(name.clone()),
        _ => None,
    }
}

impl crate::expand::ForeignExpander for FileExpander {
    fn expand_call(
        &mut self,
        alias: Option<&str>,
        name: &str,
        arena: &Arena,
        call_id: NodeId,
    ) -> Option<Result<(Arena, NodeId), String>> {
        // A local macro (bare head only — `DefMacro` registers a bare name)
        // expands through the file's *compiled* component, taking precedence
        // over foreign imports (mirroring the interpreter's local-first lookup).
        if alias.is_none() && self.local_names.contains(name) {
            let args = crate::meta::arena_to_tree(arena, call_id);
            let comp = match self.local_component() {
                Ok(c) => c,
                Err(e) => return Some(Err(e)),
            };
            return Some(
                comp.expand(name, &args)
                    .map(|tree| crate::meta::tree_to_arena(&tree)),
            );
        }
        // Which imported component owns this macro? For a qualified head, route
        // strictly to the import bound to `alias`; for a bare head, scan all
        // imports. `None` => not foreign; fall through to local-macro handling.
        let lookup = match alias {
            Some(a) => self.owner_for_alias(a, name),
            None => self.owner_of(name),
        };
        let owner = match lookup {
            Ok(Some(i)) => i,
            Ok(None) => return None,
            Err(e) => return Some(Err(e)),
        };
        // PINNED contract: ship the whole call form as the `args` tree.
        let args = crate::meta::arena_to_tree(arena, call_id);
        let import = &self.imports[owner];
        let comp = match self.resolver.resolve(import) {
            Ok(c) => c,
            Err(e) => return Some(Err(e)),
        };
        Some(
            comp.expand(name, &args)
                .map(|tree| crate::meta::tree_to_arena(&tree)),
        )
    }
}

/// Read `src` like [`crate::read_file`], but registering the `manifest()`
/// arities of every `macros: true` import as the reader reads top-to-bottom, so
/// foreign TitleCase macros read with the correct arity (Step 6). `root` is the
/// project root used to resolve each import's macro component (the parent of
/// `src/`; see [`MacroResolver::new`]).
///
/// This is the native compiler's read entry point: it threads a fresh
/// [`MacroResolver`] into the reader's [`FormHook`]. The wasm playground keeps
/// calling [`crate::read_file`] (no hook, no runtime), so it is unaffected. A
/// file with no `macros: true` import resolves nothing and reads exactly as
/// [`crate::read_file`] would.
pub fn read_file_with_macros(
    src: &str,
    root: impl Into<PathBuf>,
) -> Result<(Arena, Vec<NodeId>), ReadError> {
    let mut resolver = MacroResolver::new(root);
    let mut macros = MacroTable::core();
    let mut hook = register_macro_imports(&mut resolver);
    crate::reader::read_with_hook(src, &mut macros, Some(hook.as_mut()))
}

/// Build a reader [`FormHook`] backed by `resolver` that registers each
/// `macros: true` import's `manifest()` arities as the reader reads top-to-bottom
/// (Step 6). Pass the returned closure to
/// [`crate::reader::read_with_hook`]; the native compiler uses this so foreign
/// TitleCase macros read with the correct arity, exactly like local ones. The
/// playground (wasm32) has no component runtime, supplies no hook, and so simply
/// reads without foreign registration.
///
/// This is the *inline* shape (over a pre-scan): registration happens the moment
/// the reader finishes an `Import` form, faithful to top-to-bottom semantics and
/// analogous to the reader's own `register_if_def_macro`. Imports must precede
/// their uses (§2.4/§6.1 already require this), so an inline hook always
/// registers a foreign macro before any later form can consume it.
pub fn register_macro_imports<'a>(resolver: &'a mut MacroResolver) -> Box<FormHook<'a>> {
    Box::new(move |arena, id, macros| resolver.register_form(arena, id, macros))
}

/// Parse a single top-level form into an [`ImportInfo`] iff it is an
/// `Import {… macros: true}` record form. Returns `None` for any other form —
/// including a bare-string or non-`macros` import — so only macro imports drive
/// foreign arity registration.
///
/// This mirrors the `import-MACRO` record branch of [`crate::wit::collect`]; it
/// is duplicated rather than shared because `collect` runs over a *whole*
/// already-read file, whereas the reader hook needs one form at a time, before
/// the rest of the file is read.
fn parse_macro_import(arena: &Arena, id: NodeId) -> Option<ImportInfo> {
    let Node::Tup(items) = arena.node(id) else { return None };
    let head = *items.first()?;
    let Node::Sym(head_name) = arena.node(head) else { return None };
    if head_name != "import-MACRO" {
        return None;
    }
    let Node::Rec(fields) = arena.node(*items.get(1)?) else { return None };

    let mut pkg = None;
    let mut alias = None;
    let mut macros = false;
    let mut from = None;
    for (k, v) in fields {
        match (k.as_str(), arena.node(*v)) {
            ("pkg", Node::Str(s)) => pkg = Some(s.clone()),
            ("as", Node::Sym(s)) => alias = Some(s.clone()),
            ("macros", Node::Bool(b)) => macros = *b,
            ("from", Node::Str(s)) => from = Some(s.clone()),
            _ => {}
        }
    }
    if !macros {
        return None;
    }
    let pkg_str = pkg?;
    let path = pkg_str.split('@').next().unwrap_or(&pkg_str).to_string();
    let package = path.split('/').next().unwrap_or(&path).to_string();
    let alias = alias.unwrap_or_else(|| path.rsplit('/').next().unwrap_or(&path).to_string());
    Some(ImportInfo { path, package, alias, macros, from })
}

/// A read-time error tied to a source offset.
fn read_err(msg: impl Into<String>, at: u32) -> ReadError {
    ReadError { msg: msg.into(), at }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `MacroComponent` wraps non-`Debug` `wasmtime` handles, so `expect_err`
    /// (which needs `Ok: Debug`) won't compile. Pull the error out by hand.
    fn resolve_err(r: &mut MacroResolver, import: &ImportInfo) -> String {
        match r.resolve(import) {
            Ok(_) => panic!("expected resolve to fail, but it succeeded"),
            Err(e) => e,
        }
    }

    /// Path to the checked-in fixture macro component (the same one
    /// `macros.rs` round-trips against).
    fn fixture_path() -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/macros.wasm"
        ))
    }

    fn macros_import(from: Option<&str>) -> ImportInfo {
        ImportInfo {
            path: "acme:html/dsl".to_string(),
            package: "acme:html".to_string(),
            alias: "dsl".to_string(),
            macros: true,
            from: from.map(|s| s.to_string()),
        }
    }

    #[test]
    fn resolves_macro_component_from_explicit_path() {
        // Root is irrelevant when `from:` is absolute.
        let mut r = MacroResolver::new(".");
        let import = macros_import(Some(fixture_path().to_str().unwrap()));
        let comp = r.resolve(&import).expect("fixture resolves");
        let manifest = comp.manifest().expect("manifest call");
        // The fixture publishes identity/1, unless/2, boom/0.
        assert_eq!(manifest.len(), 3);
        assert!(manifest.iter().any(|(n, a)| n == "identity" && *a == 1));
    }

    #[test]
    fn caches_one_instance_per_package() {
        let mut r = MacroResolver::new(".");
        let import = macros_import(Some(fixture_path().to_str().unwrap()));
        r.resolve(&import).expect("first resolve");
        assert_eq!(r.cache.len(), 1);
        // A second import of the *same package* (different alias) reuses the
        // cached instance rather than instantiating again.
        let mut second = macros_import(Some(fixture_path().to_str().unwrap()));
        second.alias = "html".to_string();
        r.resolve(&second).expect("second resolve");
        assert_eq!(r.cache.len(), 1, "same package must not re-instantiate");
    }

    #[test]
    fn resolves_from_conventional_location() {
        // Lay out a fake project root with `wit/macros/<ns>-<name>.wasm`.
        let tmp = std::env::temp_dir().join(format!(
            "wavelet-macrodep-{}-{}",
            std::process::id(),
            line!()
        ));
        let macros_dir = tmp.join("wit").join("macros");
        std::fs::create_dir_all(&macros_dir).unwrap();
        let dest = macros_dir.join("acme-html.wasm");
        std::fs::copy(fixture_path(), &dest).unwrap();

        let mut r = MacroResolver::new(&tmp);
        let import = macros_import(None); // no `from:` — use the convention.
        let comp = r.resolve(&import).expect("conventional path resolves");
        comp.manifest().expect("manifest call");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn missing_binary_is_actionable() {
        let mut r = MacroResolver::new("/no/such/project");
        let import = macros_import(Some("does-not-exist.wasm"));
        let err = resolve_err(&mut r, &import);
        assert!(err.contains("macros: true"), "unexpected error: {err}");
        assert!(err.contains("does-not-exist.wasm"), "should name the from path: {err}");
        assert!(err.contains("wit/macros/acme-html.wasm"), "should name the convention: {err}");
    }

    #[test]
    fn non_macro_component_is_rejected() {
        // `add.wasm` is a valid component but not a macro library.
        let add = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/add.wasm"
        ));
        let mut r = MacroResolver::new(".");
        let import = macros_import(Some(add.to_str().unwrap()));
        let err = resolve_err(&mut r, &import);
        // The MacroComponent interface check reports the missing interface.
        assert!(
            err.contains("wavelet:meta/macros") || err.contains("does not export"),
            "unexpected error: {err}"
        );
    }

    // -- Step 6: foreign manifest() arities register with the reader -----------

    /// A source file that imports the fixture macro component (by absolute
    /// `from:` path so it resolves regardless of cwd) and then uses one of its
    /// macros. `{tail}` is appended after the import line.
    fn src_importing_fixture(tail: &str) -> String {
        format!(
            "Package \"demo:app@0.1.0\"\n\
             Import {{pkg: \"acme:html/dsl\" macros: true from: \"{}\"}}\n\
             {tail}\n",
            fixture_path().to_str().unwrap()
        )
    }

    /// Read with foreign-macro registration, rooted at the manifest dir (the
    /// `from:` paths in these tests are absolute, so the root is irrelevant).
    fn read(src: &str) -> Result<(Arena, Vec<NodeId>), ReadError> {
        read_file_with_macros(src, env!("CARGO_MANIFEST_DIR"))
    }

    /// Without registration, a paren-free foreign TitleCase macro is "unknown".
    #[test]
    fn foreign_macro_is_unknown_without_registration() {
        let src = "Package \"demo:app@0.1.0\"\nUnless false \"ran\"\n";
        let err = crate::reader::read_file(src).expect_err("Unless is unknown");
        assert!(
            err.msg.contains("unknown macro") && err.msg.contains("unless-MACRO"),
            "unexpected error: {}",
            err.msg
        );
    }

    /// A ≥2-arity foreign macro reads paren-free, consuming exactly arity forms.
    #[test]
    fn foreign_arity_two_macro_reads_paren_free() {
        let src = src_importing_fixture("Unless false \"ran\"");
        let (arena, roots) = read(&src).expect("reads with foreign arity");
        // roots: [Package…, Import…, the Unless form].
        let last = *roots.last().unwrap();
        // `Unless false "ran"` -> `(unless-MACRO, false, "ran")` — the head plus
        // exactly two consumed following forms.
        assert_eq!(crate::printer::print(&arena, last), r#"(unless-MACRO, false, "ran")"#);
    }

    /// A 0-arity foreign macro reads paren-free, consuming nothing after it; the
    /// following form stays a separate top-level root.
    #[test]
    fn foreign_arity_zero_macro_consumes_nothing() {
        let src = src_importing_fixture("Boom\n42");
        let (arena, roots) = read(&src).expect("reads zero-arity foreign macro");
        // roots: [Package…, Import…, Boom, 42] — Boom did not swallow the 42.
        assert_eq!(roots.len(), 4, "Boom must not consume the following form");
        assert_eq!(crate::printer::print(&arena, roots[2]), "(boom-MACRO)");
        assert_eq!(crate::printer::print(&arena, roots[3]), "42");
    }

    /// A 1-arity foreign macro reads paren-free too (sanity over the full set).
    #[test]
    fn foreign_arity_one_macro_reads_paren_free() {
        let src = src_importing_fixture("Identity add(1 2)");
        let (arena, roots) = read(&src).expect("reads one-arity foreign macro");
        let last = *roots.last().unwrap();
        assert_eq!(crate::printer::print(&arena, last), "(identity-MACRO, (add, 1, 2))");
    }

    /// An explicit-payload spelling reads identically (§2.4) once registered.
    #[test]
    fn foreign_macro_explicit_payload_reads() {
        let src = src_importing_fixture(r#"Unless(false "ran")"#);
        let (arena, roots) = read(&src).expect("reads explicit-payload foreign macro");
        let last = *roots.last().unwrap();
        assert_eq!(crate::printer::print(&arena, last), r#"(unless-MACRO, false, "ran")"#);
    }

    /// A failed resolve surfaces as a read-time error tied to the import, not a
    /// generic reader failure.
    #[test]
    fn unresolvable_macro_import_errors_at_read_time() {
        let src = "Package \"demo:app@0.1.0\"\n\
                   Import {pkg: \"acme:html/dsl\" macros: true from: \"nope.wasm\"}\n";
        let err = read_file_with_macros(src, "/no/such/project").expect_err("resolve fails");
        assert!(err.msg.contains("macros: true"), "unexpected error: {}", err.msg);
        assert!(err.msg.contains("nope.wasm"), "should name the from path: {}", err.msg);
    }

    /// A non-`macros` import registers nothing, so its package is not treated as
    /// a macro library and a later TitleCase use is still "unknown".
    #[test]
    fn non_macro_import_registers_nothing() {
        let src = "Package \"demo:app@0.1.0\"\n\
                   Import \"acme:html/dsl\"\n\
                   Unless false \"ran\"\n";
        let err = read_file_with_macros(src, env!("CARGO_MANIFEST_DIR"))
            .expect_err("non-macro import does not register Unless");
        assert!(err.msg.contains("unknown macro"), "unexpected error: {}", err.msg);
    }

    // -- Step 8: qualified references, aliasing, collisions ---------------------

    /// Import the *same* fixture macro component under two distinct packages /
    /// aliases (`dsl` and `web`), both pointing at the same `.wasm`. Both
    /// publish `unless`/`identity`/`boom`, so every bare name collides while the
    /// qualified `dsl/…` and `web/…` keys both resolve. `{tail}` follows.
    fn src_two_aliases(tail: &str) -> String {
        let p = fixture_path();
        let p = p.to_str().unwrap();
        format!(
            "Package \"demo:app@0.1.0\"\n\
             Import {{pkg: \"acme:html/dsl\" macros: true from: \"{p}\"}}\n\
             Import {{pkg: \"other:html/web\" macros: true from: \"{p}\"}}\n\
             {tail}\n"
        )
    }

    /// Two imports exporting a same-named macro → a *bare* use is ambiguous and
    /// errors actionably (names both aliases, suggests qualify/alias).
    #[test]
    fn colliding_bare_use_is_ambiguous_error() {
        let src = src_two_aliases("Unless false \"ran\"");
        let err = read(&src).expect_err("bare Unless is ambiguous across two imports");
        assert!(err.msg.contains("ambiguous"), "unexpected error: {}", err.msg);
        assert!(err.msg.contains("unless"), "should name the macro: {}", err.msg);
        // Actionable: mentions both qualified spellings / aliasing.
        assert!(
            err.msg.contains("dsl/unless") && err.msg.contains("web/unless"),
            "should suggest qualified spellings: {}",
            err.msg
        );
    }

    /// A collision makes only the bare name ambiguous — the qualified heads
    /// `dsl/Unless` and `web/Unless` both still read with the right arity.
    #[test]
    fn qualified_use_resolves_despite_collision() {
        let src = src_two_aliases("dsl/Unless false \"a\"\nweb/Unless true \"b\"");
        let (arena, roots) = read(&src).expect("qualified uses resolve despite collision");
        // roots: [Package, Import, Import, dsl/Unless…, web/Unless…]
        assert_eq!(
            crate::printer::print(&arena, roots[3]),
            r#"(dsl/unless-MACRO, false, "a")"#
        );
        assert_eq!(
            crate::printer::print(&arena, roots[4]),
            r#"(web/unless-MACRO, true, "b")"#
        );
    }

    /// A qualified head naming an alias that exists but does not publish the
    /// macro (or an unknown alias) errors actionably rather than silently.
    #[test]
    fn unknown_qualified_macro_errors() {
        let src = src_two_aliases("dsl/Nope 1");
        let err = read(&src).expect_err("dsl/nope is unknown");
        assert!(
            err.msg.contains("unknown qualified macro") && err.msg.contains("dsl/nope"),
            "unexpected error: {}",
            err.msg
        );
    }

    /// An aliased import (`as: dsl`) registers its macros under that alias even
    /// when the package path's last segment differs, so `dsl/Unless` resolves.
    #[test]
    fn explicit_as_alias_resolves_qualified() {
        let src = format!(
            "Package \"demo:app@0.1.0\"\n\
             Import {{pkg: \"acme:html/elements\" as: dsl macros: true from: \"{}\"}}\n\
             dsl/Unless false \"ran\"\n",
            fixture_path().to_str().unwrap()
        );
        let (arena, roots) = read(&src).expect("aliased import resolves qualified head");
        let last = *roots.last().unwrap();
        assert_eq!(
            crate::printer::print(&arena, last),
            r#"(dsl/unless-MACRO, false, "ran")"#
        );
        // The package's own last segment (`elements`) is NOT a valid alias.
        let bad = format!(
            "Package \"demo:app@0.1.0\"\n\
             Import {{pkg: \"acme:html/elements\" as: dsl macros: true from: \"{}\"}}\n\
             elements/Unless false \"ran\"\n",
            fixture_path().to_str().unwrap()
        );
        let err = read(&bad).expect_err("elements is not the alias");
        assert!(err.msg.contains("unknown qualified macro"), "unexpected: {}", err.msg);
    }

    /// Qualified expansion routes to the specific aliased component: a qualified
    /// call expands through the import bound to that alias. (Uses `identity`,
    /// which the fixture expands to its single argument.)
    #[test]
    fn qualified_call_expands_through_aliased_component() {
        let src = src_two_aliases("web/Identity 99");
        let (arena, roots) = read(&src).expect("reads qualified call");
        let mut fx = FileExpander::for_file(env!("CARGO_MANIFEST_DIR"), &arena, &roots)
            .expect("file imports macros");
        let (out, new_roots) =
            crate::expand::expand_file(arena, &roots, Some(&mut fx)).expect("expands");
        // The qualified `web/Identity 99` expands to `99` via the web-aliased
        // component (the fixture's identity returns arg 1 verbatim).
        let last = *new_roots.last().unwrap();
        assert_eq!(crate::printer::print(&out, last), "99");
    }

    /// Regression: a single unambiguous foreign macro still works by bare name
    /// (Steps 6–7 behaviour preserved) — no collision, no ambiguity.
    #[test]
    fn single_import_bare_use_unchanged() {
        let src = src_importing_fixture("Unless false \"ran\"");
        let (arena, roots) = read(&src).expect("single-import bare use still reads");
        let last = *roots.last().unwrap();
        assert_eq!(
            crate::printer::print(&arena, last),
            r#"(unless-MACRO, false, "ran")"#
        );
    }
}
