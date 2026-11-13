use std::collections::HashMap;

use wasmtime::component::{Component, Linker, Val};
use wasmtime::{Engine, Store};
use wavelet::{emit, expand, read_file, wit, witdep};

fn compile(source: &str, dependency: Option<&str>) -> Result<Vec<u8>, String> {
    let dir = tempfile::tempdir().unwrap();
    let mut deps = HashMap::new();
    if let Some(body) = dependency {
        std::fs::write(
            dir.path().join("audit.wit"),
            format!("package audit:dep; interface api {{ {body} }}"),
        )
        .unwrap();
        deps.insert(
            "audit:dep".into(),
            witdep::resolve_dep(dir.path(), "audit:dep")?.unwrap(),
        );
    }
    let (arena, roots) = read_file(source).map_err(|e| e.to_string())?;
    let (arena, roots) = expand::expand_file(arena, &roots, None)?;
    let info = wit::collect(&arena, &roots)?;
    emit::emit_component(&arena, &roots, &info, &deps)
}

fn roundtrip(
    declarations: &str,
    ty: &str,
    dependency: Option<&str>,
    values: &[Val],
) -> Result<(), String> {
    let imported = dependency.is_some();
    let import = if imported {
        "Import {pkg: \"audit:dep/api\" as: dep}"
    } else {
        ""
    };
    let body = if imported { "dep/echo(value)" } else { "value" };
    let bytes = compile(
        &format!(
            "Package \"audit:app\"\n{import}\n{declarations}\nExport echo\nDef echo Fn {{value: {ty}}} The {ty} {body}"
        ),
        dependency,
    )?;
    let engine = Engine::default();
    let component = Component::from_binary(&engine, &bytes).map_err(|e| format!("{e:#}"))?;
    let mut linker = Linker::<()>::new(&engine);
    if imported {
        linker
            .instance("audit:dep/api")
            .unwrap()
            .func_new("echo", |_, _, params, results| {
                results[0] = params[0].clone();
                Ok(())
            })
            .unwrap();
    }
    let mut store = Store::new(&engine, ());
    let instance = linker
        .instantiate(&mut store, &component)
        .map_err(|e| format!("{e:#}"))?;
    let api = instance
        .get_export_index(&mut store, None, "audit:app/api")
        .unwrap();
    let index = instance
        .get_export_index(&mut store, Some(&api), "echo")
        .unwrap();
    let func = instance.get_func(&mut store, index).unwrap();
    for value in values {
        let mut results = vec![Val::Bool(false)];
        func.call(&mut store, std::slice::from_ref(value), &mut results)
            .map_err(|e| format!("{e:#}"))?;
        assert_eq!(results, vec![value.clone()]);
    }
    Ok(())
}

#[test]
fn nested_options_cross_export_and_import_boundaries() {
    let values = [
        Val::Option(None),
        Val::Option(Some(Box::new(Val::Option(None)))),
        Val::Option(Some(Box::new(Val::Option(Some(Box::new(Val::U32(42))))))),
    ];
    for dep in [
        None,
        Some("echo: func(value: option<option<u32>>) -> option<option<u32>>;"),
    ] {
        roundtrip("", "option(option(u32))", dep, &values).unwrap();
    }
}

#[test]
fn nested_records_lists_variants_cross_both_boundaries() {
    let values = [
        Val::List(vec![
            Val::Record(vec![(
                "value".into(),
                Val::Variant("item".into(), Some(Box::new(Val::String("hé🌊".into())))),
            )]),
            Val::Record(vec![("value".into(), Val::Variant("empty".into(), None))]),
        ]),
        Val::List(vec![]),
    ];
    let declarations = "DefType choice [item(string) empty]\nDefType entry {value: choice}";
    let dep = "variant choice { item(string), empty } record entry { value: choice } echo: func(value: list<entry>) -> list<entry>;";
    for dependency in [None, Some(dep)] {
        roundtrip(declarations, "list(entry)", dependency, &values).unwrap();
    }
}

#[test]
fn primitive_alias_chains_cross_import_boundary() {
    roundtrip(
        "",
        "u32",
        Some("type first = u32; type second = first; echo: func(value: second) -> second;"),
        &[Val::U32(0), Val::U32(u32::MAX)],
    )
    .unwrap();
}

#[test]
fn empty_tuple_export_signature_is_rejected() {
    let error = roundtrip("", "tuple()", None, &[Val::Tuple(vec![])]).unwrap_err();
    assert!(
        error.contains("tuple type must have at least one type"),
        "{error}"
    );
}

