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
