use std::collections::HashMap;

use wavelet::{emit, expand, read_file, wit, witdep};

fn expanded(source: &str) -> (wavelet::form::Arena, Vec<wavelet::form::NodeId>) {
    let (arena, roots) = read_file(source).unwrap();
    expand::expand_file(arena, &roots, None).unwrap()
}

#[test]
fn fetch_planning_preserves_arbitrary_namespaces_and_versions() {
    let source = r#"
Package "app:frontend@1.0.0"
Import {pkg: "acme:devices/api@3.4.5" as: devices}
Import {pkg: "company:domain/api" as: domain}
Export {iface: "custom:events/handler@7.8.9" name: handle result: string}
Def handle Fn {} "ok"
"#;
    let (arena, roots) = expanded(source);
    let info = wit::collect(&arena, &roots).unwrap();
    assert_eq!(info.imports[0].version.as_deref(), Some("3.4.5"));
    let local = vec!["company:domain".to_owned()];
    assert!(wit::has_external_deps(&info, &local));
    let fetch = wit::synthesize_fetch_world_excluding(&arena, &roots, &local).unwrap();
    assert!(fetch.contains("import acme:devices/api@3.4.5;"), "{fetch}");
    assert!(
        fetch.contains("export custom:events/handler@7.8.9;"),
        "{fetch}"
    );
    assert!(!fetch.contains("company:domain"), "{fetch}");
    let full = wit::synthesize(&arena, &roots).unwrap();
    assert!(full.contains("import company:domain/api;"), "{full}");
    assert!(!full.contains("0.2.0"), "{full}");
}

#[test]
fn arbitrary_wit_versions_are_selected_explicitly() {
    let dir = tempfile::tempdir().unwrap();
    for version in ["1.0.0", "2.0.0"] {
        std::fs::write(
            dir.path().join(format!("devices-{version}.wit")),
            format!("package acme:devices@{version}; interface api {{ ping: func() -> u32; }}"),
        )
        .unwrap();
    }
    let dep = witdep::resolve_dep(dir.path(), "acme:devices@2.0.0")
        .unwrap()
        .unwrap();
    assert_eq!(dep.package, "acme:devices@2.0.0");
    assert!(
        witdep::resolve_dep(dir.path(), "acme:devices@3.0.0")
            .unwrap()
            .is_none()
    );
    let error = witdep::resolve_dep(dir.path(), "acme:devices")
        .err()
        .unwrap();
    assert!(error.contains("multiple versions"), "{error}");
}

#[test]
fn generic_resources_and_external_exports_compile_with_or_without_versions() {
    for version in ["", "@3.4.5"] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("devices.wit"), format!(
            "package acme:devices{version}; interface api {{ resource device {{ constructor(); label: func() -> string; }} inspect: func(item: borrow<device>) -> string; }}"
        )).unwrap();
        std::fs::write(
            dir.path().join("events.wit"),
            format!(
                "package custom:events{version}; interface handler {{ handle: func() -> string; }}"
            ),
        )
        .unwrap();
        let source = format!(
            r#"
Package "app:frontend@1.0.0"
Import {{pkg: "acme:devices/api{version}" as: devices}}
Export {{iface: "custom:events/handler{version}" name: handle result: string}}
Def inspect Fn {{item: borrow(device)}} devices/inspect(item)
Def handle Fn {{}}
  Let {{item: devices/device()}}
    Let {{label: inspect(item)}}
      Do [devices/drop-device(item) label]
"#
        );
        let (arena, roots) = expanded(&source);
        let info = wit::collect(&arena, &roots).unwrap();
        let mut deps = HashMap::new();
        for name in ["acme:devices", "custom:events"] {
            deps.insert(
                name.to_owned(),
                witdep::resolve_dep(dir.path(), &format!("{name}{version}"))
                    .unwrap()
                    .unwrap(),
            );
        }
        let bytes = emit::emit_component(&arena, &roots, &info, &deps).unwrap();
        assert!(bytes.starts_with(b"\0asm"));
        let wit_component::DecodedWasm::Component(resolve, world) =
            wit_component::decode(&bytes).unwrap()
        else {
            panic!("expected a component");
        };
        let world = &resolve.worlds[world];
        assert!(world.imports.values().any(|item| {
            match item {
                wit_parser::WorldItem::Interface { id, .. } => {
                    resolve.interfaces[*id].package.is_some_and(|pkg| {
                        resolve.packages[pkg].name.to_string() == format!("acme:devices{version}")
                    })
                }
                _ => false,
            }
        }));
        assert!(world.exports.values().any(|item| {
            match item {
                wit_parser::WorldItem::Interface { id, .. } => {
                    resolve.interfaces[*id].package.is_some_and(|pkg| {
                        resolve.packages[pkg].name.to_string() == format!("custom:events{version}")
                    })
                }
                _ => false,
            }
        }));
    }
}

