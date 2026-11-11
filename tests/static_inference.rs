#[test]
fn recursive_numeric_parameters_and_results_are_inferred() {
    let result = wavelet::eval_snippet(
        "Def sum Fn {n total} If eq(n 0) total sum(sub(n 1) add(total n))\nsum(100 0)",
    );
    assert!(result.ok, "{}", result.error);
    assert_eq!(result.value, "5050");
}

#[test]
fn cells_keep_their_static_value_type_after_mutation() {
    let good = wavelet::eval_snippet("Let {c: cell-new(1)} Do [cell-set(c 41) add(cell-get(c) 1)]");
    assert!(good.ok, "{}", good.error);
    assert_eq!(good.value, "42");
    let bad = wavelet::eval_snippet("Let {c: cell-new(1)} cell-set(c \"changed\")");
    assert!(!bad.ok);
    assert!(bad.error.contains("stored type"), "{}", bad.error);
}

#[test]
fn cell_reads_do_not_retain_the_initial_literal_range() {
    let result = wavelet::eval_snippet(
        "Def byte Fn {n: u8} n\nLet {c: cell-new(0)} Do [cell-set(c 1000) byte(cell-get(c))]",
    );
    assert!(!result.ok);
    assert!(result.error.contains("wrong type"), "{}", result.error);
}

#[test]
fn unresolved_local_call_is_not_reported_as_an_unbound_name() {
    let result = wavelet::eval_snippet("Def invoke Fn {f} f(1)");
    assert!(!result.ok);
    assert!(
        result.error.contains("cannot infer a static type"),
        "{}",
        result.error
    );
    assert!(!result.error.contains("unbound"));
}

#[test]
fn runtime_function_selection_is_rejected() {
    for source in [
        "Let {f: If true Fn {n: s64} add(n 1) Fn {n: s64} sub(n 1)} f(1)",
        "map(If true Fn {n: s64} add(n 1) Fn {n: s64} sub(n 1) [1 2])",
    ] {
        let result = wavelet::eval_snippet(source);
        assert!(!result.ok, "{source}");
        assert!(
            result.error.contains("static function")
                || result.error.contains("runtime function selection"),
            "{}",
            result.error
        );
    }
}
