use wavelet::compiler::{self, Mode, NoMacros, Request, Step};
use wavelet::host::{HostComponent, Val};

fn compile(source: &str, mode: Mode) -> compiler::Compilation {
    let mut request = Request::snippet(source);
    request.mode = mode;
    let Step::Complete(compilation) = compiler::compile(&request, &mut NoMacros).unwrap() else {
        panic!("unexpected macro request")
    };
    compilation
}

#[test]
fn keyword_named_resource_keeps_its_public_identity() {
    let compilation = compile(
        "DefResource map { New: Fn {start: u32} cell-new(start) func: Fn {self: map} The u32 cell-get(self) }\nLet {item: map(42)} map/func(item)",
        Mode::Snippet,
    );
    let mut host = HostComponent::from_bytes(&compilation.entry_artifact().unwrap().bytes).unwrap();
    assert_eq!(
        host.call_instance("wavelet:snippet/api@0.1.0", "evaluate", &[])
            .unwrap(),
        vec![Val::String("42".into())]
    );
}

#[test]
fn package_interface_function_and_parameter_keywords_keep_their_names() {
    let compilation = compile(
        "Package \"package:world@1.2.3\"\nExport {iface: \"interface\" name: func params: {use: s64} result: s64}\nDef func Fn {use: s64} add(use 1)",
        Mode::Project,
    );
    let mut host = HostComponent::from_bytes(&compilation.entry_artifact().unwrap().bytes).unwrap();
    assert_eq!(
        host.call_instance("package:world/interface@1.2.3", "func", &[Val::S64(41)])
            .unwrap(),
        vec![Val::S64(42)]
    );
}

#[test]
fn named_types_fields_and_cases_escape_keywords() {
    let source = "Package \"demo:keywords@1.0.0\"\nDefType world {import: s64 export: string}\nDefType flags [use with]\nDefType variant [map(world) include]\nDefType type {async static}\nExport {name: func params: {value: world} result: world}\nDef func Fn {value: world} value";
    let compilation = compile(source, Mode::Project);
    let mut host = HostComponent::from_bytes(&compilation.entry_artifact().unwrap().bytes).unwrap();
    let record = Val::Record(vec![
        ("import".into(), Val::S64(42)),
        ("export".into(), Val::String("ok".into())),
    ]);
    assert_eq!(
        host.call_instance(
            "demo:keywords/api@1.0.0",
            "func",
            std::slice::from_ref(&record)
        )
        .unwrap(),
        vec![record]
    );
}

#[test]
fn imported_keyword_types_are_escaped_in_use_aliases() {
    let request = Request {
        sources: vec![
            compiler::SourceFile { name: "library.wlt".into(), contents: "Package \"package:world@1.2.3\"\nDefType map {use: s64}\nExport func\nDef func Fn {value: map} value".into() },
            compiler::SourceFile { name: "app.wlt".into(), contents: "Package \"demo:app@1.0.0\"\nImport {pkg: \"package:world/api@1.2.3\" as: library}\nExport {name: run result: map}\nDef run Fn {} library/func({use: 42})".into() },
        ],
        dependencies: vec![],
        entry: Some("app.wlt".into()),
        mode: Mode::Project,
    };
    let Step::Complete(compilation) = compiler::compile(&request, &mut NoMacros).unwrap() else {
        panic!("unexpected macro request")
    };
    let mut host = HostComponent::from_bytes(&compilation.entry_artifact().unwrap().bytes).unwrap();
    assert_eq!(
        host.call_instance("demo:app/api@1.0.0", "run", &[])
            .unwrap(),
        vec![Val::Record(vec![("use".into(), Val::S64(42))])]
    );
}
