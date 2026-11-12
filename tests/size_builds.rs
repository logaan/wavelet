use std::path::Path;
use wavelet::compiler::{self, Mode, Request, SourceFile};
use wavelet::host::{HostComponent, Val};

fn request() -> Request {
    Request {
        sources: vec![SourceFile {
            name: "small.wlt".into(),
            contents:
                "Package \"test:size\" Export add-two Def add-two Fn {a: s64 b: s64} add(a b)"
                    .into(),
        }],
        dependencies: vec![],
        entry: None,
        mode: Mode::Project,
    }
}

#[test]
fn optimizer_receives_core_module_before_component_metadata() {
    let mut called = false;
    let mut optimizer = |bytes: Vec<u8>| {
        called = true;
        assert_eq!(&bytes[..8], b"\0asm\x01\0\0\0");
        assert!(!bytes.windows(14).any(|part| part == b"component-type"));
        Ok(bytes)
    };
    let result =
        compiler::compile_with_optimizer(&request(), &mut compiler::NoMacros, &mut optimizer);
    assert!(result.is_ok());
    assert!(called);
}

#[test]
fn optimizer_failure_is_reported_without_fallback() {
    let errors = compiler::compile_with_optimizer(&request(), &mut compiler::NoMacros, &mut |_| {
        Err("optimizer unavailable".into())
    })
    .unwrap_err();
    assert_eq!(errors[0].phase, compiler::Phase::Emit);
    assert!(errors[0].message.contains("optimizer unavailable"));
}

#[test]
fn size_build_preserves_wrapping_arithmetic() {
    if std::process::Command::new("wasm-opt")
        .arg("--version")
        .output()
        .is_err()
    {
        assert!(std::env::var_os("WAVELET_REQUIRE_HOST_TOOLS").is_none());
        return;
    }
    let plain = compiler::native::compile(&request(), Path::new(".")).unwrap();
    let optimized = compiler::native::compile_for_size(&request(), Path::new(".")).unwrap();
    let bytes = &optimized.entry_artifact().unwrap().bytes;
    assert!(bytes.len() < plain.entry_artifact().unwrap().bytes.len());
    let mut component = HostComponent::from_bytes(bytes).unwrap();
    assert_eq!(
        component
            .call_instance(
                "test:size/api",
                "add-two",
                &[Val::S64(i64::MAX), Val::S64(1)]
            )
            .unwrap(),
        vec![Val::S64(i64::MIN)]
    );
}

#[test]
fn size_build_preserves_macro_component_exports() {
    if std::process::Command::new("wasm-opt")
        .arg("--version")
        .output()
        .is_err()
    {
        assert!(std::env::var_os("WAVELET_REQUIRE_HOST_TOOLS").is_none());
        return;
    }
    let mut request = request();
    request.sources[0].contents = "Package \"test:macro-size\" DefMacro identity {x} x".into();
    let compiled = compiler::native::compile_for_size(&request, Path::new(".")).unwrap();
    let mut component =
        wavelet::macros::MacroComponent::from_bytes(&compiled.artifacts[0].bytes).unwrap();
    assert_eq!(component.manifest().unwrap(), vec![("identity".into(), 1)]);
    let (arena, roots) = wavelet::read_file("identity(42)").unwrap();
    let input = wavelet::meta::arena_to_tree(&arena, roots[0]);
    let output = component.expand("identity", &input).unwrap();
    assert_eq!(
        output.nodes[output.root as usize],
        wavelet::meta::Node::IntVal(42)
    );
}

#[test]
fn components_without_formatting_do_not_embed_printer_data() {
    let mut optimizer = |bytes: Vec<u8>| {
        assert!(!bytes.windows(5).any(|part| part == b"cell("));
        Ok(bytes)
    };
    compiler::compile_with_optimizer(&request(), &mut compiler::NoMacros, &mut optimizer).unwrap();
}

