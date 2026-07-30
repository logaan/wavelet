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
// A WIT variant case carries at most ONE payload type: a multi-payload
// declaration used to be accepted (bundling as a tuple) but synthesized
// invalid WIT (`rect(u32, u32)`), rejected by wasm-tools. It is now a
// deliberate check-time error on every path.
fn multi_payload_case_declaration_is_rejected() {
    let e = err_of("DefType shape [rect(u32 u32) dot]\nrect(3 4)");
    assert!(
        e.contains("a variant case takes at most one payload type"),
        "{e}"
    );
    assert!(e.contains("wrap several in tuple(...)"), "{e}");
    // Rejected even when no case is ever constructed: the declaration alone
    // is the error.
    let e = err_of("DefType shape [rect(u32 u32) dot]\ndot");
    assert!(
        e.contains("a variant case takes at most one payload type"),
        "{e}"
    );
}

#[test]
// The supported spelling: ONE payload type that is a tuple.
fn tuple_wrapped_payload_constructs() {
    assert_eq!(
        ok_value("DefType shape [rect(tuple(u32 u32)) dot]\nrect(tuple2(3 4))"),
        "rect((3, 4))"
    );
    assert_eq!(
        ok_value(
            "DefType shape [rect(tuple(u32 u32)) dot]\n\
             Match rect(tuple2(3 4)) [((rect (w h)) mul(w h)) (dot 0)]"
        ),
        "12"
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

// ---------------------------------------------------------------------------
// Backend agreement: the same constructions compile and cross the boundary.
// ---------------------------------------------------------------------------

use wavelet::host::{HostComponent, Val};

fn cases_component() -> HostComponent {
    let dir = std::env::temp_dir().join(format!("wvl-cases-emit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let app = r#"Package "demo:cases@0.1.0"

DefType direction [north east south west]
DefType ttl [days(u32) forever]

// Rotate a direction: nullary cases constructed bare, like the interpreter.
Export {name: rotate params: {d: direction} result: direction}
Def rotate Fn {d}
  Match d [(north east) (east south) (south west) (west north)]

// Payloaded construction and destructuring.
Export {name: extend params: {t: ttl} result: ttl}
Def extend Fn {t}
  Match t [((days n) days(add(n 1))) (forever forever)]

Export {name: mk-days params: {n: u32} result: ttl}
Def mk-days Fn {n}
  days(n)
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();
    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the cases component");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);
    HostComponent::from_bytes(&bytes).expect("instantiate the cases component")
}

const IFACE: &str = "demo:cases/api@0.1.0";

#[test]
fn backend_constructs_enum_and_variant_cases() {
    let mut c = cases_component();
    let out = c
        .call_instance(IFACE, "rotate", &[Val::Enum("north".into())])
        .expect("rotate(north)");
    assert_eq!(out[0], Val::Enum("east".into()));
    let out = c
        .call_instance(IFACE, "rotate", &[Val::Enum("west".into())])
        .expect("rotate(west)");
    assert_eq!(out[0], Val::Enum("north".into()));

    let out = c
        .call_instance(
            IFACE,
            "extend",
            &[Val::Variant("days".into(), Some(Box::new(Val::U32(30))))],
        )
        .expect("extend(days(30))");
    assert_eq!(
        out[0],
        Val::Variant("days".into(), Some(Box::new(Val::U32(31))))
    );
    let out = c
        .call_instance(IFACE, "extend", &[Val::Variant("forever".into(), None)])
        .expect("extend(forever)");
    assert_eq!(out[0], Val::Variant("forever".into(), None));

    let out = c
        .call_instance(IFACE, "mk-days", &[Val::U32(7)])
        .expect("mk-days(7)");
    assert_eq!(
        out[0],
        Val::Variant("days".into(), Some(Box::new(Val::U32(7))))
    );
}

// ---------------------------------------------------------------------------
// The one-payload rule end to end: a `tuple(...)`-wrapped payload (the
// supported spelling for "several payload types") synthesizes VALID WIT —
// `zipped(tuple<list<u32>, list<u32>>)` — so the build componentizes, and the
// value round-trips across the boundary intact.
// ---------------------------------------------------------------------------

fn zip_component() -> HostComponent {
    let dir = std::env::temp_dir().join(format!("wvl-zip-emit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let app = r#"Package "demo:zipper@0.1.0"

DefType mylist [zipped(tuple(list(u32) list(u32))) other]

Export {name: mk params: {a: list(u32) b: list(u32)} result: mylist}
Def mk Fn {a b}
  zipped(tuple2(a b))

Export {name: rt params: {v: mylist} result: mylist}
Def rt Fn {v}
  v
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();
    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the tuple-payload component");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);
    HostComponent::from_bytes(&bytes).expect("instantiate the tuple-payload component")
}

#[test]
fn tuple_wrapped_payload_builds_and_round_trips() {
    let mut c = zip_component();
    let pair = Val::Tuple(vec![
        Val::List(vec![Val::U32(1), Val::U32(2)]),
        Val::List(vec![Val::U32(9)]),
    ]);
    let zipped = Val::Variant("zipped".into(), Some(Box::new(pair)));

    let out = c
        .call_instance("demo:zipper/api@0.1.0", "mk", &[
            Val::List(vec![Val::U32(1), Val::U32(2)]),
            Val::List(vec![Val::U32(9)]),
        ])
        .expect("mk([1 2] [9])");
    assert_eq!(out[0], zipped);

    let out = c
        .call_instance("demo:zipper/api@0.1.0", "rt", &[zipped.clone()])
        .expect("rt(zipped(...))");
    assert_eq!(out[0], zipped);

    let out = c
        .call_instance(
            "demo:zipper/api@0.1.0",
            "rt",
            &[Val::Variant("other".into(), None)],
        )
        .expect("rt(other)");
    assert_eq!(out[0], Val::Variant("other".into(), None));
}
