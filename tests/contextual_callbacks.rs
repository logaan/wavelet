use wavelet::eval_snippet;

#[test]
fn callbacks_infer_parameters_from_sequences_under_strict_checking() {
    wavelet::check::set_strict(true);
    for (source, expected) in [
        ("map(Fn {x} mul(x x) [1 2 3])", "[1, 4, 9]"),
        ("DefType numbers list(s64)\nDef f Fn {xs: numbers} map(Fn {x} add(x 1) xs)\nf([1 2])", "[2, 3]"),
        ("filter(Fn {x} gt(x 1) [1 2 3])", "[2, 3]"),
        ("fold(Fn {a x} add(a x) 0 [1 2 3])", "6"),
        ("map(Fn {x} upper(x) [\"a\" \"b\"])", "[\"A\", \"B\"]"),
    ] {
        let outcome = eval_snippet(source);
        assert!(outcome.ok, "{source}: {}", outcome.error);
        assert_eq!(outcome.value, expected);
    }
    for source in [
        "filter(Fn {x} add(x 1) [1 2])",
        "map(Fn {x: string} upper(x) [1 2])",
        "fold(Fn {a x} upper(x) 0 [1 2])",
        "map(Fn {x y} add(x y) [1 2])",
    ] {
        let outcome = eval_snippet(source);
        assert!(!outcome.ok, "{source} unexpectedly succeeded");
    }
    wavelet::check::set_strict(false);
}
