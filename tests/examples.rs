use serde_json::Value;

fn examples() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/examples.json");
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    serde_json::from_str(&text).expect("docs/examples.json is not valid JSON")
}

#[test]
fn documented_interpreter_examples_match_reference_results() {
    let examples = examples();
    let map = examples
        .as_object()
        .expect("examples.json must be an object");
    assert!(!map.is_empty(), "no examples found");

    let mut failures = Vec::new();

    for (id, entry) in map {
        if entry.get("sources").is_some() || entry["compiledOnly"] == true {
            continue;
        }
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

#[test]
fn documented_compiled_examples_execute() {
    use wavelet::compiler::{self, Mode, Request, SourceFile};
    use wavelet::host::{HostComponent, Val};

    for (id, entry) in examples().as_object().unwrap() {
        if entry.get("sources").is_none() && entry["compiledOnly"] != true {
            continue;
        }
        let request = if let Some(sources) = entry["sources"].as_array() {
            Request {
                sources: sources
                    .iter()
                    .map(|source| SourceFile {
                        name: source["name"].as_str().unwrap().into(),
                        contents: source["contents"].as_str().unwrap().into(),
                    })
                    .collect(),
                dependencies: vec![],
                entry: Some(entry["entry"].as_str().unwrap().into()),
                mode: Mode::Project,
            }
        } else {
            Request::snippet(entry["code"].as_str().unwrap())
        };
        let compilation = compiler::native::compile(&request, std::path::Path::new("."))
            .unwrap_or_else(|errors| panic!("{id}: {errors:?}"));
        let mut host =
            HostComponent::from_bytes(&compilation.entry_artifact().unwrap().bytes).unwrap();
        let invocation = compilation
            .invocation
            .unwrap_or_else(|| compiler::Invocation {
                interface_name: entry["invocation"]["interfaceName"]
                    .as_str()
                    .unwrap()
                    .into(),
                function_name: entry["invocation"]["functionName"].as_str().unwrap().into(),
            });
        let results = host
            .call_instance(&invocation.interface_name, &invocation.function_name, &[])
            .unwrap();
        let [Val::String(value)] = results.as_slice() else {
            panic!("{id}: unexpected result {results:?}");
        };
        let actual = if request.mode == Mode::Project {
            serde_json::to_string(value).unwrap()
        } else {
            value.clone()
        };
        assert_eq!(actual, entry["value"].as_str().unwrap(), "{id}");
    }
}
