use std::path::Path;
use wasmtime::component::{Component, Linker, Resource};
use wasmtime::{Engine, Store};

mod bindings {
    wasmtime::component::bindgen!({
        path: "components/compiler/wit",
        world: "compiler-component",
    });
}

use bindings::exports::wavelet::compiler::compiler::{Mode, Request, SourceFile, Step};
use bindings::wavelet::compiler::macro_host::{Host, HostLibrary, Library};
use bindings::wavelet::meta::code::Tree;

struct HostState;

impl Host for HostState {}
impl bindings::wavelet::meta::code::Host for HostState {}

impl HostLibrary for HostState {
    fn manifest(&mut self, _: Resource<Library>) -> Result<Vec<(String, u32)>, String> {
        Err("No macro library supplied".into())
    }

    fn expand(&mut self, _: Resource<Library>, _: String, _: Tree) -> Result<Tree, String> {
        Err("No macro library supplied".into())
    }

    fn drop(&mut self, _: Resource<Library>) -> wasmtime::Result<()> {
        Ok(())
    }
}

fn compiler() -> (Store<HostState>, bindings::CompilerComponent) {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/src/compiler/wavelet_compiler.wasm");
    assert!(
        path.exists(),
        "run scripts/build-compiler-component.sh first"
    );
    let engine = Engine::default();
    let component = Component::from_file(&engine, path).unwrap();
    let mut linker = Linker::new(&engine);
    bindings::CompilerComponent::add_to_linker::<_, wasmtime::component::HasSelf<_>>(
        &mut linker,
        |state| state,
    )
    .unwrap();
    let mut store = Store::new(&engine, HostState);
    let instance =
        bindings::CompilerComponent::instantiate(&mut store, &component, &linker).unwrap();
    (store, instance)
}

fn snippet(source: &str) -> Request {
    Request {
        sources: vec![SourceFile {
            name: "example.wlt".into(),
            contents: source.into(),
        }],
        dependencies: vec![],
        entry: None,
        mode: Mode::Snippet,
    }
}

#[test]
fn component_compiles_a_snippet_and_its_output_executes() {
    let (mut store, compiler) = compiler();
    let result = compiler
        .wavelet_compiler_compiler()
        .call_compile(&mut store, &snippet("add(20 22)"), &[])
        .unwrap()
        .unwrap();
    let Step::Complete(compilation) = result else {
        panic!("unexpected macro request")
    };
    let artifact = compilation
        .artifacts
        .iter()
        .find(|artifact| Some(&artifact.name) == compilation.entry.as_ref())
        .unwrap();
    let invocation = compilation.invocation.unwrap();
    let mut component = wavelet::host::HostComponent::from_bytes(&artifact.bytes).unwrap();
    let result = component
        .call_instance(&invocation.interface_name, &invocation.function_name, &[])
        .unwrap();
    assert_eq!(result, vec![wavelet::host::Val::String("42".into())]);
}

#[test]
fn component_returns_structured_diagnostics_without_trapping() {
    let (mut store, compiler) = compiler();
    let errors = compiler
        .wavelet_compiler_compiler()
        .call_compile(&mut store, &snippet("unknown-function(1)"), &[])
        .unwrap()
        .unwrap_err();
    assert!(!errors.is_empty());
    assert_eq!(errors[0].source.as_deref(), Some("example.wlt"));
    assert!(!errors[0].message.is_empty());
    assert!(
        compiler
            .wavelet_compiler_compiler()
            .call_compile(&mut store, &snippet("42"), &[])
            .unwrap()
            .is_ok()
    );
}
