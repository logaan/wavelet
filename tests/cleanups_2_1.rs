use wavelet::{EvalOutcome, eval_snippet};

fn run(src: &str) -> EvalOutcome {
    eval_snippet(src)
}

#[test]
fn list_argument_does_not_spread_across_parameters() {
    let r = run("Def f Fn {a b} add(a b)\nf([20 22])");
    assert!(
        !r.ok,
        "a list must not spread across parameters (2.1 proposal 1); got {:?}",
        r.value
    );
}

#[test]
fn tuple_argument_still_binds_by_order() {
    let r = run("Def add2 Fn {a b} add(a b)\nadd2(20 22)");
    assert!(r.ok, "tuple bundle should still bind by order: {}", r.error);
    assert_eq!(r.value, "42");
}

#[test]
fn record_argument_still_binds_by_name() {
    let r = run("Def f Fn {a b} add(a b)\nf({a: 20 b: 22})");
    assert!(r.ok, "record should bind by name: {}", r.error);
    assert_eq!(r.value, "42");
}

#[test]
fn single_list_binds_to_a_sole_parameter() {
    let r = run("Def f Fn {xs: list(s64)} len(xs)\nf([1 2 3])");
    assert!(r.ok, "a list binds to a sole parameter: {}", r.error);
    assert_eq!(r.value, "3");
}

#[test]
fn empty_list_is_not_accepted_as_no_arguments() {
    let r = run("Def f Fn {} 42\nf([])");
    assert!(
        !r.ok,
        "an empty list must not stand in for no arguments (2.1 proposal 1); got {:?}",
        r.value
    );
}

#[test]
fn zero_parameter_call_still_works() {
    let r = run("Def answer Fn {} 42\nanswer()");
    assert!(r.ok, "a no-argument call should still work: {}", r.error);
    assert_eq!(r.value, "42");
}

#[test]
fn float_literal_pattern_is_a_check_error() {
    let r = run("Match 0.5 [(0.5 \"half\")\n(x \"other\")]");
    assert!(
        !r.ok,
        "a float-literal Match pattern must be a check error (2.1 proposal 2); got {:?}",
        r.value
    );
    assert!(
        r.error.contains("float"),
        "expected a float-pattern error, got: {}",
        r.error
    );
}

#[test]
fn nested_float_literal_pattern_is_a_check_error() {
    let r = run("Match [1.0 2.0] [([1.0 y] y)\n(other 0.0)]");
    assert!(
        !r.ok,
        "a nested float-literal pattern must be a check error (2.1 proposal 2); got {:?}",
        r.value
    );
    assert!(
        r.error.contains("float"),
        "expected a float-pattern error, got: {}",
        r.error
    );
}

#[test]
fn integer_literal_pattern_still_matches() {
    let r = run("Match 1 [(1 \"one\")\n(x \"other\")]");
    assert!(r.ok, "int literal pattern should still work: {}", r.error);
    assert_eq!(r.value, "\"one\"");
}

#[test]
fn string_literal_pattern_still_matches() {
    let r = run("Match \"hi\" [(\"hi\" true)\n(x false)]");
    assert!(
        r.ok,
        "string literal pattern should still work: {}",
        r.error
    );
    assert_eq!(r.value, "true");
}
