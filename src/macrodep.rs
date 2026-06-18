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
//! `wit/deps` (`wkg wit fetch`; see `dev-notes/decouple-wasi.md`). `wkg` does
//! **not** fetch components today, so registry-fetch of a macro component is
//! deferred (see the handoff in
//! `dev-notes/macro-components/step-05-resolve-macro-component.md`). For now a
//! project points an import at a locally built macro component, resolved in this
//! order:
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

use crate::macros::MacroComponent;
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
}
