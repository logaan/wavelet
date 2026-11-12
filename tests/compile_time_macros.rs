#[test]
fn form_operations_are_rejected_in_runtime_expressions() {
    for source in [
        "Quote add(1 2)",
        "Quasi add(1 2)",
        "gensym()",
        "expand(42)",
        "form-kind(42)",
        "rec-key({x: 1})",
        "rec-val({x: 1})",
        "Def inspect Fn {x: s64} form-kind(x)\ninspect(42)",
    ] {
        let result = wavelet::eval_snippet(source);
        assert!(!result.ok, "accepted runtime macro operation: {source}");
        assert!(
            result.error.contains("compile time"),
            "{source}: {}",
            result.error
        );
    }
}

#[test]
fn macro_operations_execute_during_expansion() {
    for (source, expected) in [
        (
            "DefMacro inspect {} form-kind(Quote add(1 2))\nInspect",
            "\"tup\"",
        ),
        (
            "DefMacro names-differ {} not(eq(gensym() gensym()))\nNames-differ",
            "true",
        ),
        ("DefMacro field {r} rec-val(r)\nField {x: 42}", "42"),
        (
            "DefMacro increment {n} Quasi add(Unquote(n) 1)\nIncrement 41",
            "42",
        ),
    ] {
        let result = wavelet::eval_snippet(source);
        assert!(result.ok, "{source}: {}", result.error);
        assert_eq!(result.value, expected, "{source}");
    }
}

#[test]
fn expansion_does_not_add_runtime_code_or_state() {
    let plain =
        wavelet::staging::Project::compile("Package \"test:expanded\" Export run Def run Fn {} 42")
            .unwrap();
    let expanded = wavelet::staging::Project::compile(
        "Package \"test:expanded\" DefMacro constant {} 42 Export run Def run Fn {} Constant",
    )
    .unwrap();
    assert_eq!(expanded, plain);
}
