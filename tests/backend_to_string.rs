use wavelet::host::{HostComponent, Val};
use wavelet::value::{Value, print_value};

fn to_string_component() -> HostComponent {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-tostr-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let app = r#"Package "demo:app@0.1.0"

Export {name: f2s params: {x: f64} result: string}
Def f2s Fn {x}
  to-string(x)

Export {name: c2s params: {c: char} result: string}
Def c2s Fn {c}
  to-string(c)

Export {name: compound params: {x: f64 c: char} result: string}
Def compound Fn {x c}
  to-string({f: x c: c l: [x]})
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();

    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the to-string component");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);

    HostComponent::from_bytes(&bytes).expect("instantiate the to-string component")
}

const IFACE: &str = "demo:app/api@0.1.0";

#[test]
fn compiled_unit_values_match_expected_values() {
    use wavelet::compiler::{NoMacros, Request, Step, compile};

    for (source, expected) in [
        ("Let {n: cell-new(0)} cell-set(n 1)", r#""{}""#),
        ("drop(42)", r#""{}""#),
        ("Do []", r#""{}""#),
        ("[drop(1) drop(2)]", r#""[{}, {}]""#),
        (
            "{unit: drop(1) boolean: false}",
            r#""{unit: {}, boolean: false}""#,
        ),
    ] {
        let source = format!("to-string({source})");
        let Step::Complete(compilation) =
            compile(&Request::snippet(&source), &mut NoMacros).unwrap()
        else {
            panic!("unexpected macro request")
        };
        let mut component =
            HostComponent::from_bytes(&compilation.entry_artifact().unwrap().bytes).unwrap();
        let invocation = compilation.invocation.unwrap();
        assert_eq!(
            component
                .call_instance(&invocation.interface_name, &invocation.function_name, &[])
                .unwrap(),
            vec![Val::String(expected.into())],
            "{source}"
        );
    }
}

fn call(c: &mut HostComponent, f: &str, args: &[Val]) -> String {
    match &c
        .call_instance(IFACE, f, args)
        .unwrap_or_else(|e| panic!("`{f}` should succeed: {e}"))[0]
    {
        Val::String(s) => {
            wasm_wave::untyped::UntypedValue::parse(s)
                .unwrap_or_else(|e| panic!("invalid WAVE {s:?}: {e}"));
            s.to_string()
        }
        other => panic!("`{f}` returned {other:?}, expected a string"),
    }
}

const FLOATS: &[f64] = &[
    0.0,
    -0.0,
    1.0,
    -1.0,
    0.1,
    2.5,
    -12.25,
    std::f64::consts::PI,
    2.0 / 3.0,
    100.0,
    999999.0,
    999999.9,
    123456.7,
    0.0001,
    0.00001,
    1000000.0,
    123456789.0,
    1e300,
    1.5e300,
    -2.5e-7,
    9.9999999,
    f64::MAX,
    f64::MIN_POSITIVE,
    5e-324,
    f64::NAN,
    f64::INFINITY,
    f64::NEG_INFINITY,
];

#[test]
fn compiled_float_to_string_matches_expected_values() {
    let mut c = to_string_component();
    for &x in FLOATS {
        let expect = print_value(&Value::Dec(x));
        let got = call(&mut c, "f2s", &[Val::Float64(x)]);
        assert_eq!(got, expect, "to-string({x:?}) diverged from print_value");
    }
}

const CHARS: &[char] = &[
    'a', 'Z', '0', ' ', '"', '\'', '\\', '\n', '\r', '\t', '\0', '\u{1}', '\u{f}', '\u{10}',
    '\u{1b}', '\u{7f}', '\u{80}', '\u{9f}', '\u{a0}', '\u{ad}', 'é', 'ß', '☃', '😀',
];

#[test]
fn compiled_char_to_string_matches_expected_values() {
    let mut c = to_string_component();
    for &ch in CHARS {
        let expect = print_value(&Value::Char(ch));
        let got = call(&mut c, "c2s", &[Val::Char(ch)]);
        assert_eq!(got, expect, "to-string({ch:?}) diverged from print_value");
    }
}

#[test]
fn floats_and_chars_print_inside_compounds() {
    let mut c = to_string_component();
    let x = std::f64::consts::PI;
    let expect = print_value(&Value::Rec(vec![
        ("f".into(), Value::Dec(x)),
        ("c".into(), Value::Char('☃')),
        ("l".into(), Value::Lst(vec![Value::Dec(x)])),
    ]));
    let got = call(&mut c, "compound", &[Val::Float64(x), Val::Char('☃')]);
    assert_eq!(got, expect);
}
