//! Runtime (interpreter-oracle) tests for the `Set` functor.
//!
//! The interpreter (`interp.rs`/`builtins.rs`) is the language's semantics
//! oracle; the wasm backend is validated against it. These tests pin the
//! `wavelet:coll/set` functor's runtime behaviour at two levels:
//!
//!   * the `set-*` builtins directly (the exact dispatch a functor `Import`'s
//!     `alias/op` bindings reach), and
//!   * the whole `wavelet run` path (`runner::run_files`), so a functor program
//!     — `Derive`, the parameterized `Import`, and qualified ops together —
//!     evaluates end to end.
//!
//! The op semantics mirror the `SET_OPS` WIT descriptor in `wit.rs`:
//! `new() -> set`, `add(set, elem)`, `contains(set, elem) -> bool`,
//! `size(set) -> u32` (an integer here).

use wavelet::builtins;
use wavelet::interp::Interp;
use wavelet::value::{unit, Value};

/// Call a `set-*` builtin with positional args, as the interpreter does after
/// bundling a call's arguments.
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
// A fresh set is empty: `new()` then `size` is 0, and `contains` is false.
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
// `add` mutates the handle in place (shared identity): a `size`/`contains` on
// the same handle observes earlier `add`s, and adding a duplicate is a no-op.
fn add_is_observed_on_the_same_handle_and_dedups() {
    let interp = Interp::new();
    let s = op(&interp, "set-new", vec![]);

    op(&interp, "set-add", vec![s.clone(), Value::Int(1)]);
    op(&interp, "set-add", vec![s.clone(), Value::Int(2)]);
    op(&interp, "set-add", vec![s.clone(), Value::Int(1)]); // duplicate

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
// `add` returns unit (a discarding op), exactly like the WIT `add: func(value)`.
fn add_returns_unit() {
    let interp = Interp::new();
    let s = op(&interp, "set-new", vec![]);
    assert_eq!(op(&interp, "set-add", vec![s, Value::Int(7)]), unit());
}

#[test]
// Element membership uses `Value` equality — the same equality the `eq` builtin
// computes — so structurally-equal records are the same element (the functor is
// instantiated at a derived record type in the worked example).
fn element_equality_matches_eq_for_records() {
    let interp = Interp::new();
    let s = op(&interp, "set-new", vec![]);

    op(&interp, "set-add", vec![s.clone(), point(1, 2)]);
    op(&interp, "set-add", vec![s.clone(), point(3, 4)]);
    op(&interp, "set-add", vec![s.clone(), point(1, 2)]); // structural duplicate

    assert_eq!(op(&interp, "set-size", vec![s.clone()]), Value::Int(2));
    assert_eq!(
        op(&interp, "set-contains", vec![s.clone(), point(3, 4)]),
        Value::Bool(true)
    );
    assert_eq!(
        op(&interp, "set-contains", vec![s, point(9, 9)]),
        Value::Bool(false)
    );

    // Cross-check: the set's "same element" agrees with the `eq` builtin.
    let eq = |a: Value, b: Value| {
        builtins::call(&interp, "eq", Value::Tup(vec![a, b]), None).unwrap()
    };
    assert_eq!(eq(point(1, 2), point(1, 2)), Value::Bool(true));
    assert_eq!(eq(point(1, 2), point(3, 4)), Value::Bool(false));
}

#[test]
// Two independent `new()` handles do not share state.
fn distinct_handles_are_independent() {
    let interp = Interp::new();
    let a = op(&interp, "set-new", vec![]);
    let b = op(&interp, "set-new", vec![]);
    op(&interp, "set-add", vec![a.clone(), Value::Int(1)]);

    assert_eq!(op(&interp, "set-size", vec![a]), Value::Int(1));
    assert_eq!(op(&interp, "set-size", vec![b]), Value::Int(0));
}

// ---- end-to-end through `wavelet run` (`runner::run_files`) ----------------

/// Run one `.wvl` source through the real `wavelet run` path and return the
/// outcome. The program is written to a temp file because `run_files` reads
/// files (it is the interpreter stand-in for `wavelet compose`).
fn run_source(src: &str) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!(
        "wavelet-functor-{}-{}",
        std::process::id(),
        // a per-call salt so parallel tests don't collide on the path
        RUN_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("src.wvl");
    std::fs::write(&path, src).unwrap();
    let res = wavelet::runner::run_files(&[path.to_string_lossy().into_owned()]);
    let _ = std::fs::remove_dir_all(&dir);
    res
}

static RUN_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The functor worked example, made self-checking: `run` builds a set of
/// `point`s (with a duplicate), then *asserts* the expected `size`/`contains`
/// results. A failed assertion calls `head([])`, which the interpreter rejects,
/// so a wrong answer turns this from `Ok` into `Err` — the program is its own
/// oracle. `map` stands in for the (not-yet-implemented) `each`.
const WORKED_EXAMPLE: &str = r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Derive {Eq Ord Show} point
Import {pkg: "wavelet:coll/set" elem: point as: pts}
Def assert Fn {cond} If cond {} head([])
Def run Fn {}
  Let {s: pts/new()}
    Do [ pts/add(s {x: 1 y: 2})
         pts/add(s {x: 3 y: 4})
         pts/add(s {x: 1 y: 2})
         assert(eq(pts/size(s) 2))
         assert(pts/contains(s {x: 3 y: 4}))
         assert(not(pts/contains(s {x: 9 y: 9}))) ]"#;

#[test]
// The whole functor program — `Derive`, the parameterized `Import`, and the
// qualified `pts/...` ops — evaluates end to end under `wavelet run`, and its
// self-checks all hold.
fn worked_example_runs_and_self_checks_pass() {
    run_source(WORKED_EXAMPLE).expect("functor worked example should run cleanly");
}

#[test]
// Sanity check that the self-checking harness can fail: a wrong expected size
// trips the `assert`, so a divergence would not silently pass.
fn a_wrong_assertion_is_caught() {
    let bad = WORKED_EXAMPLE.replace("eq(pts/size(s) 2)", "eq(pts/size(s) 99)");
    assert!(
        run_source(&bad).is_err(),
        "a wrong size assertion must fail the run"
    );
}

#[test]
// `each` is referenced by the documented worked example but is not yet a real
// builtin/macro; this test documents that gap so it is not mistaken for a
// regression. (The runnable form above uses `map`/explicit `add`s instead.)
fn each_is_not_yet_defined() {
    let src = r#"Def run Fn {} each([1 2 3] Fn {x} x)"#;
    let err = run_source(src).expect_err("`each` is not defined yet");
    assert!(err.contains("each"), "unexpected error: {err}");
}

/// Build one `.wvl` source through `wavelet build` in a throwaway dir, returning
/// the build result and (on success) the bytes of the single output component.
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
    let path = src.join("app.wvl");
    std::fs::write(&path, src_text).unwrap();

    let out = dir.join("out");
    let res = wavelet::build::build_files(
        &[path.to_string_lossy().into_owned()],
        &out.to_string_lossy(),
    )
    .map(|outputs| std::fs::read(&outputs[0]).expect("read built component"));
    let _ = std::fs::remove_dir_all(&dir);
    res
}

