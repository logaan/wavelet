use wasm_wave::untyped::UntypedValue;
use wavelet::host::{HostComponent, Val};
use wavelet::value::{Value, print_value};

#[test]
fn documented_values_parse_as_wave() {
    let examples: serde_json::Value =
        serde_json::from_str(include_str!("../docs/examples.json")).unwrap();
    let mut failures = Vec::new();
    let mut empty_tuple_exception_seen = false;
    for (name, example) in examples.as_object().unwrap() {
        let Some(text) = example.get("value").and_then(|v| v.as_str()) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        if let Err(error) = UntypedValue::parse(text) {
            if name == "std-tuple" && error.to_string().contains("empty tuple") {
                empty_tuple_exception_seen = true;
                continue;
            }
            failures.push(format!("{name}: {text}: {error}"));
        }
    }
    assert!(
        empty_tuple_exception_seen,
        "remove the obsolete std-tuple WAVE exception"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn string_control_characters_roundtrip_through_both_printers() {
    let source = r#"Package "test:strings@0.1.0"
Export {name: render params: {s: string} result: string}
Def render Fn {s: string} to-string(s)
"#;
    let bytes = wavelet::staging::Project::compile(source).unwrap();
    let mut component = HostComponent::from_bytes(&bytes).unwrap();
    let cases = [
        (0u8..=127).map(char::from).collect::<String>(),
        "\0after a NUL\0\n\t\r\"\\'".to_string(),
        "é𐐀\u{85}\u{200b}".to_string(),
    ];
    for input in cases {
        let expected = print_value(&Value::Str(input.clone()));
        let output = component
            .call_instance(
                "test:strings/api@0.1.0",
                "render",
                &[Val::String(input.clone())],
            )
            .unwrap();
        assert_eq!(output, vec![Val::String(expected.clone())]);
        let parsed: wasm_wave::value::Value =
            wasm_wave::from_str(&wasm_wave::value::Type::STRING, &expected).unwrap();
        use wasm_wave::wasm::WasmValue;
        assert_eq!(parsed.unwrap_string(), input);
    }
}