#[test]
fn formatting_through_a_local_callback_keeps_printer_data() {
    let mut request = request();
    request.sources[0].contents = "Package \"test:size\" Export render Def format Fn {n: s64} to-string(n) Def render Fn {n: s64} Let {f: format} f(n)".into();
    let compiled = compiler::native::compile(&request, Path::new(".")).unwrap();
    let mut component =
        HostComponent::from_bytes(&compiled.entry_artifact().unwrap().bytes).unwrap();
    assert_eq!(
        component
            .call_instance("test:size/api", "render", &[Val::S64(42)])
            .unwrap(),
        vec![Val::String("42".into())]
    );
}

fn size_component(source: &str) -> Option<Vec<u8>> {
    if std::process::Command::new("wasm-opt")
        .arg("--version")
        .output()
        .is_err()
    {
        assert!(std::env::var_os("WAVELET_REQUIRE_HOST_TOOLS").is_none());
        return None;
    }
    let mut request = request();
    request.sources[0].contents = source.into();
    Some(
        compiler::native::compile_for_size(&request, Path::new("."))
            .unwrap()
            .entry_artifact()
            .unwrap()
            .bytes
            .clone(),
    )
}

fn memory_count(bytes: &[u8]) -> u32 {
    wasmparser::Parser::new(0)
        .parse_all(bytes)
        .map(|payload| match payload.unwrap() {
            wasmparser::Payload::MemorySection(section) => section.count(),
            _ => 0,
        })
        .sum()
}

#[test]
fn pure_scalar_size_build_omits_memory_and_cleanup() {
    let Some(bytes) = size_component("Package \"test:size\" Export run Def run Fn {} add(1 1)")
    else {
        return;
    };
    assert_eq!(memory_count(&bytes), 0);
    assert!(!bytes.windows(12).any(|part| part == b"cabi_realloc"));
    assert!(!bytes.windows(10).any(|part| part == b"cabi_post_"));
    let mut component = HostComponent::from_bytes(&bytes).unwrap();
    assert_eq!(
        component
            .call_instance("test:size/api", "run", &[])
            .unwrap(),
        vec![Val::S64(2)]
    );
}

#[test]
fn unused_string_and_list_parameters_keep_canonical_memory() {
    for (ty, input) in [
        ("string", Val::String("hello".into())),
        ("list(s64)", Val::List(vec![Val::S64(1), Val::S64(2)])),
    ] {
        let source = format!("Package \"test:size\" Export run Def run Fn {{value: {ty}}} 42");
        let Some(bytes) = size_component(&source) else {
            return;
        };
        assert!(memory_count(&bytes) > 0, "{ty} ABI requires memory");
        let mut component = HostComponent::from_bytes(&bytes).unwrap();
        assert_eq!(
            component
                .call_instance("test:size/api", "run", &[input])
                .unwrap(),
            vec![Val::S64(42)]
        );
    }
}

#[test]
fn allocating_scalar_computation_keeps_cleanup() {
    let Some(bytes) =
        size_component("Package \"test:size\" Export run Def run Fn {n: s64} len(range(0 n))")
    else {
        return;
    };
    assert!(memory_count(&bytes) > 0);
    assert!(bytes.windows(10).any(|part| part == b"cabi_post_"));
    let mut component = HostComponent::from_bytes(&bytes).unwrap();
    for count in [1000, 2, 1000] {
        assert_eq!(
            component
                .call_instance("test:size/api", "run", &[Val::S64(count)])
                .unwrap(),
            vec![Val::S64(count)]
        );
    }
}

#[test]
fn string_results_and_resources_keep_canonical_support() {
    let Some(bytes) = size_component("Package \"test:size\" Export run Def run Fn {} \"hello\"")
    else {
        return;
    };
    assert!(memory_count(&bytes) > 0);
    let mut component = HostComponent::from_bytes(&bytes).unwrap();
    assert_eq!(
        component
            .call_instance("test:size/api", "run", &[])
            .unwrap(),
        vec![Val::String("hello".into())]
    );
    let source = "Package \"test:size\" DefResource counter {New: Fn {start: u32} cell-new(start) value: Fn {self: counter} The u32 cell-get(self)} Export counter";
    let bytes = size_component(source).unwrap();
    assert!(memory_count(&bytes) > 0);
    assert!(bytes.windows(12).any(|part| part == b"cabi_realloc"));
}