#[test]
fn flags_storage_boundaries() {
    for count in [1, 8, 9, 16, 17, 32, 33] {
        let names: Vec<_> = (0..count).map(|i| format!("flag{i}")).collect();
        let declarations = format!("DefType bits {{{}}}", names.join(" "));
        let dep = format!(
            "flags bits {{ {} }} echo: func(value: list<bits>) -> list<bits>;",
            names.join(", ")
        );
        for dependency in [None, Some(dep.as_str())] {
            let result = roundtrip(
                &declarations,
                "list(bits)",
                dependency,
                &[Val::List(vec![Val::Flags(vec![names[count - 1].clone()])])],
            );
            if count <= 32 {
                result.unwrap();
            } else {
                assert!(result.unwrap_err().contains("32"));
            }
        }
    }
}

#[test]
fn enum_storage_boundaries() {
    for count in [256, 257] {
        let names: Vec<_> = (0..count).map(|i| format!("case{i}")).collect();
        let declarations = format!("DefType choice [{}]", names.join(" "));
        let dep = format!(
            "enum choice {{ {} }} echo: func(value: list<choice>) -> list<choice>;",
            names.join(", ")
        );
        for dependency in [None, Some(dep.as_str())] {
            let result = roundtrip(
                &declarations,
                "list(choice)",
                dependency,
                &[Val::List(vec![Val::Enum(names[count - 1].clone())])],
            );
            if count == 256 {
                result.unwrap();
            } else {
                assert!(result.unwrap_err().contains("256"));
            }
        }
    }
}

#[test]
fn authored_aliases() {
    roundtrip(
        "DefType first u32\nDefType second first",
        "second",
        None,
        &[Val::U32(42)],
    )
    .unwrap();
}

#[test]
fn owned_resources_created_inside_exported_compounds_currently_trap() {
    for (declarations, ty, body) in [
        ("", "list(counter)", "[counter(42)]"),
        (
            "DefType entry {value: counter}",
            "entry",
            "{value: counter(42)}",
        ),
        (
            "DefType choice [item(counter) empty]",
            "choice",
            "item(counter(42))",
        ),
        ("", "option(counter)", "some(counter(42))"),
    ] {
        let bytes = compile(&format!("Package \"audit:app\"\nDefResource counter {{New: Fn {{start: u32}} cell-new(start) value: Fn {{self: counter}} The u32 cell-get(self)}}\nExport counter\n{declarations}\nExport make\nDef make Fn {{}} The {ty} {body}"), None).unwrap();
        let mut component = wavelet::host::HostComponent::from_bytes(&bytes).unwrap();
        let error = component
            .call_instance("audit:app/api", "make", &[])
            .unwrap_err();
        assert!(error.contains("unreachable"), "{ty}: {error}");
    }
}

#[test]
fn borrows_cannot_escape_in_results_or_stored_types() {
    for declaration in [
        "Export {name: make result: borrow(counter)}\nDef make Fn {} counter(0)",
        "Export {name: make result: list(borrow(counter))}\nDef make Fn {} [counter(0)]",
        "DefType entry {value: borrow(counter)}",
        "DefType choice [item(borrow(counter)) empty]",
    ] {
        let error = compile(&format!("Package \"audit:app\"\nDefResource counter {{New: Fn {{value: u32}} cell-new(value)}}\n{declaration}"), None).unwrap_err();
        assert!(error.contains("borrow"), "{error}");
    }
}

