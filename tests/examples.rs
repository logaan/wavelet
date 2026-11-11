use serde_json::Value;

fn examples() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/examples.json");
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    serde_json::from_str(&text).expect("docs/examples.json is not valid JSON")
}

#[test]
fn every_documented_example_matches_the_interpreter() {
    let examples = examples();
    let map = examples
        .as_object()
        .expect("examples.json must be an object");
    assert!(!map.is_empty(), "no examples found");

    let mut failures = Vec::new();

    for (id, entry) in map {
        let code = entry["code"].as_str().unwrap_or_else(|| {
            panic!("example `{id}` has no string `code`");
        });
        let outcome = wavelet::eval_snippet(code);

        if let Some(expected_err) = entry.get("error").and_then(Value::as_str) {
            if outcome.ok {
                failures.push(format!(
                    "`{id}`: expected an error but it succeeded with value {:?}",
                    outcome.value
                ));
            } else if outcome.error != expected_err {
                failures.push(format!(
                    "`{id}`: error mismatch\n  expected: {expected_err}\n  actual:   {}",
                    outcome.error
                ));
            }
            continue;
        }

        if !outcome.ok {
            failures.push(format!(
                "`{id}`: expected success but got error: {}",
                outcome.error
            ));
            continue;
        }
        let want_value = entry["value"].as_str().unwrap_or("");
        let want_output = entry["output"].as_str().unwrap_or("");
        if outcome.value != want_value {
            failures.push(format!(
                "`{id}`: value mismatch\n  expected: {want_value:?}\n  actual:   {:?}",
                outcome.value
            ));
        }
        if outcome.output != want_output {
            failures.push(format!(
                "`{id}`: output mismatch\n  expected: {want_output:?}\n  actual:   {:?}",
                outcome.output
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "\n{} documented example(s) drifted from the interpreter:\n\n{}\n\n\
         If a language change made this intentional, regenerate the examples:\n  \
         wasm-pack build --target web --out-dir docs/src/wasm --out-name wavelet\n  \
         node docs/scripts/gen-examples.mjs\n",
        failures.len(),
        failures.join("\n\n"),
    );
}
