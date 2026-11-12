#[test]
fn unknown_runtime_types_are_rejected_by_default() {
    assert!(wavelet::check::strict());
    let result = wavelet::eval_snippet("Def identity Fn {x} x");
    assert!(
        !result.ok,
        "unknown runtime function types must be rejected"
    );
    assert!(
        result.error.contains("static type") || result.error.contains("function"),
        "{}",
        result.error
    );
}

#[test]
fn statically_resolved_functions_and_callbacks_remain_available() {
    for (source, expected) in [
        ("Let {g: Fn {n: s32} add(n 1)} g(41)", "42"),
        ("map(Fn {n} mul(n n) [2 3])", "[4, 9]"),
        ("fold(Fn {sum n} add(sum n) 0 [1 2 3])", "6"),
        ("Def inc Fn {n: s64} add(n 1)\nLet {f: inc} f(41)", "42"),
    ] {
        let result = wavelet::eval_snippet(source);
        assert!(result.ok, "{source}: {}", result.error);
        assert_eq!(result.value, expected);
    }
}

#[test]
fn runtime_storage_and_return_of_functions_are_rejected() {
    for source in [
        "[Fn {n: s64} add(n 1)]",
        "{operation: Fn {n: s64} add(n 1)}",
        "tuple1(Fn {n: s64} add(n 1))",
        "cell-new(Fn {n: s64} add(n 1))",
        "Def adder Fn {n: s64} Fn {x: s64} add(n x)\nadder(3)",
    ] {
        let result = wavelet::eval_snippet(source);
        assert!(!result.ok, "runtime function value was accepted: {source}");
        assert!(
            result.error.contains("function"),
            "{source}: {}",
            result.error
        );
    }
}

#[test]
fn documented_interpreter_successes_have_static_types() {
    let examples: serde_json::Value =
        serde_json::from_str(include_str!("../docs/examples.json")).unwrap();
    let mut failures = Vec::new();
    for (id, entry) in examples.as_object().unwrap() {
        if entry.get("error").is_some()
            || entry.get("sources").is_some()
            || entry["compiledOnly"] == true
        {
            continue;
        }
        let result = wavelet::eval_snippet(entry["code"].as_str().unwrap());
        if !result.ok {
            failures.push(format!("{id}: {}", result.error));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
