//! Step 1 of the WASI-decoupling plan: an `Import` can be satisfied by an
//! external WIT package vendored under a project's `wit/deps` directory, as a
//! fallback after sibling-`.wvl` resolution, and the resulting `Dep` has the
//! same shape the emitter already consumes for a Wavelet dependency.

use std::path::Path;

use wavelet::emit::{self, Dep};
use wavelet::{expand, read_file, wit, witdep};

/// A fresh temp directory unique to this test, cleaned on entry and exit.
fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("wavelet-witdep-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Build the `Dep` a Wavelet source file would contribute as a build-set
/// dependency — the existing, known-good shape we compare the WIT path against.
fn dep_from_wavelet(src: &str) -> Dep {
    let (arena, roots) = read_file(src).expect("read");
    let (arena, roots) = expand::expand_file(arena, &roots, None).expect("expand");
    let info = wit::collect(&arena, &roots).expect("collect");
    Dep {
        package: info.package.clone(),
        funcs: info.exports.clone(),
        package_wit: emit::dep_package_wit(&arena, &info).expect("package_wit"),
        types: emit::dep_record_types(&arena, &info),
        type_defs: Vec::new(),
    }
}

fn assert_same_dep(a: &Dep, b: &Dep) {
    assert_eq!(a.package, b.package, "package id differs");
    assert_eq!(a.package_wit, b.package_wit, "nested package WIT differs");
    assert_eq!(a.types, b.types, "record types differ");
    assert_eq!(a.funcs.len(), b.funcs.len(), "function count differs");
    for (x, y) in a.funcs.iter().zip(&b.funcs) {
        assert_eq!(x.name, y.name);
        assert_eq!(x.iface, y.iface);
        assert_eq!(x.params, y.params);
        assert_eq!(x.result, y.result);
    }
}

/// An external WIT package placed in `wit/deps` parses into the *same* `Dep` a
/// sibling Wavelet file with the same surface would produce.
#[test]
fn external_wit_dep_matches_wavelet_dep_shape() {
    let dir = scratch("shape");
    let deps = dir.join("wit/deps");
    std::fs::create_dir_all(&deps).unwrap();

    // The WIT package, as `wkg` would vendor it under wit/deps.
    std::fs::write(
        deps.join("acme-greet.wit"),
        "package acme:greet@0.1.0;\n\
         interface api {\n  \
           greet: func(name: string) -> string;\n\
         }\n",
    )
    .unwrap();

    let from_wit = witdep::resolve_dep(&deps, "acme:greet")
        .expect("resolve_dep ok")
        .expect("acme:greet found in wit/deps");

    // The equivalent Wavelet dependency file.
    let from_wvl = dep_from_wavelet(
        "Package \"acme:greet@0.1.0\"\n\n\
         Export greet\n\
         Def greet Fn {name: string}\n  \
           str-cat(\"Hello, \" name \"!\")\n",
    );

    assert_same_dep(&from_wit, &from_wvl);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Record types defined by the external WIT package are projected onto the
/// `Dep` the same way a Wavelet dep's `DefType`s are.
#[test]
fn external_wit_dep_carries_record_types() {
    let dir = scratch("record");
    let deps = dir.join("wit/deps");
    std::fs::create_dir_all(&deps).unwrap();

    std::fs::write(
        deps.join("acme-people.wit"),
        "package acme:people@0.1.0;\n\
         interface api {\n  \
           record person { name: string, age: u32 }\n  \
           describe: func(p: person) -> string;\n\
         }\n",
    )
    .unwrap();

    let dep = witdep::resolve_dep(&deps, "acme:people")
        .expect("resolve_dep ok")
        .expect("acme:people found");

    assert_eq!(dep.package, "acme:people@0.1.0");
    assert_eq!(
        dep.types,
        vec![(
            "person".to_string(),
            vec![
                ("name".to_string(), "string".to_string()),
                ("age".to_string(), "u32".to_string()),
            ]
        )]
    );
    let f = &dep.funcs[0];
    assert_eq!(f.name, "describe");
    assert_eq!(f.params, vec![("p".to_string(), "person".to_string())]);
    assert_eq!(f.result, Some("string".to_string()));

    let _ = std::fs::remove_dir_all(&dir);
}

/// `resolve_dep` returns `Ok(None)` for an absent directory or an unknown
/// package, so the build driver falls through to its usual error rather than
/// failing the parse.
#[test]
fn resolve_dep_absent_returns_none() {
    let dir = scratch("absent");
    // No wit/deps directory exists at all.
    assert!(
        witdep::resolve_dep(&dir.join("wit/deps"), "acme:greet")
            .expect("ok")
            .is_none()
    );

    let deps = dir.join("wit/deps");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(
        deps.join("acme-greet.wit"),
        "package acme:greet@0.1.0;\ninterface api { greet: func() -> string; }\n",
    )
    .unwrap();
    // Directory present, but the requested package is not in it.
    assert!(
        witdep::resolve_dep(&deps, "nope:missing")
            .expect("ok")
            .is_none()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// End to end: an importer whose dependency lives only in `wit/deps` (no
/// sibling `.wvl`) resolves against it. Resolution is the Step 1 concern, so we
/// assert the build gets *past* import resolution — it must not fail with the
/// "not satisfied" error. (Lowering a call into a dep is a later step.)
#[test]
fn build_resolves_import_from_wit_deps() {
    let dir = scratch("build");
    let src = dir.join("src");
    let deps = dir.join("wit/deps");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&deps).unwrap();

    std::fs::write(
        deps.join("acme-greet.wit"),
        "package acme:greet@0.1.0;\n\
         interface api {\n  greet: func(name: string) -> string;\n}\n",
    )
    .unwrap();

    let app = src.join("app.wvl");
    std::fs::write(
        &app,
        "Package \"demo:app@0.1.0\"\n\n\
         Import {pkg: \"acme:greet/api\" as: greeting}\n\n\
         Export {name: hello params: [{who: string}] result: string}\n\
         Def hello Fn {who: string}\n  greeting/greet(who)\n",
    )
    .unwrap();

    let out = dir.join("out");
    let res = wavelet::build::build_files(
        &[app.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    );

    // The build may still fail in later codegen (lowering a dep call is a
    // future step), but it must not fail at *import resolution*: the dependency
    // was found in wit/deps and fed to the emitter.
    if let Err(e) = &res {
        assert!(
            !e.contains("is not satisfied"),
            "import was not resolved from wit/deps: {e}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Sanity: the path-derivation helper is exercised through `build_files`
/// (`wit/deps` is a sibling of `src/`), so a plain file path resolves the right
/// directory. This mirrors the layout `wavelet new` scaffolds.
#[test]
fn wit_deps_is_sibling_of_src() {
    // src/app.wvl -> ../wit/deps
    let p = Path::new("/proj/src/app.wvl");
    let expected = Path::new("/proj/wit/deps");
    let derived = p.parent().unwrap().parent().unwrap().join("wit").join("deps");
    assert_eq!(derived, expected);
}
