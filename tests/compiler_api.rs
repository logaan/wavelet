use wavelet::compiler::{
    self, ArtifactKind, MacroHost, Mode, NoMacros, Phase, Request, SourceFile, Step,
};
use wavelet::host::{HostComponent, Val};

fn complete(request: &Request) -> compiler::Compilation {
    match compiler::compile(request, &mut NoMacros).expect("compile in memory") {
        Step::Complete(compilation) => compilation,
        Step::NeedsMacros(_) => panic!("unexpected macro request"),
    }
}

fn evaluate(compilation: &compiler::Compilation) -> String {
    let artifact = compilation.entry_artifact().unwrap();
    let invocation = compilation.invocation.as_ref().unwrap();
    let mut host = HostComponent::from_bytes(&artifact.bytes).unwrap();
    match host
        .call_instance(&invocation.interface_name, &invocation.function_name, &[])
        .unwrap()
        .as_slice()
    {
        [Val::String(value)] => value.clone(),
        values => panic!("unexpected result {values:?}"),
    }
}

#[test]
fn snippets_compile_deterministically_without_files() {
    let request = Request::snippet("Def twice Fn {x: s64} mul(x 2)\ntwice(21)");
    let first = complete(&request);
    let second = complete(&request);
    assert_eq!(
        first.entry_artifact().unwrap().bytes,
        second.entry_artifact().unwrap().bytes
    );
    assert_eq!(evaluate(&first), "42");
}

#[test]
fn virtual_projects_compose_in_memory() {
    let request = Request {
        sources: vec![
            SourceFile { name: "lib.wlt".into(), contents: "Package \"memory:math@1.0.0\"\nExport {name: double params: {x: s64} result: s64}\nDef double Fn {x: s64} mul(x 2)".into() },
            SourceFile { name: "main.wlt".into(), contents: "Package \"memory:app@1.0.0\"\nImport {pkg: \"memory:math/api@1.0.0\" as: math}\nExport {name: answer result: s64}\nDef answer Fn {} math/double(21)".into() },
        ], dependencies: vec![], entry: Some("main.wlt".into()), mode: Mode::Project,
    };
    let compilation = complete(&request);
    let artifact = compilation.entry_artifact().unwrap();
    assert_eq!(artifact.kind, ArtifactKind::Application);
    let mut host = HostComponent::from_bytes(&artifact.bytes).unwrap();
    assert!(matches!(
        host.call_instance("memory:app/api@1.0.0", "answer", &[])
            .unwrap()
            .as_slice(),
        [Val::S64(42)]
    ));
}

#[test]
fn local_macro_compilation_uses_the_preparation_handshake() {
    let request = Request::snippet("DefMacro twice {x} Quasi add(Unquote(x) Unquote(x))\nTwice 21");
    let Step::NeedsMacros(requests) = compiler::compile(&request, &mut NoMacros).unwrap() else {
        panic!("expected capability request")
    };
    assert_eq!(requests.len(), 1);
    assert!(requests[0].key.starts_with("local:"));
    let mut host = compiler::native::NativeMacroHost::new(".");
    for request in requests {
        host.provide(request).unwrap();
    }
    let Step::Complete(compilation) = compiler::compile(&request, &mut host).unwrap() else {
        panic!("capability did not satisfy request")
    };
    assert_eq!(evaluate(&compilation), "42");
}

struct BrokenMacro;

impl MacroHost for BrokenMacro {
    fn manifest(&mut self, _: &str) -> Option<Result<Vec<(String, u32)>, String>> {
        Some(Ok(vec![("broken".into(), 1)]))
    }
    fn expand(
        &mut self,
        _: &str,
        _: &str,
        _: &wavelet::meta::Tree,
    ) -> Result<wavelet::meta::Tree, String> {
        Ok(wavelet::meta::Tree {
            nodes: vec![wavelet::meta::Node::Lst(vec![0])],
            root: 0,
            spans: vec![(0, 0)],
        })
    }
}

#[test]
fn malformed_macro_trees_return_diagnostics_without_panicking() {
    let request =
        Request::snippet("Import {pkg: \"memory:macros/api@1.0.0\" macros: true}\nBroken 1");
    let errors = compiler::compile(&request, &mut BrokenMacro).unwrap_err();
    assert_eq!(errors[0].phase, Phase::Expand);
    assert!(errors[0].message.contains("cycle"));
}

