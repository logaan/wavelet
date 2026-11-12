use wavelet::compiler::{Request, native};

fn evaluates(source: &str, expected: &str) {
    let actual = wavelet::eval_snippet(source);
    assert!(actual.ok, "{source}: {}", actual.error);
    assert_eq!(actual.value, expected, "{source}");
}

#[test]
fn strings_concatenate_characters_and_sequence_payloads() {
    for (source, expected) in [
        (r#"str-cat("a" 'b')"#, r#""ab""#),
        (r#"str-cat('é' '☃' '😀')"#, r#""é☃😀""#),
        (r#"str-cat('😀')"#, r#""😀""#),
        (r#"str-cat(["a" "b"])"#, r#""ab""#),
        (r#"str-cat(['a' '☃'])"#, r#""a☃""#),
        (r#"str-cat(tuple2("a" 'b'))"#, r#""ab""#),
        ("str-cat([])", r#""""#),
        ("str-cat()", r#""""#),
        (
            r#"Def cat Fn {a: string b: char} str-cat(a b)
cat("snow" '☃')"#,
            r#""snow☃""#,
        ),
    ] {
        evaluates(source, expected);
    }
}

#[test]
fn collection_length_counts_tuple_elements_and_unicode_scalars() {
    for (source, expected) in [
        (r#"len(tuple2(1 "a"))"#, "2"),
        ("len(tuple0())", "0"),
        (r#"len("☃😀aé")"#, "4"),
        (r#"len("")"#, "0"),
        (
            r#"Def count Fn {s: string} len(s)
count("café")"#,
            "4",
        ),
    ] {
        evaluates(source, expected);
    }
}

#[test]
fn tuple_indexing_infers_constant_and_homogeneous_dynamic_results() {
    for (source, expected) in [
        ("get(tuple2(1 2) 0)", "1"),
        (r#"get(tuple2(1 "a") 1)"#, r#""a""#),
        (
            "Def choose Fn {index: s64} get(tuple2(10 20) index)\nchoose(1)",
            "20",
        ),
    ] {
        evaluates(source, expected);
    }
}

#[test]
fn statically_known_builtin_callbacks_are_specialized() {
    for (source, expected) in [
        ("fold(add 0 [1 2 3 4])", "10"),
        ("fold(mul 1 [2 3 4])", "24"),
        ("map(neg [1 2 3])", "[-1, -2, -3]"),
        ("map(to-string [1 2])", r#"["1", "2"]"#),
        (r#"map(len ["a" "☃"])"#, "[1, 1]"),
        ("filter(not [true false true])", "[false]"),
        (
            "Def add Fn {a: s64 b: s64} sub(a b)\nfold(add 20 [1 2])",
            "17",
        ),
    ] {
        evaluates(source, expected);
    }
}

#[test]
fn derive_is_rejected_without_synthesizing_placeholder_operations() {
    for classes in ["Eq", "Ord", "Show", "Hash", "Eq Ord Show Hash"] {
        let source = format!("DefType point {{x: s32 y: s32}}\nDerive {{{classes}}} point\n42");
        let error =
            native::compile(&Request::snippet(source), std::path::Path::new(".")).unwrap_err();
        assert!(
            error[0].message.contains("`Derive` is not implemented"),
            "{error:?}"
        );
    }
}

#[test]
fn canonical_derive_forms_preserve_their_titlecase_flag_context() {
    let (arena, roots) = wavelet::reader::read_file("Derive {Eq Ord Show} point").unwrap();
    let printed = wavelet::printer::print(&arena, roots[0]);
    let (roundtrip, roots) = wavelet::reader::read_file(&printed).unwrap();
    assert_eq!(wavelet::printer::print(&roundtrip, roots[0]), printed);
    assert!(wavelet::reader::read_file("{Eq Ord}").is_err());
}
