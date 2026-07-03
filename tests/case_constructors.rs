//! 4.1 — variant/enum case constructors are bound values.
//!
//! A `DefType name [case case(t) …]` declaration binds each case in the value
//! namespace: nullary cases as payload-less variant values (like `none`),
//! payloaded cases as constructor functions. Dep-declared cases are reachable
//! through the import alias (`alias/case`). The interpreter is the semantics
//! oracle; the wasm-backend agreement is exercised by the sibling emit tests.

use wavelet::eval_snippet;

fn ok_value(src: &str) -> String {
    let out = eval_snippet(src);
    assert!(out.ok, "expected success for {src:?}, got: {}", out.error);
    out.value
}

fn err_of(src: &str) -> String {
    let out = eval_snippet(src);
    assert!(!out.ok, "expected an error for {src:?}, got: {}", out.value);
    out.error
}

#[test]
fn nullary_case_is_a_value() {
    assert_eq!(
        ok_value("DefType direction [north east south west]\nnorth"),
        "north"
    );
}

#[test]
fn payloaded_case_constructs() {
    assert_eq!(
        ok_value("DefType ttl [days(u32) forever]\ndays(30)"),
        "days(30)"
    );
}

#[test]
fn multi_payload_case_bundles_a_tuple() {
    assert_eq!(
        ok_value("DefType shape [rect(u32 u32) dot]\nrect(3 4)"),
        "rect((3, 4))"
    );
}

#[test]
fn constructed_values_match_and_destructure() {
    assert_eq!(
        ok_value(
            "DefType ttl [days(u32) forever]\n\
             Match days(30) [((days n) to-string(n)) (forever \"forever\")]"
        ),
        "\"30\""
    );
    assert_eq!(
        ok_value(
            "DefType ttl [days(u32) forever]\n\
             Match forever [((days n) to-string(n)) (forever \"forever\")]"
        ),
        "\"forever\""
    );
}

#[test]
fn nullary_cases_compare_by_equality() {
    assert_eq!(ok_value("DefType d [a b]\neq(a b)"), "false");
    assert_eq!(ok_value("DefType d [a b]\neq(a a)"), "true");
}

#[test]
fn constructor_is_first_class() {
    assert_eq!(
        ok_value("DefType ttl [days(u32) forever]\nmap(days [1 2])"),
        "[days(1), days(2)]"
    );
}

#[test]
fn constructor_arity_is_checked() {
    let e = err_of("DefType ttl [days(u32) forever]\ndays()");
    assert!(e.contains("takes 1 argument"), "{e}");
    let e = err_of("DefType ttl [days(u32) forever]\ndays(1 2)");
    assert!(e.contains("takes 1 argument"), "{e}");
}

#[test]
fn constructor_payload_is_type_checked() {
    let e = err_of("DefType ttl [days(u32) forever]\ndays(\"x\")");
    assert!(e.contains("type"), "{e}");
}

/// Dep-defined cases are reachable through the import alias: build a two-file
/// module set on disk and run it through the interpreter's compose stand-in.
#[test]
fn dep_cases_bind_under_the_import_alias() {
    let dir = std::env::temp_dir().join(format!("wvl-dep-cases-{}", std::process::id()));
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let dep = src.join("shapes.wlt");
    std::fs::write(
        &dep,
        "Package \"demo:shapes@0.1.0\"\n\
         DefType shape [circle(f64) dot]\n\
         Export area\n\
         Def area Fn {s: shape}\n\
           Match s [((circle r) mul(r r)) (dot 0.0)]\n",
    )
    .unwrap();
    let main = src.join("main.wlt");
    std::fs::write(
        &main,
        "Package \"demo:app@0.1.0\"\n\
         Import {pkg: \"demo:shapes/api\" as: sh}\n\
         Def run Fn {}\n\
           Do [to-string(sh/area(sh/circle(2.0)))\n\
               to-string(sh/area(sh/dot))]\n",
    )
    .unwrap();
    let paths = vec![
        main.to_string_lossy().to_string(),
        dep.to_string_lossy().to_string(),
    ];
    wavelet::runner::run_files(&paths).expect("dep cases resolve through the alias");
    std::fs::remove_dir_all(&dir).ok();
}