#[test]
fn payload_variant_storage_boundaries() {
    for count in [256, 257] {
        let names: Vec<_> = (0..count).map(|i| format!("case{i}")).collect();
        let declarations = format!(
            "DefType choice [{}]",
            names
                .iter()
                .map(|name| format!("{name}(u32)"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let dep = format!(
            "variant choice {{ {} }} echo: func(value: list<choice>) -> list<choice>;",
            names
                .iter()
                .map(|name| format!("{name}(u32)"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        for dependency in [None, Some(dep.as_str())] {
            let result = roundtrip(
                &declarations,
                "list(choice)",
                dependency,
                &[Val::List(vec![Val::Variant(
                    names[count - 1].clone(),
                    Some(Box::new(Val::U32(42))),
                )])],
            );
            if count == 256 {
                result.unwrap();
            } else {
                assert!(result.unwrap_err().contains("256"));
            }
        }
    }
}

#[test]
fn independently_authored_component_calls_wavelet_and_is_called_by_wavelet() {
    let external = wat::parse_str(
        r#"(component
      (core module $m (func (export "echo") (param i32) (result i32) local.get 0))
      (core instance $i (instantiate $m))
      (func $echo (param "value" u32) (result u32) (canon lift (core func $i "echo")))
      (instance $api (export "echo" (func $echo)))
      (export "audit:dep/api" (instance $api))
    )"#,
    )
    .unwrap();
    let caller = wat::parse_str(r#"(component
      (import "audit:app/api" (instance $api (export "echo" (func (param "value" u32) (result u32)))))
      (alias export $api "echo" (func $echo))
      (core func $lower (canon lower (func $echo)))
      (core module $m
        (import "host" "echo" (func $echo (param i32) (result i32)))
        (func (export "run") (param i32) (result i32) local.get 0 call $echo))
      (core instance $i (instantiate $m (with "host" (instance (export "echo" (func $lower))))))
      (func (export "run") (param "value" u32) (result u32) (canon lift (core func $i "run")))
    )"#).unwrap();
    let wavelet = compile("Package \"audit:app\"\nImport {pkg: \"audit:dep/api\" as: dep}\nExport echo\nDef echo Fn {value: u32} The u32 dep/echo(value)", Some("echo: func(value: u32) -> u32;")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for (name, bytes) in [
        ("caller", caller),
        ("wavelet", wavelet),
        ("external", external),
    ] {
        let path = dir.path().join(format!("{name}.wasm"));
        std::fs::write(&path, bytes).unwrap();
        paths.push(path.to_str().unwrap().to_owned());
    }
    let inner = dir.path().join("inner.wasm");
    wavelet::build::compose_files(&paths[1..], inner.to_str().unwrap()).unwrap();
    let output = dir.path().join("app.wasm");
    wavelet::build::compose_files(
        &[paths[0].clone(), inner.to_str().unwrap().into()],
        output.to_str().unwrap(),
    )
    .unwrap();
    let mut component = wavelet::host::HostComponent::from_file(&output).unwrap();
    for value in [0, 42, u32::MAX] {
        assert_eq!(
            component.call("run", &[Val::U32(value)]).unwrap(),
            vec![Val::U32(value)]
        );
    }
}

#[test]
fn imported_owned_resources_roundtrip_inside_compounds() {
    use wasmtime::component::{Resource, ResourceType};
    for (declaration, ty, wrap) in [
        ("", "list<counter>", 0),
        ("record entry { value: counter }", "entry", 1),
        ("variant choice { item(counter), empty }", "choice", 2),
        ("", "option<counter>", 3),
    ] {
        let dep = format!(
            "resource counter; {declaration} make: func() -> {ty}; take: func(value: {ty}) -> u32;"
        );
        let bytes = compile("Package \"audit:app\"\nImport {pkg: \"audit:dep/api\" as: dep}\nExport run\nDef run Fn {} The u32 dep/take(dep/make())", Some(&dep)).unwrap();
        let engine = Engine::default();
        let component = Component::from_binary(&engine, &bytes).unwrap();
        let mut linker = Linker::<()>::new(&engine);
        let mut api = linker.instance("audit:dep/api").unwrap();
        api.resource("counter", ResourceType::host::<u32>(), |_, _| Ok(()))
            .unwrap();
        api.func_new("make", move |store, _, _, results| {
            let value = Val::Resource(Resource::<u32>::new_own(42).try_into_resource_any(store)?);
            results[0] = match wrap {
                0 => Val::List(vec![value]),
                1 => Val::Record(vec![("value".into(), value)]),
                2 => Val::Variant("item".into(), Some(Box::new(value))),
                _ => Val::Option(Some(Box::new(value))),
            };
            Ok(())
        })
        .unwrap();
        api.func_new("take", |store, _, params, results| {
            let value = match &params[0] {
                Val::List(values) => &values[0],
                Val::Record(fields) => &fields[0].1,
                Val::Variant(_, Some(value)) | Val::Option(Some(value)) => value,
                other => panic!("unexpected compound: {other:?}"),
            };
            let Val::Resource(handle) = value else {
                panic!("expected resource")
            };
            results[0] = Val::U32(Resource::<u32>::try_from_resource_any(*handle, store)?.rep());
            Ok(())
        })
        .unwrap();
        let mut store = Store::new(&engine, ());
        let instance = linker.instantiate(&mut store, &component).unwrap();
        let api = instance
            .get_export_index(&mut store, None, "audit:app/api")
            .unwrap();
        let index = instance
            .get_export_index(&mut store, Some(&api), "run")
            .unwrap();
        let func = instance.get_func(&mut store, index).unwrap();
        let mut result = [Val::Bool(false)];
        func.call(&mut store, &[], &mut result).unwrap();
        assert_eq!(result, [Val::U32(42)]);
    }
}
