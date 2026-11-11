use wavelet::eval_snippet;

#[test]
fn removed_runtime_builtins_are_unbound() {
    for source in [r#"read("42")"#, "apply(Fn {x} x 42)"] {
        let outcome = eval_snippet(source);
        assert!(!outcome.ok, "{source} unexpectedly succeeded");
        assert!(outcome.error.contains("unbound"), "{}", outcome.error);
    }
}

#[test]
fn removed_names_can_be_user_functions() {
    for name in ["read", "apply"] {
        let outcome = eval_snippet(&format!("Def {name} Fn {{x}} add(x 1)\n{name}(41)"));
        assert!(outcome.ok, "{}", outcome.error);
        assert_eq!(outcome.value, "42");
    }
}
