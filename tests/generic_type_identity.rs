use std::path::Path;

use wavelet::host::{HostComponent, Val};
use wavelet::staging::Project;

#[test]
fn imported_records_with_the_same_name_keep_distinct_layouts() {
    let mut project = Project::new().unwrap();
    project
        .write(
            Path::new("numbers.wlt"),
            r#"
Package "custom:numbers@1.0.0"
DefType payload {value: u32}
Export {name: make result: payload}
Def make Fn {} {value: 42}
Export {name: inspect params: {p: payload} result: bool}
Def inspect Fn {p: payload} Match p [({value: n} eq(n 42))]
"#,
        )
        .unwrap();
    project
        .write(
            Path::new("words.wlt"),
            r#"
Package "custom:words@1.0.0"
DefType payload {value: string}
Export {name: make result: payload}
Def make Fn {} {value: "forty-two"}
Export {name: inspect params: {p: payload} result: bool}
Def inspect Fn {p: payload} Match p [({value: s} eq(s "forty-two"))]
"#,
        )
        .unwrap();
    project
        .write(
            Path::new("consumer.wlt"),
            r#"
Package "custom:consumer@1.0.0"
Import {pkg: "custom:numbers/api" as: numbers}
Import {pkg: "custom:words/api" as: words}
Export {name: check result: bool}
Def check Fn {} If numbers/inspect(numbers/make()) words/inspect(words/make()) false
"#,
        )
        .unwrap();
    project.build().unwrap();
    let mut component = HostComponent::from_file(&project.output().join("app.wasm")).unwrap();
    let result = component
        .call_instance("custom:consumer/api@1.0.0", "check", &[])
        .unwrap();
    assert!(matches!(result.as_slice(), [Val::Bool(true)]), "{result:?}");
}

fn compile_external(directory: &std::path::Path, name: &str, source: &str) -> std::path::PathBuf {
    let dep = wavelet::witdep::resolve_dep(directory, "custom:duo")
        .unwrap()
        .unwrap();
    let (arena, roots) = wavelet::read_file(source).unwrap();
    let (arena, roots) = wavelet::expand::expand_file(arena, &roots, None).unwrap();
    let info = wavelet::wit::collect(&arena, &roots).unwrap();
    let bytes = wavelet::emit::emit_component(
        &arena,
        &roots,
        &info,
        &std::collections::HashMap::from([("custom:duo".to_owned(), dep)]),
    )
    .unwrap();
    let path = directory.parent().unwrap().join(format!("{name}.wasm"));
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn same_named_resources_and_variants_in_two_interfaces_remain_distinct() {
    let mut project = Project::new().unwrap();
    let deps = project.path().join("wit/deps");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(
        deps.join("duo.wit"),
        r#"
package custom:duo@1.0.0;
interface numbers {
    variant error-code { value(u32), absent }
    resource device { constructor(value: u32); read: func() -> error-code; }
}
interface words {
    variant error-code { value(string), absent }
    resource device { constructor(value: string); read: func() -> error-code; }
}
"#,
    )
    .unwrap();
    let numbers = compile_external(
        &deps,
        "numbers",
        r#"
Package "custom:number-implementation@1.0.0"
DefType error-code [value(u32) absent]
DefResource device {
    New: Fn {start: u32} start
    read: Fn {self: device} The error-code value(self)
}
Export {iface: "custom:duo/numbers@1.0.0" name: device}
"#,
    );
    let words = compile_external(
        &deps,
        "words",
        r#"
Package "custom:word-implementation@1.0.0"
DefType error-code [value(string) absent]
DefResource device {
    New: Fn {start: string} start
    read: Fn {self: device} The error-code value(self)
}
Export {iface: "custom:duo/words@1.0.0" name: device}
"#,
    );
    project
        .write(
            Path::new("consumer.wlt"),
            r#"
Package "custom:consumer@1.0.0"
Import {pkg: "custom:duo/numbers@1.0.0" as: numbers}
Import {pkg: "custom:duo/words@1.0.0" as: words}
Def read-number Fn {item: borrow(numbers/device)} numbers/device-read(item)
Def read-word Fn {item: borrow(words/device)} words/device-read(item)
Export {name: check result: bool}
Def check Fn {}
  Let {number: numbers/device(42) word: words/device("forty-two")}
    Let {
      number-ok: Match read-number(number) [(value(n) eq(n 42)) (absent false)]
      word-ok: Match read-word(word) [(value(s) eq(s "forty-two")) (absent false)]
    }
      Do [numbers/drop-device(number) words/drop-device(word) If number-ok word-ok false]
"#,
        )
        .unwrap();
    let outputs = project.build().unwrap();
    let composed = project.output().join("composed.wasm");
    wavelet::tools::wac_plug(Path::new(&outputs[0]), &[&numbers, &words], &composed).unwrap();
    let mut component = HostComponent::from_file(&composed).unwrap();
    let result = component
        .call_instance("custom:consumer/api@1.0.0", "check", &[])
        .unwrap();
    assert!(matches!(result.as_slice(), [Val::Bool(true)]), "{result:?}");
}

#[test]
fn distinct_resource_identities_cannot_be_interchanged() {
    let mut project = Project::new().unwrap();
    let deps = project.path().join("wit/deps");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(
        deps.join("duo.wit"),
        r#"
package custom:duo@1.0.0;
interface numbers { resource device { constructor(); read: func() -> u32; } }
interface words { resource device { constructor(); read: func() -> u32; } }
"#,
    )
    .unwrap();
    project
        .write(
            Path::new("consumer.wlt"),
            r#"
Package "custom:consumer@1.0.0"
Import {pkg: "custom:duo/numbers@1.0.0" as: numbers}
Import {pkg: "custom:duo/words@1.0.0" as: words}
Export {name: wrong result: u32}
Def wrong Fn {} numbers/device-read(words/device())
"#,
        )
        .unwrap();
    let error = project.build().unwrap_err().to_string();
    assert!(error.contains("has the wrong type"), "{error}");
}

#[test]
fn shared_record_aliases_support_destructuring_across_interfaces() {
    let mut project = Project::new().unwrap();
    let deps = project.path().join("wit/deps");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(
        deps.join("duo.wit"),
        r#"
package custom:duo@1.0.0;
interface types { record point { x: s32, y: s32 } }
interface values { use types.{point}; inspect: func(value: point) -> s32; }
interface resources { use types.{point}; }
"#,
    )
    .unwrap();
    project
        .write(
            Path::new("consumer.wlt"),
            r#"
Package "custom:consumer@1.0.0"
Import {pkg: "custom:duo/types@1.0.0" as: types}
Import {pkg: "custom:duo/values@1.0.0" as: values}
Import {pkg: "custom:duo/resources@1.0.0" as: resources}
Export {name: total params: {p: point} result: s32}
Def total Fn {p: point} Match p [({x: a y: b} add(a b))]
"#,
        )
        .unwrap();
    project.build().unwrap();
}
