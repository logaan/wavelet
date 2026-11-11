use std::path::Path;

use wavelet::emit::{self, Dep};
use wavelet::{expand, read_file, wit, witdep};

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("wavelet-witdep-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

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
        aliases: Vec::new(),
        type_ifaces: Vec::new(),
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

#[test]
fn external_wit_dep_matches_wavelet_dep_shape() {
    let dir = scratch("shape");
    let deps = dir.join("wit/deps");
    std::fs::create_dir_all(&deps).unwrap();

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

    let from_wlt = dep_from_wavelet(
        "Package \"acme:greet@0.1.0\"\n\n\
         Export greet\n\
         Def greet Fn {name: string}\n  \
           str-cat(\"Hello, \" name \"!\")\n",
    );

    assert_same_dep(&from_wit, &from_wlt);

    let _ = std::fs::remove_dir_all(&dir);
}

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
    let (identity, interface, original) = &dep.type_ifaces[0];
    assert_eq!(interface, "api");
    assert_eq!(original, "person");
    assert_eq!(
        dep.types,
        vec![(
            identity.clone(),
            vec![
                ("name".to_owned(), "string".to_owned()),
                ("age".to_owned(), "u32".to_owned())
            ]
        )]
    );
    let f = &dep.funcs[0];
    assert_eq!(f.name, "describe");
    assert_eq!(f.params, vec![("p".to_string(), identity.clone())]);
    assert_eq!(f.result, Some("string".to_string()));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn resolve_dep_absent_returns_none() {
    let dir = scratch("absent");

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

    assert!(
        witdep::resolve_dep(&deps, "nope:missing")
            .expect("ok")
            .is_none()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

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

    let app = src.join("app.wlt");
    std::fs::write(
        &app,
        "Package \"demo:app@0.1.0\"\n\n\
         Import {pkg: \"acme:greet/api\" as: greeting}\n\n\
         Export {name: hello params: [{who: string}] result: string}\n\
         Def hello Fn {who: string}\n  greeting/greet(who)\n",
    )
    .unwrap();

    let out = dir.join("out");
    let res =
        wavelet::build::build_files(&[app.to_str().unwrap().to_string()], out.to_str().unwrap());

    if let Err(e) = &res {
        assert!(
            !e.contains("is not satisfied"),
            "import was not resolved from wit/deps: {e}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wit_deps_is_sibling_of_src() {
    let p = Path::new("/proj/src/app.wlt");
    let expected = Path::new("/proj/wit/deps");
    let derived = p
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("wit")
        .join("deps");
    assert_eq!(derived, expected);
}

#[test]
fn encoded_wit_dependency_resolves_alone_and_from_text_dependents() {
    let directory = tempfile::tempdir().unwrap();
    let mut resolve = wit_parser::Resolve::default();
    let package = resolve.push_str("encoded.wit", "package test:encoded@1.0.0; interface api { record item { value: u32 } inspect: func(value: item) -> u32; }").unwrap();
    let bytes = wit_component::encode(&resolve, package).unwrap();
    std::fs::write(directory.path().join("encoded.wasm"), bytes).unwrap();
    let encoded = witdep::resolve_dep(directory.path(), "test:encoded@1.0.0")
        .unwrap()
        .unwrap();
    assert_eq!(encoded.funcs[0].name, "inspect");
    assert_eq!(encoded.types.len(), 1);
    std::fs::write(directory.path().join("consumer.wit"), "package test:consumer; interface api { use test:encoded/api@1.0.0.{item}; inspect: func(value: item) -> u32; }").unwrap();
    let consumer = witdep::resolve_dep(directory.path(), "test:consumer")
        .unwrap()
        .unwrap();
    assert_eq!(consumer.funcs[0].name, "inspect");
    assert_eq!(consumer.types.len(), 1);
}

#[test]
fn text_encoded_wit_dependency_resolves() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("encoded.wat"),
        r#"(component
        (type (component
            (type (instance
                (type (func (result u32)))
                (export "inspect" (func (type 0)))
            ))
            (export "test:encoded/api@1.0.0" (instance (type 0)))
        ))
        (export "api" (type 0))
    )"#,
    )
    .unwrap();
    let dependency = witdep::resolve_dep(directory.path(), "test:encoded@1.0.0")
        .unwrap()
        .unwrap();
    assert_eq!(dependency.funcs[0].name, "inspect");
    assert_eq!(dependency.funcs[0].result.as_deref(), Some("u32"));
}
