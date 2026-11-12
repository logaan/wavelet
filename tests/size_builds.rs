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
