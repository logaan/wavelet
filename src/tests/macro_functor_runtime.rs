use crate::builtins;
use crate::interp::Interp;
use crate::value::{Value, unit};

fn op(interp: &Interp, name: &str, args: Vec<Value>) -> Value {
    let arg = match args.len() {
        0 => unit(),
        1 => args.into_iter().next().unwrap(),
        _ => Value::Tup(args),
    };
    builtins::call(interp, name, arg, None).unwrap_or_else(|e| panic!("`{name}` failed: {e}"))
}

fn point(x: i64, y: i64) -> Value {
    Value::Rec(vec![
        ("x".to_string(), Value::Int(x)),
        ("y".to_string(), Value::Int(y)),
    ])
}

#[test]

fn new_set_is_empty() {
    let interp = Interp::new();
    let s = op(&interp, "set-new", vec![]);
    assert_eq!(op(&interp, "set-size", vec![s.clone()]), Value::Int(0));
    assert_eq!(
        op(&interp, "set-contains", vec![s, Value::Int(1)]),
        Value::Bool(false)
    );
}

#[test]

fn add_is_observed_on_the_same_handle_and_dedups() {
    let interp = Interp::new();
    let s = op(&interp, "set-new", vec![]);

    op(&interp, "set-add", vec![s.clone(), Value::Int(1)]);
    op(&interp, "set-add", vec![s.clone(), Value::Int(2)]);
    op(&interp, "set-add", vec![s.clone(), Value::Int(1)]);

    assert_eq!(op(&interp, "set-size", vec![s.clone()]), Value::Int(2));
    assert_eq!(
        op(&interp, "set-contains", vec![s.clone(), Value::Int(1)]),
        Value::Bool(true)
    );
    assert_eq!(
        op(&interp, "set-contains", vec![s.clone(), Value::Int(2)]),
        Value::Bool(true)
    );
    assert_eq!(
        op(&interp, "set-contains", vec![s, Value::Int(3)]),
        Value::Bool(false)
    );
}

#[test]

fn add_returns_unit() {
    let interp = Interp::new();
    let s = op(&interp, "set-new", vec![]);
    assert_eq!(op(&interp, "set-add", vec![s, Value::Int(7)]), unit());
}

#[test]

fn element_equality_matches_eq_for_records() {
    let interp = Interp::new();
    let s = op(&interp, "set-new", vec![]);

    op(&interp, "set-add", vec![s.clone(), point(1, 2)]);
    op(&interp, "set-add", vec![s.clone(), point(3, 4)]);
    op(&interp, "set-add", vec![s.clone(), point(1, 2)]);

    assert_eq!(op(&interp, "set-size", vec![s.clone()]), Value::Int(2));
    assert_eq!(
        op(&interp, "set-contains", vec![s.clone(), point(3, 4)]),
        Value::Bool(true)
    );
    assert_eq!(
        op(&interp, "set-contains", vec![s, point(9, 9)]),
        Value::Bool(false)
    );

    let eq =
        |a: Value, b: Value| builtins::call(&interp, "eq", Value::Tup(vec![a, b]), None).unwrap();
    assert_eq!(eq(point(1, 2), point(1, 2)), Value::Bool(true));
    assert_eq!(eq(point(1, 2), point(3, 4)), Value::Bool(false));
}

#[test]

fn distinct_handles_are_independent() {
    let interp = Interp::new();
    let a = op(&interp, "set-new", vec![]);
    let b = op(&interp, "set-new", vec![]);
    op(&interp, "set-add", vec![a.clone(), Value::Int(1)]);

    assert_eq!(op(&interp, "set-size", vec![a]), Value::Int(1));
    assert_eq!(op(&interp, "set-size", vec![b]), Value::Int(0));
}

fn run_source(src: &str) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!(
        "wavelet-functor-{}-{}",
        std::process::id(),
        RUN_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("src.wlt");
    std::fs::write(&path, src).unwrap();
    let res = crate::runner::run_files(&[path.to_string_lossy().into_owned()]);
    let _ = std::fs::remove_dir_all(&dir);
    res
}

static RUN_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

