use wavelet::host::{HostComponent, Val};

fn compiled_eval(code: &str) -> Result<(String, String), String> {
    let compilation = wavelet::compiler::native::compile(
        &wavelet::compiler::Request::snippet(code),
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
    )
    .map_err(wavelet::compiler::native::display_diagnostics)?;
    let artifact = compilation.entry_artifact().ok_or("no entry artifact")?;
    let invocation = compilation
        .invocation
        .as_ref()
        .ok_or("no snippet invocation")?;
    let mut component =
        HostComponent::from_bytes(&artifact.bytes).map_err(|e| format!("instantiate: {e}"))?;
    wavelet::output_capture_start();
    let result =
        component.call_instance(&invocation.interface_name, &invocation.function_name, &[]);
    let output = wavelet::output_capture_take();
    let vals = result.map_err(|e| format!("call: {e}"))?;
    match vals.as_slice() {
        [Val::String(s)] => Ok((s.to_string(), output)),
        other => Err(format!("call: unexpected result shape {other:?}")),
    }
}

enum Outcome {
    Agree,

    Disagree(String),
}

fn run_example(id: &str, code: &str, expect_error: bool) -> Outcome {
    let interp = wavelet::eval_snippet(code);
    assert_eq!(
        interp.ok, !expect_error,
        "`{id}`: interpreter outcome drifted from examples.json — \
         tests/examples.rs should be failing too"
    );

    let compiled = compiled_eval(code);

    match (expect_error, compiled) {
        (true, Err(_)) => Outcome::Agree,
        (true, Ok(v)) => Outcome::Disagree(format!(
            "interpreter errors (`{}`) but the compiled artifact succeeds with {v:?}",
            interp.error
        )),
        (false, Err(e)) => Outcome::Disagree(format!(
            "interpreter succeeds with {:?} but the compiled artifact fails at {e}",
            interp.value
        )),
        (false, Ok((v, output))) => {
            if output != interp.output {
                return Outcome::Disagree(format!(
                    "stdout mismatch: {:?} != {output:?}",
                    interp.output
                ));
            }

            if v == interp.value || (interp.value.is_empty() && v == "{}") {
                Outcome::Agree
            } else {
                Outcome::Disagree(format!(
                    "value mismatch\n    interpreter: {:?}\n    compiled:    {v:?}",
                    interp.value
                ))
            }
        }
    }
}

#[test]
fn every_documented_example_agrees_with_the_compiled_artifact() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/examples.json");
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    let examples: serde_json::Value =
        serde_json::from_str(&text).expect("docs/examples.json is not valid JSON");
    let map = examples
        .as_object()
        .expect("examples.json must be an object");
    assert!(!map.is_empty(), "no examples found");

    let mut ids: Vec<&String> = map.keys().collect();
    ids.sort();

    let mut divergences = Vec::new();
    for id in ids {
        let entry = &map[id.as_str()];
        let code = entry["code"]
            .as_str()
            .unwrap_or_else(|| panic!("example `{id}` has no string `code`"));
        if let Outcome::Disagree(how) = run_example(id, code, entry.get("error").is_some()) {
            divergences.push(format!("`{id}`: {how}"));
        }
    }
    let report = divergences.join("\n\n");
    assert!(report.is_empty(), "{report}");
}

#[test]
fn independent_execution_corpus_agrees() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/execution");
    let mut paths: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();
    assert!(!paths.is_empty());
    for path in paths {
        let code = std::fs::read_to_string(&path).unwrap();
        let id = path.file_stem().unwrap().to_str().unwrap();
        if let Outcome::Disagree(message) = run_example(id, &code, id.ends_with("-error")) {
            panic!("{id}: {message}");
        }
    }
}