#[test]
fn missing_imported_macros_return_explicit_references() {
    let request = Request::snippet(
        "Import {pkg: \"memory:macros/api@1.2.3\" macros: true from: \"supplied.wasm\"}\nBroken 1",
    );
    let Step::NeedsMacros(requests) = compiler::compile(&request, &mut NoMacros).unwrap() else {
        panic!("expected imported capability")
    };
    let compiler::MacroSource::Imported(import) = &requests[0].source else {
        panic!("expected import reference")
    };
    assert_eq!(import.package_id, "memory:macros@1.2.3");
    assert_eq!(import.source_path.as_deref(), Some("supplied.wasm"));
}

#[test]
fn invalid_inputs_and_sources_have_structured_diagnostics() {
    let mut request = Request::snippet("add(1 true)");
    let errors = compiler::compile(&request, &mut NoMacros).unwrap_err();
    assert_eq!(errors[0].phase, Phase::Check);
    assert_eq!(errors[0].source.as_deref(), Some("snippet.wlt"));
    request.sources[0].name = "../outside.wlt".into();
    assert_eq!(
        compiler::compile(&request, &mut NoMacros).unwrap_err()[0].phase,
        Phase::Input
    );
    request.sources.clear();
    assert_eq!(
        compiler::compile(&request, &mut NoMacros).unwrap_err()[0].message,
        "no input files"
    );
}

#[test]
fn project_macro_sources_supply_generated_capabilities() {
    let mut request =
        Request::snippet("Import {pkg: \"memory:macros/api@1.0.0\" macros: true}\nIdentity 42");
    request.mode = Mode::Project;
    request.sources[0].contents = "Package \"memory:app@1.0.0\"\nImport {pkg: \"memory:macros/api@1.0.0\" macros: true}\nExport {name: answer result: s64}\nDef answer Fn {} Identity 42".into();
    request.sources.push(SourceFile {
        name: "macros.wlt".into(),
        contents: "Package \"memory:macros@1.0.0\"\nDefMacro identity {x} x".into(),
    });
    let Step::NeedsMacros(requests) = compiler::compile(&request, &mut NoMacros).unwrap() else {
        panic!("expected preparation")
    };
    assert!(matches!(
        requests[0].source,
        compiler::MacroSource::Component(_)
    ));
    let compilation = compiler::native::compile(&request, std::path::Path::new(".")).unwrap();
    let mut host = HostComponent::from_bytes(&compilation.entry_artifact().unwrap().bytes).unwrap();
    assert!(matches!(
        host.call_instance("memory:app/api@1.0.0", "answer", &[])
            .unwrap()
            .as_slice(),
        [Val::S64(42)]
    ));
}

#[test]
fn virtual_wit_dependencies_preserve_custom_versions_and_interfaces() {
    let mut request =
        Request::snippet("Import {pkg: \"memory:types/calc@1.2.3\" as: calc}\ncalc/double(21)");
    request.dependencies.push(compiler::DependencyFile {
        name: "types/package.wit".into(),
        contents: b"package memory:types@1.2.3; interface calc { double: func(x: s64) -> s64; }"
            .to_vec(),
    });
    let compilation = complete(&request);
    let artifact = compilation.entry_artifact().unwrap();
    assert!(!artifact.bytes.is_empty());
    request.dependencies[0].contents =
        b"package memory:types@9.0.0; interface calc { double: func(x: s64) -> s64; }".to_vec();
    let errors = compiler::compile(&request, &mut NoMacros).unwrap_err();
    assert_eq!(errors[0].phase, Phase::Wit);
    assert!(errors[0].message.contains("memory:types@1.2.3"));
}

#[test]
fn reader_diagnostics_include_positions_and_invalid_dependency_files_fail() {
    let errors = compiler::compile(&Request::snippet("[1"), &mut NoMacros).unwrap_err();
    assert_eq!(errors[0].phase, Phase::Read);
    assert!(errors[0].span.is_some());
    let mut request = Request::snippet("42");
    request.dependencies.push(compiler::DependencyFile {
        name: "package.txt".into(),
        contents: vec![],
    });
    assert_eq!(
        compiler::compile(&request, &mut NoMacros).unwrap_err()[0].phase,
        Phase::Wit
    );
    request.dependencies[0].name = "broken.wit".into();
    assert_eq!(
        compiler::compile(&request, &mut NoMacros).unwrap_err()[0].phase,
        Phase::Wit
    );
}