const WORKED_EXAMPLE: &str = r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Def eq-point Fn {a: point b: point} eq(a b)
Export eq-point
Instantiate {pkg: "wavelet:coll/set" with: {elem: point} as: pts}
Def assert Fn {cond: bool} If cond true eq(div(1 0) 0)
Export {name: run result: bool}
Def run Fn {}
  Let {s: pts/new()}
    Do [ pts/add(s {x: 1 y: 2})
         pts/add(s {x: 3 y: 4})
         pts/add(s {x: 1 y: 2})
         assert(eq(pts/size(s) 2))
         assert(pts/contains(s {x: 3 y: 4}))
         assert(not(pts/contains(s {x: 9 y: 9}))) ]"#;

#[test]

fn worked_example_runs_and_self_checks_pass() {
    run_source(WORKED_EXAMPLE).expect("functor worked example should run cleanly");
}

#[test]

fn a_wrong_assertion_is_caught() {
    let bad = WORKED_EXAMPLE.replace("eq(pts/size(s) 2)", "eq(pts/size(s) 99)");
    assert!(
        run_source(&bad).is_err(),
        "a wrong size assertion must fail the run"
    );
}

#[test]

fn each_is_not_yet_defined() {
    let src = r#"Export {name: run result: bool}
Def run Fn {} each([1 2 3] Fn {x} x)"#;
    let err = run_source(src).expect_err("`each` is not defined yet");
    assert!(err.contains("each"), "unexpected error: {err}");
}

fn build_source(src_text: &str) -> Result<Vec<u8>, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "wavelet-functor-build-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let path = src.join("app.wlt");
    std::fs::write(&path, src_text).unwrap();

    let out = dir.join("out");
    let res = crate::build::build_files(
        &[path.to_string_lossy().into_owned()],
        &out.to_string_lossy(),
    )
    .map(|outputs| std::fs::read(&outputs[0]).expect("read built component"));
    let _ = std::fs::remove_dir_all(&dir);
    res
}

#[test]

fn build_emits_a_validating_set_resource() {
    const SRC: &str = r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Def eq-point Fn {a: point b: point} eq(a b)
Export eq-point
Instantiate {pkg: "wavelet:coll/set" with: {elem: point} as: pts}
Export count-distinct
Def count-distinct Fn {ps: list(point)}
  Let {s: pts/new()}
    Do [ pts/add(s {x: 1 y: 2})
         pts/size(s) ]"#;

    let bytes = build_source(SRC).expect("a functor program now builds");
    let wit = wasm_tools_component_wit(&bytes);

    assert!(
        wit.contains("interface point-set"),
        "WIT missing point-set: {wit}"
    );
    assert!(
        wit.contains("resource set"),
        "WIT missing the set resource: {wit}"
    );
    for op in ["constructor()", "add:", "contains:", "size:"] {
        assert!(
            wit.contains(op),
            "WIT missing `{op}` on the set resource: {wit}"
        );
    }
}

#[test]

fn build_hoists_handle_returning_export_over_local_record() {
    const SRC: &str = r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Def eq-point Fn {a: point b: point} eq(a b)
Export eq-point
Instantiate {pkg: "wavelet:coll/set" with: {elem: point} as: pts}
Export nearest-set
Def nearest-set Fn {ps: list(point)}
  Let {s: pts/new()}
    Do [ pts/add(s {x: 1 y: 2})
         s ]"#;

    let bytes =
        build_source(SRC).expect("a handle-returning export over a local record now builds (4.7)");
    let wit = wasm_tools_component_wit(&bytes);
    assert!(
        wit.contains("interface types"),
        "element record should be hoisted into a shared types interface:\n{wit}"
    );
    assert!(
        wit.contains("use types.{point}"),
        "both api and point-set should use the hoisted record:\n{wit}"
    );
}

fn wasm_tools_component_wit(bytes: &[u8]) -> String {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!(
        "wavelet-wit-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("c.wasm");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
    let out = std::process::Command::new("wasm-tools")
        .args(["component", "wit"])
        .arg(&path)
        .output()
        .expect("run wasm-tools component wit");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        out.status.success(),
        "wasm-tools component wit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
