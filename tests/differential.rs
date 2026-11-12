#[test]
fn execution_corpus_has_explicit_expected_results() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/execution");
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/execution.json")).unwrap();
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let id = path.file_stem().unwrap().to_str().unwrap();
        let expectation = expected
            .get(id)
            .unwrap_or_else(|| panic!("missing explicit expectation: {id}"));
        let code = std::fs::read_to_string(&path).unwrap();
        let actual = wavelet::eval_snippet(&code);
        if expectation.is_null() {
            assert!(!actual.ok, "{id}: expected failure, got {}", actual.value);
        } else {
            assert!(actual.ok, "{id}: {}", actual.error);
            assert_eq!(actual.value, expectation.as_str().unwrap(), "{id}");
        }
    }
}
