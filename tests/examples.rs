use serde_json::Value;

fn examples() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/examples.json");
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    serde_json::from_str(&text).expect("docs/examples.json is not valid JSON")
}

#[test]
fn documented_compiled_examples_execute() {
    use wavelet::compiler::{self, Mode, Request, SourceFile};
    use wavelet::host::{HostComponent, Val};

    for (id, entry) in examples().as_object().unwrap() {
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
        let compilation = compiler::native::compile(&request, std::path::Path::new("."));
        if entry.get("error").is_some() {
            if let Ok(compilation) = compilation {
                let mut host =
                    HostComponent::from_bytes(&compilation.entry_artifact().unwrap().bytes)
                        .unwrap();
                let invocation = compilation.invocation.unwrap();
                assert!(
                    host.call_instance(&invocation.interface_name, &invocation.function_name, &[])
                        .is_err(),
                    "{id}: expected an error"
                );
            }
            continue;
        }
        let compilation = compilation.unwrap_or_else(|errors| panic!("{id}: {errors:?}"));
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
