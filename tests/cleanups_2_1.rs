//! Regression tests for the two 2.1 code-review cleanups (confirmed by Logan
//! 2026-07-04), landed as part of the goal-5 docket.
//!
//!   * Proposal 1 — a single list argument no longer spreads positionally across
//!     an n-parameter function; a list is one value, bound to one parameter.
//!     The empty-list / flags / record "no arguments" acceptances are gone —
//!     only the empty tuple `()` denotes no arguments.
//!   * Proposal 2 — a float literal in Match pattern position is a check-time
//!     error. Int/bool/char/string literal patterns are unaffected.

use wavelet::{EvalOutcome, eval_snippet};

fn run(src: &str) -> EvalOutcome {
    eval_snippet(src)
}

// ── Proposal 1: list no longer spreads across parameters ──

#[test]
fn list_argument_does_not_spread_across_parameters() {
    // A two-element list passed to a two-parameter function is one value bound
    // to the first parameter, leaving the second unbound — a bind error.
    let r = run("apply(Fn {a b} add(a b) [20 22])");
    assert!(
        !r.ok,
        "a list must not spread across parameters (2.1 proposal 1); got {:?}",
        r.value
    );
}

#[test]
fn tuple_argument_still_binds_by_order() {
    // The tuple bundle from a ≥2-arg call still binds positionally.
    let r = run("Def add2 Fn {a b} add(a b)\nadd2(20 22)");
    assert!(r.ok, "tuple bundle should still bind by order: {}", r.error);
    assert_eq!(r.value, "42");
}

#[test]
fn record_argument_still_binds_by_name() {
    let r = run("apply(Fn {a b} add(a b) {a: 20 b: 22})");
    assert!(r.ok, "record should bind by name: {}", r.error);
    assert_eq!(r.value, "42");
}

#[test]
fn single_list_binds_to_a_sole_parameter() {
    // With one parameter, a list is simply the one value bound to it.
    let r = run("apply(Fn {xs} len(xs) [1 2 3])");
    assert!(r.ok, "a list binds to a sole parameter: {}", r.error);
    assert_eq!(r.value, "3");
}

#[test]
fn empty_list_is_not_accepted_as_no_arguments() {
    // Only the empty tuple denotes "no arguments"; an empty list is a value.
    let r = run("apply(Fn {} 42 [])");
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

// ── Proposal 2: float literals are rejected as Match patterns ──

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
    // Also rejected inside a compound pattern.
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
    assert!(r.ok, "string literal pattern should still work: {}", r.error);
    assert_eq!(r.value, "true");
}