#[test]
fn request_validation_rejects_duplicate_names_unknown_entries_and_project_forms_in_snippets() {
    let mut request = Request::snippet("42");
    request.mode = Mode::Project;
    request.sources.push(request.sources[0].clone());
    assert!(
        compiler::compile(&request, &mut NoMacros).unwrap_err()[0]
            .message
            .contains("duplicate")
    );
    request.mode = Mode::Snippet;
    assert!(
        compiler::compile(&request, &mut NoMacros).unwrap_err()[0]
            .message
            .contains("exactly one")
    );
    request.sources.pop();
    request.entry = Some("missing.wlt".into());
    assert!(
        compiler::compile(&request, &mut NoMacros).unwrap_err()[0]
            .message
            .contains("entry")
    );
    request.entry = None;
    request.sources[0].contents = "Package \"memory:app\"".into();
    assert_eq!(
        compiler::compile(&request, &mut NoMacros).unwrap_err()[0].phase,
        Phase::Input
    );
}

#[test]
fn macro_tree_validation_rejects_missing_spans_and_invalid_references() {
    use wavelet::meta::{Node, Tree, validate_tree};
    for tree in [
        Tree {
            nodes: vec![],
            root: 0,
            spans: vec![],
        },
        Tree {
            nodes: vec![Node::IntVal(1)],
            root: 0,
            spans: vec![],
        },
        Tree {
            nodes: vec![Node::Lst(vec![1])],
            root: 0,
            spans: vec![(0, 0)],
        },
    ] {
        assert!(validate_tree(&tree).is_err());
    }
    let tree = Tree {
        nodes: vec![
            Node::IntVal(42),
            Node::Rec(vec![("a".into(), 0), ("b".into(), 0)]),
        ],
        root: 1,
        spans: vec![(0, 0); 2],
    };
    validate_tree(&tree).unwrap();
}

#[derive(Default)]
struct CounterMacroHost {
    available: std::collections::HashSet<String>,
    expansions: usize,
}

impl MacroHost for CounterMacroHost {
    fn manifest(&mut self, key: &str) -> Option<Result<Vec<(String, u32)>, String>> {
        self.available
            .contains(key)
            .then(|| Ok(vec![("tick".into(), 1)]))
    }

    fn expand(
        &mut self,
        _: &str,
        _: &str,
        _: &wavelet::meta::Tree,
    ) -> Result<wavelet::meta::Tree, String> {
        self.expansions += 1;
        Ok(wavelet::meta::Tree {
            nodes: vec![wavelet::meta::Node::IntVal(self.expansions as i64)],
            root: 0,
            spans: vec![(0, 0)],
        })
    }
}

#[test]
fn preparation_retries_do_not_execute_or_replay_stateful_macro_expansions() {
    let declarations = [
        "Import {pkg: \"test:first/api@1.0.0\" macros: true}",
        "DefMacro tick {x} x",
        "Import {pkg: \"test:last/api@1.0.0\" macros: true}",
    ];
    let request = Request {
        sources: declarations
            .iter()
            .enumerate()
            .map(|(index, declaration)| SourceFile {
                name: format!("source-{index}.wlt"),
                contents: format!(
                    "Package \"test:unit-{index}@1.0.0\"\n{declaration}\nExport {{name: answer result: s64}}\nDef answer Fn {{}} Tick 0"
                ),
            })
            .collect(),
        dependencies: vec![],
        entry: None,
        mode: Mode::Project,
    };
    let mut host = CounterMacroHost::default();
    for _ in 0..3 {
        let Step::NeedsMacros(requests) = compiler::compile(&request, &mut host).unwrap() else {
            panic!("expected preparation")
        };
        assert_eq!(
            host.expansions, 0,
            "preparation must not execute expansions"
        );
        assert_eq!(requests.len(), 1);
        assert!(host.available.insert(requests[0].key.clone()));
    }
    let Step::Complete(output) = compiler::compile(&request, &mut host).unwrap() else {
        panic!("all capabilities were supplied")
    };
    assert_eq!(host.expansions, 3);
    assert_eq!(output.artifacts.len(), 3);
    for (index, artifact) in output.artifacts.iter().enumerate() {
        let mut component = HostComponent::from_bytes(&artifact.bytes).unwrap();
        let values = component
            .call_instance(&format!("test:unit-{index}/api@1.0.0"), "answer", &[])
            .unwrap();
        assert!(matches!(values.as_slice(), [Val::S64(value)] if *value == index as i64 + 1));
    }
}
