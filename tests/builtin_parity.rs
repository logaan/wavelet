use wavelet::compiler::{Request, native};
use wavelet::host::{HostComponent, Val};

fn agrees(source: &str) {
    let reference = wavelet::eval_snippet(source);
    assert!(
        reference.ok,
        "oracle failed for {source}: {}",
        reference.error
    );
    let compilation = native::compile(&Request::snippet(source), std::path::Path::new("."))
        .unwrap_or_else(|errors| panic!("{source}: {}", native::display_diagnostics(errors)));
    let artifact = compilation.entry_artifact().unwrap();
    let invocation = compilation.invocation.as_ref().unwrap();
    let mut component = HostComponent::from_bytes(&artifact.bytes).unwrap();
    let values = component
        .call_instance(&invocation.interface_name, &invocation.function_name, &[])
        .unwrap_or_else(|error| panic!("{source}: {error}"));
    let [Val::String(value)] = values.as_slice() else {
        panic!("unexpected {values:?}")
    };
    assert_eq!(value, &reference.value, "{source}");
}

#[test]
fn strings_concatenate_characters_and_sequence_payloads() {
    for source in [
        "str-cat(\"a\" 'b')",
        "str-cat('é' '☃' '😀' '\\n' '\\\\' '\\'')",
        "str-cat('😀')",
        "str-cat([\"a\" \"b\"])",
        "str-cat(['a' '☃'])",
        "str-cat(tuple2(\"a\" 'b'))",
        "str-cat([])",
        "str-cat()",
        "Def cat Fn {a: string b: char} str-cat(a b)\ncat(\"snow\" '☃')",
    ] {
        agrees(source);
    }
}

#[test]
fn collection_length_counts_tuple_elements_and_unicode_scalars() {
    for source in [
        "len(tuple2(1 \"a\"))",
        "len(tuple0())",
        "len(\"☃😀aé\")",
        "len(\"\")",
        "Def count Fn {s: string} len(s)\ncount(\"café\")",
    ] {
        agrees(source);
    }
}

#[test]
fn tuple_indexing_infers_constant_and_homogeneous_dynamic_results() {
    for source in [
        "get(tuple2(1 2) 0)",
        "get(tuple2(1 \"a\") 1)",
        "Def choose Fn {index: s64} get(tuple2(10 20) index)\nchoose(1)",
    ] {
        agrees(source);
    }
}

#[test]
fn statically_known_builtin_callbacks_are_specialized() {
    for source in [
        "fold(add 0 [1 2 3 4])",
        "fold(mul 1 [2 3 4])",
        "map(neg [1 2 3])",
        "map(to-string [1 2])",
        "map(len [\"a\" \"☃\"])",
        "filter(not [true false true])",
        "Def add Fn {a: s64 b: s64} sub(a b)\nfold(add 20 [1 2])",
    ] {
        agrees(source);
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
