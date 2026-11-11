use std::path::Path;

use wavelet::host::{HostComponent, Val};
use wavelet::staging::Project;

#[test]
fn one_import_expands_macros_and_links_runtime_calls() {
    let mut project = Project::new().unwrap();
    let macros = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/produced-macros.wasm");
    project
        .write(
            Path::new("main.wlt"),
            &format!(
                r#"
Package "demo:consumer@0.1.0"
Import {{pkg: "demo:library/api" as: lib roles: {{runtime macro}} from: "{}"}}
Export {{name: answer result: s64}}
Def answer Fn {{}} lib/Identity lib/increment(41)
"#,
                macros.display()
            ),
        )
        .unwrap();
    project
        .write(
            Path::new("library.wlt"),
            r#"
Package "demo:library@0.1.0"
Export increment
Def increment Fn {x: s64} add(x 1)
"#,
        )
        .unwrap();
    project.build().unwrap();
    let mut component = HostComponent::from_file(&project.output().join("app.wasm")).unwrap();
    let result = component
        .call_instance("demo:consumer/api@0.1.0", "answer", &[])
        .unwrap();
    assert!(matches!(result.as_slice(), [Val::S64(42)]));
}

#[test]
fn roles_control_synthesized_runtime_imports() {
    for (roles, runtime) in [
        ("{runtime macro}", true),
        ("{runtime}", true),
        ("{macro}", false),
    ] {
        let source = format!(
            r#"Package "demo:consumer"
Import {{pkg: "demo:library/api" roles: {roles}}}"#
        );
        let (arena, roots) = wavelet::read_file(&source).unwrap();
        let wit = wavelet::wit::synthesize(&arena, &roots).unwrap();
        assert_eq!(wit.contains("import demo:library/api"), runtime);
    }
}

#[test]
fn malformed_or_conflicting_roles_are_rejected() {
    for fields in [
        "roles: {}",
        "roles: {unknown}",
        "roles: true",
        "roles: {macro} macros: true",
    ] {
        let source = format!(
            r#"Package "demo:consumer"
Import {{pkg: "demo:library/api" {fields}}}"#
        );
        let (arena, roots) = wavelet::read_file(&source).unwrap();
        assert!(wavelet::wit::synthesize(&arena, &roots).is_err());
    }
}
