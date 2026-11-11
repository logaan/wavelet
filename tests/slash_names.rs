use wavelet::host::{HostComponent, Val};
use wavelet::staging::Project;

#[test]
fn slash_and_hyphen_spellings_have_identical_forms() {
    for (slash, hyphen) in [
        ("foo/bar", "foo-bar"),
        ("one/two/three(1)", "one-two-three(1)"),
        ("Quote (math/add 1 2)", "Quote (math-add 1 2)"),
        ("Def foo/bar Fn {x} x", "Def foo-bar Fn {x} x"),
        (
            "DefMacro lib-id {x} x\nlib/Id 42",
            "DefMacro lib-id {x} x\nlib-Id 42",
        ),
    ] {
        let canonical = |source| {
            let (arena, roots) = wavelet::read_file(source).unwrap();
            roots
                .iter()
                .map(|&root| wavelet::printer::print(&arena, root))
                .collect::<Vec<_>>()
        };
        assert_eq!(canonical(slash), canonical(hyphen));
    }
    assert_eq!(wavelet::eval_snippet("Quote foo/bar").value, "foo-bar");
}

#[test]
fn variant_constructor_alias_is_an_ordinary_function_value() {
    let body = r#"
DefType ttl [days(u32) forever]
Def ttl-days days
Def answer Fn {} Match ttl/days(5) [((days n) n) (forever 0)]
"#;
    let oracle = wavelet::eval_snippet(&format!("{body}\nanswer()"));
    assert!(oracle.ok, "{}", oracle.error);
    assert_eq!(oracle.value, "5");
    let bytes = Project::compile(&format!(
        "Package \"demo:aliases@0.1.0\"\n{body}\nExport {{name: answer result: u32}}"
    ))
    .unwrap();
    let mut component = HostComponent::from_bytes(&bytes).unwrap();
    let result = component
        .call_instance("demo:aliases/api@0.1.0", "answer", &[])
        .unwrap();
    assert!(matches!(result.as_slice(), [Val::U32(5)]));
}

#[test]
fn local_constructor_aliases_typecheck_their_arguments() {
    let outcome = wavelet::eval_snippet(
        "DefType ttl [days(u32) forever]\nDef ttl-days days\nttl/days(\"bad\")",
    );
    assert!(!outcome.ok);
}
