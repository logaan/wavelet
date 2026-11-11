use wavelet::host::{HostComponent, Val};
use wavelet::staging::Project;

const TEMPLATE: &str = r#"
DefFunctor "demo:box" {elem} [
  Export box
  DefResource box {
    New: Fn {value: elem} cell-new(value)
    get: Fn {self: box} The elem cell-get(self)
    echo: Static Fn {value: elem} The elem value
  }
  Def unwrap Fn {value: elem} box/get(box(value))
  Export {name: unwrap params: {value: elem} result: elem}
]
Instantiate {pkg: "demo:box" with: {elem: s64} as: numbers}
Instantiate {pkg: "demo:box" with: {elem: string} as: words}
"#;

#[test]
fn user_resource_template_instantiates_distinct_concrete_types() {
    let program = format!("{TEMPLATE}\n[numbers-unwrap(42) len(words-unwrap(\"hello\"))]");
    let outcome = wavelet::eval_snippet(&program.replace("  Export box\n", "").replace(
        "  Export {name: unwrap params: {value: elem} result: elem}\n",
        "",
    ));
    assert!(outcome.ok, "{}", outcome.error);
    assert_eq!(outcome.value, "[42, 5]");

    let bytes = Project::compile(&format!("Package \"demo:boxes@0.1.0\"\n{TEMPLATE}")).unwrap();
    let mut component = HostComponent::from_bytes(&bytes).unwrap();
    let numbers = component
        .call_instance("demo:boxes/api@0.1.0", "numbers-unwrap", &[Val::S64(42)])
        .unwrap();
    assert!(matches!(numbers.as_slice(), [Val::S64(42)]));
    let words = component
        .call_instance(
            "demo:boxes/api@0.1.0",
            "words-unwrap",
            &[Val::String("hello".into())],
        )
        .unwrap();
    assert!(matches!(words.as_slice(), [Val::String(text)] if text == "hello"));
    let handle = component
        .call_instance(
            "demo:boxes/api@0.1.0",
            "[constructor]words-box",
            &[Val::String("stored".into())],
        )
        .unwrap()
        .remove(0);
    for _ in 0..3 {
        let value = component
            .call_instance(
                "demo:boxes/api@0.1.0",
                "[method]words-box.get",
                std::slice::from_ref(&handle),
            )
            .unwrap();
        assert!(matches!(value.as_slice(), [Val::String(text)] if text == "stored"));
    }
    let value = component
        .call_instance(
            "demo:boxes/api@0.1.0",
            "[static]words-box.echo",
            &[Val::String("static".into())],
        )
        .unwrap();
    assert!(matches!(value.as_slice(), [Val::String(text)] if text == "static"));
    component.drop_resource(handle).unwrap();
}

#[test]
fn generic_functions_support_multiple_type_parameters_and_local_shadowing() {
    let source = r#"
DefFunctor "demo:pair" {left right} [
  DefType pair tuple(left right)
  Def make Fn {x: left y: right} The pair tuple2(x y)
  Def shadow Fn {make: left} make
]
Instantiate {pkg: "demo:pair" with: {right: string left: s64} as: mixed}
mixed-make(mixed-shadow(7) "seven")
"#;
    let outcome = wavelet::eval_snippet(source);
    assert!(outcome.ok, "{}", outcome.error);
    assert_eq!(outcome.value, "(7, \"seven\")");
}

#[test]
fn template_arguments_and_namespace_collisions_are_checked() {
    for declaration in [
        "Instantiate {pkg: \"demo:id\" with: {wrong: s64} as: a}",
        "Instantiate {pkg: \"demo:id\" with: {elem: s64 extra: string} as: a}",
        "Instantiate {pkg: \"demo:id\" with: {elem: 42} as: a}",
        "Def a-id 1\nInstantiate {pkg: \"demo:id\" with: {elem: s64} as: a}",
    ] {
        let source =
            format!("DefFunctor \"demo:id\" {{elem}} [Def id Fn {{x: elem}} x]\n{declaration}");
        assert!(wavelet::read_file(&source).is_err(), "{source}");
    }
}