#[test]
// `wavelet build` now emits the functor `set` resource. A program that
// instantiates the functor and *derives an ordinary result* from it (here a
// `u32` from `size`) builds and produces a component that `wasm-tools` validates,
// whose WIT exports the specialized `point-set` interface with the `set`
// resource and its four ops. The qualified `pts/...` calls are not yet *routed*
// (step 04) — their emitted bodies trap if reached — but the component is
// structurally complete and valid. (Runtime call-correctness is covered by the
// interpreter-oracle tests above and lands in the backend in the routing step.)
fn build_emits_a_validating_set_resource() {
    const SRC: &str = r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Derive {Eq Ord Show} point
Import {pkg: "wavelet:coll/set" elem: point as: pts}
Export count-distinct
Def count-distinct Fn {ps: list(point)}
  Let {s: pts/new()}
    Do [ pts/add(s {x: 1 y: 2})
         pts/size(s) ]"#;

    let bytes = build_source(SRC).expect("a functor program now builds");
    let wit = wasm_tools_component_wit(&bytes);
    // The specialized interface and its `set` resource with all four ops.
    assert!(wit.contains("interface point-set"), "WIT missing point-set: {wit}");
    assert!(wit.contains("resource set"), "WIT missing the set resource: {wit}");
    for op in ["constructor()", "add:", "contains:", "size:"] {
        assert!(wit.contains(op), "WIT missing `{op}` on the set resource: {wit}");
    }
}

#[test]
// An export that *returns* a `set` handle whose element is a *local record*
// (the docs `nearest-set: func(..) -> point-set.set` shape) makes `api` and the
// `point-set` interface mutually depend in WIT — `api` `use`s the handle while
// `point-set` `use`s the record — a cycle the component model cannot express.
// The backend rejects it with an honest, specific error rather than emitting WIT
// that fails to parse. (Lifting this is follow-up work; an export deriving an
// ordinary result from the set, as above, has no cycle and builds.)
fn build_rejects_handle_returning_export_over_local_record() {
    const SRC: &str = r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Derive {Eq Ord Show} point
Import {pkg: "wavelet:coll/set" elem: point as: pts}
Export nearest-set
Def nearest-set Fn {ps: list(point)}
  Let {s: pts/new()}
    Do [ pts/add(s {x: 1 y: 2})
         s ]"#;

    let err = build_source(SRC)
        .expect_err("a handle-returning export over a local record must not silently succeed");
    assert!(
        err.contains("cycle") && err.contains("nearest-set"),
        "build error should explain the WIT interface cycle, got: {err}"
    );
}

/// Run `wasm-tools component wit` on built component bytes, returning the WIT
/// text. Skips with a panic only if `wasm-tools` is absent (it is a dev/test
/// dependency the build path already relies on transitively).
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
    std::fs::File::create(&path).unwrap().write_all(bytes).unwrap();
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