#[test]
fn requested_versions_cannot_silently_bind_to_another_version() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("devices.wit"),
        "package acme:devices@1.0.0; interface api { ping: func() -> u32; }",
    )
    .unwrap();
    let dep = witdep::resolve_dep(dir.path(), "acme:devices")
        .unwrap()
        .unwrap();
    let deps = HashMap::from([("acme:devices".to_owned(), dep)]);
    for source in [
        "Package \"app:test\"\nImport {pkg: \"acme:devices/api@2.0.0\" as: devices}\nExport {name: run result: u32}\nDef run Fn {} devices/ping()",
        "Package \"app:test\"\nExport {iface: \"acme:devices/api@2.0.0\" name: ping result: u32}\nDef ping Fn {} 1",
    ] {
        let (arena, roots) = expanded(source);
        let info = wit::collect(&arena, &roots).unwrap();
        let error = emit::emit_component(&arena, &roots, &info, &deps).unwrap_err();
        assert!(error.contains("resolved"), "{error}");
    }
}

#[test]
fn bundled_wasi_packages_use_supported_generic_wit_types() {
    let deps = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/http/wit/deps");
    for package in [
        "wasi:cli",
        "wasi:clocks",
        "wasi:http",
        "wasi:io",
        "wasi:random",
    ] {
        let dep = witdep::resolve_dep(&deps, &format!("{package}@0.2.0"))
            .unwrap()
            .unwrap();
        assert!(!dep.funcs.is_empty(), "{package}");
        for (name, ty) in dep.type_defs {
            let valid = match ty {
                emit::TypeDef::Enum(cases) => cases.len() <= 256,
                emit::TypeDef::Variant(cases) => cases.len() <= 256,
                emit::TypeDef::Flags(cases) => cases.len() <= 32,
                _ => true,
            };
            assert!(valid, "{package}/{name} exceeds backend layout support");
        }
    }
}

#[test]
fn unsupported_wit_features_report_the_actual_feature() {
    for (body, expected) in [
        ("type pending = future<string>;", "future"),
        ("type events = stream<u8>;", "stream"),
        ("type entries = map<string, u32>;", "map"),
        ("type fixed = list<u8, 4>;", "fixed-length list"),
        ("run: async func();", "async WIT function"),
        ("run: func(context: error-context);", "error-context"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("async.wit"),
            format!("package acme:events; interface api {{ {body} }}"),
        )
        .unwrap();
        let error = witdep::resolve_dep(dir.path(), "acme:events")
            .err()
            .unwrap();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn transitive_wit_record_types_do_not_require_extra_source_imports() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("data.wit"),
        "package custom:data; interface values { record payload { value: string } }",
    )
    .unwrap();
    std::fs::write(dir.path().join("service.wit"), "package custom:service; interface api { use custom:data/values.{payload}; make: func() -> payload; inspect: func(value: payload) -> bool; }").unwrap();
    let dep = witdep::resolve_dep(dir.path(), "custom:service")
        .unwrap()
        .unwrap();
    assert_eq!(dep.types.len(), 1);
    assert_eq!(dep.aliases.len(), 1);
    let source = r#"
Package "app:test"
Import {pkg: "custom:service/api" as: service}
Export {name: run result: bool}
Def run Fn {} service/inspect(service/make())
"#;
    let (arena, roots) = expanded(source);
    let info = wit::collect(&arena, &roots).unwrap();
    let bytes = emit::emit_component(
        &arena,
        &roots,
        &info,
        &HashMap::from([("custom:service".to_owned(), dep)]),
    )
    .unwrap();
    assert!(bytes.starts_with(b"\0asm"));
}

#[test]
fn resource_aliases_across_interfaces_preserve_one_nominal_identity() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("devices.wit"),
        r#"
package acme:devices;
interface types { resource device { label: func() -> string; } }
interface handler { use types.{device}; handle: func(item: device) -> string; }
interface reader { use types.{device}; inspect: func(item: borrow<device>) -> string; }
"#,
    )
    .unwrap();
    let dep = witdep::resolve_dep(dir.path(), "acme:devices")
        .unwrap()
        .unwrap();
    let source = r#"
Package "app:test"
Import {pkg: "acme:devices/types" as: devices}
Import {pkg: "acme:devices/reader" as: reader}
Export {iface: "acme:devices/handler" name: handle params: {item: device} result: string}
Def handle Fn {item: device}
  Let {label: reader/inspect(item)} Do [devices/drop-device(item) label]
"#;
    let (arena, roots) = expanded(source);
    let info = wit::collect(&arena, &roots).unwrap();
    emit::emit_component(
        &arena,
        &roots,
        &info,
        &HashMap::from([("acme:devices".to_owned(), dep)]),
    )
    .unwrap();
}
