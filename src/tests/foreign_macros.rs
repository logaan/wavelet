use super::*;

fn fixture_macros_path() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/macros.wasm").to_string()
}

fn expand_with_foreign(src: &str) -> (Arena, Vec<form::NodeId>) {
    let root = env!("CARGO_MANIFEST_DIR");
    let (arena, roots) =
        macrodep::read_file_with_macros(src, root).expect("read with foreign macros");
    let mut foreign = macrodep::FileExpander::for_file(root, &arena, &roots);
    expand::expand_file(
        arena,
        &roots,
        foreign
            .as_mut()
            .map(|f| f as &mut dyn expand::ForeignExpander),
    )
    .expect("expand with foreign macros")
}

fn src_using_fixture(tail: &str) -> String {
    format!(
        "Package \"demo:app@0.1.0\"\n\
         Import {{pkg: \"acme:html/dsl\" macros: true from: \"{}\"}}\n\
         {tail}\n",
        fixture_macros_path()
    )
}

#[test]
fn foreign_macro_expands_and_splices_to_fixpoint() {
    let src = src_using_fixture(
        "Export run\n\
         Def run Fn {n: s64} Unless gt(n 0) 42",
    );
    let (arena, roots) = expand_with_foreign(&src);
    let printed: Vec<String> = roots.iter().map(|&r| print(&arena, r)).collect();
    assert!(
        printed.iter().all(|s| !s.contains("unless-MACRO")),
        "unless should be expanded away: {printed:?}"
    );
    assert!(
        printed.iter().any(|s| s
            .contains("(match-MACRO, (the-MACRO, bool, (gt, n, 0)), [(true, {}), (false, 42)])")),
        "unless must expand through If to its Match form, args spliced in: {printed:?}"
    );
}

#[test]
fn foreign_macro_expansion_is_re_expanded_to_fixpoint() {
    let src = src_using_fixture(
        "Export run\n\
         Def run Fn {n: s64} Unless gt(n 0) Identity add(n 1)",
    );
    let (arena, roots) = expand_with_foreign(&src);
    let printed: Vec<String> = roots.iter().map(|&r| print(&arena, r)).collect();
    assert!(
        printed
            .iter()
            .all(|s| !s.contains("unless-MACRO") && !s.contains("identity-MACRO")),
        "no foreign macro head may survive the fixpoint: {printed:?}"
    );

    assert!(
        printed.iter().any(|s| s.contains(
            "(match-MACRO, (the-MACRO, bool, (gt, n, 0)), [(true, {}), (false, (add, n, 1))])"
        )),
        "nested foreign macro in unless's body must be expanded too: {printed:?}"
    );
}

#[test]
fn foreign_identity_macro_expands_and_componentizes() {
    let src = src_using_fixture(
        "Export {name: run params: {n: s64} result: s64}\n\
         Def run Fn {n: s64} Identity add(n 1)",
    );
    let (arena, roots) = expand_with_foreign(&src);
    let printed: Vec<String> = roots.iter().map(|&r| print(&arena, r)).collect();
    assert!(
        printed.iter().any(|s| s.contains("(add, n, 1)")),
        "identity should yield its argument unchanged: {printed:?}"
    );
    assert!(
        printed.iter().all(|s| !s.contains("identity-MACRO")),
        "identity head should be gone: {printed:?}"
    );
    let info = wit::collect(&arena, &roots).expect("collect");
    let bytes = emit::emit_component(&arena, &roots, &info, &Default::default())
        .expect("foreign-expanded file componentizes");
    assert_eq!(&bytes[0..4], b"\0asm");
}

#[test]
fn foreign_macro_error_surfaces_with_macro_name() {
    let src = src_using_fixture("Boom");
    let root = env!("CARGO_MANIFEST_DIR");
    let (arena, roots) = macrodep::read_file_with_macros(&src, root).expect("read");
    let mut foreign = macrodep::FileExpander::for_file(root, &arena, &roots);
    let err = expand::expand_file(
        arena,
        &roots,
        foreign
            .as_mut()
            .map(|f| f as &mut dyn expand::ForeignExpander),
    )
    .expect_err("boom must fail expansion");
    assert!(err.contains("boom"), "error should name the macro: {err}");
    assert!(
        err.contains("expanding"),
        "error should be framed as an expansion failure: {err}"
    );
}

#[test]
fn foreign_macro_under_quote_is_not_expanded() {
    let src = src_using_fixture(r#"Quote Unless(false "ran")"#);
    let (arena, roots) = expand_with_foreign(&src);
    let printed: Vec<String> = roots.iter().map(|&r| print(&arena, r)).collect();
    assert!(
        printed.iter().any(|s| s.contains("unless-MACRO")),
        "quoted foreign macro call must be preserved verbatim: {printed:?}"
    );
}
