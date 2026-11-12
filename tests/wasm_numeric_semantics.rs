use wavelet::host::{HostComponent, Val};

fn component() -> HostComponent {
    let mut source = String::from("Package \"test:numeric\"\n");
    for (suffix, ty) in [("i", "s64"), ("f", "f64")] {
        for operation in [
            "add", "sub", "mul", "div", "rem", "min", "max", "eq", "lt", "le", "gt", "ge",
        ] {
            if suffix == "f" && operation == "rem" {
                continue;
            }
            let result = if matches!(operation, "eq" | "lt" | "le" | "gt" | "ge") {
                "bool"
            } else {
                ty
            };
            source.push_str(&format!("Export {{name: {operation}-{suffix} params: {{a: {ty} b: {ty}}} result: {result}}}\nDef {operation}-{suffix} Fn {{a: {ty} b: {ty}}} {operation}(a b)\n"));
        }
    }
    source.push_str("Export invert\nDef invert Fn {value: bool} not(value)\n");
    let bytes = wavelet::staging::Project::compile(&source).unwrap();
    HostComponent::from_bytes(&bytes).unwrap()
}

fn invoke(c: &mut HostComponent, name: &str, args: &[Val]) -> Val {
    c.call_instance("test:numeric/api", name, args)
        .unwrap()
        .remove(0)
}

#[test]
fn integer_operations_preserve_all_sixty_four_bits() {
    let mut c = component();
    for (name, a, b, expected) in [
        ("add", i64::MAX, 1, i64::MIN),
        ("sub", i64::MIN, 1, i64::MAX),
        ("mul", i64::MAX, 2, -2),
        ("mul", i64::MIN, -1, i64::MIN),
        ("rem", i64::MIN, -1, 0),
        ("min", i64::MAX, i64::MAX - 1, i64::MAX - 1),
        ("max", i64::MAX - 1, i64::MAX, i64::MAX),
        ("div", -7, 2, -3),
        ("rem", -7, 2, -1),
    ] {
        assert_eq!(
            invoke(&mut c, &format!("{name}-i"), &[Val::S64(a), Val::S64(b)]),
            Val::S64(expected)
        );
        let oracle = wavelet::eval_snippet(&format!("{name}({a} {b})"));
        assert!(oracle.ok, "{}", oracle.error);
        assert_eq!(oracle.value, expected.to_string());
    }
    for (name, a, b, expected) in [
        ("lt", i64::MAX - 1, i64::MAX, true),
        ("le", i64::MAX, i64::MAX - 1, false),
        ("gt", i64::MIN + 1, i64::MIN, true),
        ("ge", i64::MIN, i64::MIN + 1, false),
        ("eq", i64::MAX - 1, i64::MAX, false),
    ] {
        assert_eq!(
            invoke(&mut c, &format!("{name}-i"), &[Val::S64(a), Val::S64(b)]),
            Val::Bool(expected)
        );
        let oracle = wavelet::eval_snippet(&format!("{name}({a} {b})"));
        assert!(oracle.ok, "{}", oracle.error);
        assert_eq!(oracle.value, expected.to_string());
    }
}

#[test]
fn floating_comparisons_and_extrema_follow_wasm() {
    let mut c = component();
    for name in ["eq", "lt", "le", "gt", "ge"] {
        for (a, b) in [(f64::NAN, 1.0), (1.0, f64::NAN), (f64::NAN, f64::NAN)] {
            assert_eq!(
                invoke(
                    &mut c,
                    &format!("{name}-f"),
                    &[Val::Float64(a), Val::Float64(b)]
                ),
                Val::Bool(false)
            );
        }
        let oracle = wavelet::eval_snippet(&format!("{name}(nan 1.0)"));
        assert!(oracle.ok, "{}", oracle.error);
        assert_eq!(oracle.value, "false");
    }
    for name in ["min", "max"] {
        for (a, b) in [(f64::NAN, 1.0), (1.0, f64::NAN)] {
            let Val::Float64(value) = invoke(
                &mut c,
                &format!("{name}-f"),
                &[Val::Float64(a), Val::Float64(b)],
            ) else {
                panic!()
            };
            assert!(value.is_nan());
        }
        for (a, b) in [(0.0, -0.0), (-0.0, 0.0)] {
            let Val::Float64(value) = invoke(
                &mut c,
                &format!("{name}-f"),
                &[Val::Float64(a), Val::Float64(b)],
            ) else {
                panic!()
            };
            assert_eq!(
                value.to_bits(),
                if name == "min" {
                    (-0.0_f64).to_bits()
                } else {
                    0.0_f64.to_bits()
                }
            );
        }
        let oracle = wavelet::eval_snippet(&format!("{name}(nan 1.0)"));
        assert!(oracle.ok, "{}", oracle.error);
        assert_eq!(oracle.value, "nan");
        for operands in ["0.0 -0.0", "-0.0 0.0"] {
            let oracle = wavelet::eval_snippet(&format!("div(1.0 {name}({operands}))"));
            assert!(oracle.ok, "{}", oracle.error);
            assert_eq!(oracle.value, if name == "min" { "-inf" } else { "inf" });
        }
    }
    for (source, expected) in [("min(1 2.0)", "1.0"), ("max(1.0 2)", "2.0")] {
        let oracle = wavelet::eval_snippet(source);
        assert!(oracle.ok, "{}", oracle.error);
        assert_eq!(oracle.value, expected);
    }
    assert_eq!(
        invoke(&mut c, "invert", &[Val::Bool(true)]),
        Val::Bool(false)
    );
    assert_eq!(
        invoke(&mut c, "invert", &[Val::Bool(false)]),
        Val::Bool(true)
    );
}

#[test]
fn wasm_integer_traps_remain_errors_in_both_engines() {
    for source in ["div(1 0)", "rem(1 0)", "div(-9223372036854775808 -1)"] {
        let oracle = wavelet::eval_snippet(source);
        assert!(!oracle.ok, "{source}");
        let operation = source.split('(').next().unwrap();
        let (a, b) = if source.contains("922337") {
            (i64::MIN, -1)
        } else {
            (1, 0)
        };
        let mut c = component();
        assert!(
            c.call_instance(
                "test:numeric/api",
                &format!("{operation}-i"),
                &[Val::S64(a), Val::S64(b)]
            )
            .is_err()
        );
    }
}

#[test]
fn floating_remainder_is_rejected() {
    for source in ["rem(5.5 2.0)", "rem(5 2.0)", "rem(5.5 2)"] {
        let result = wavelet::eval_snippet(source);
        assert!(!result.ok);
        assert!(
            result.error.contains("requires integer operands"),
            "{}",
            result.error
        );
        let source = format!("Package \"test:remainder\" Export run Def run Fn {{}} {source}");
        let error = wavelet::staging::Project::compile(&source).unwrap_err();
        assert!(
            error.to_string().contains("requires integer operands"),
            "{error}"
        );
    }
}
