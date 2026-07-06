//! Goal 5 (result-type threading + Option-A int widening): an exported def's
//! declared result type is threaded into its body as the `expected` type, so
//! construction sites build at their declared shape and the boundary's
//! legitimate coercions are admitted while unsound ones are rejected.
//!
//! Policy (Option A): a narrower integer may widen into a wider declared result
//! of the SAME signedness; mixed signedness and wider->narrower are rejected. A
//! value carrying a result payload may flow into a declared payload-less
//! `result` (the ABI drops the payload — the `wasi:cli/run` shape).

use wavelet::{check, expand, read_file};

/// read -> expand -> full program check, the compile-path checker `wavelet build`
/// runs. `Ok(())` iff the program type-checks (threading included).
fn check_file(src: &str) -> Result<(), String> {
    let (arena, roots) = read_file(src).map_err(|e| e.to_string())?;
    let (arena, roots) = expand::expand_file(arena, &roots, None)?;
    check::check_program(&arena, &roots)
}

fn prog(result: &str, body: &str) -> String {
    format!(
        "Package \"test:t@0.1.0\"\n\
         Export {{name: f params: {{}} result: {result}}}\n\
         Def f Fn {{}} {body}\n"
    )
}

// --- Option-A widening: same-signedness narrower -> wider is accepted --------

#[test]
fn signed_widening_accepted() {
    // An s32 body flows into an s64-declared result. Threading makes the body
    // see s64; widening admits it. (Without widening this is a type error.)
    assert!(check_file(&prog("s64", "The s32 1")).is_ok());
}

#[test]
fn unsigned_widening_accepted() {
    assert!(check_file(&prog("u64", "The u16 1")).is_ok());
}

#[test]
fn same_width_accepted() {
    assert!(check_file(&prog("s32", "The s32 1")).is_ok());
}

// --- rejected: narrowing, and mixed signedness ------------------------------

#[test]
fn narrowing_rejected() {
    // s32 body into an s16 result narrows — rejected even though the value fits.
    let err = check_file(&prog("s16", "The s32 100")).unwrap_err();
    assert!(err.contains("type mismatch"), "got: {err}");
}

#[test]
fn signed_to_unsigned_rejected() {
    let err = check_file(&prog("u32", "The s32 5")).unwrap_err();
    assert!(err.contains("type mismatch"), "got: {err}");
}

#[test]
fn unsigned_to_signed_rejected() {
    let err = check_file(&prog("s64", "The u32 5")).unwrap_err();
    assert!(err.contains("type mismatch"), "got: {err}");
}

// --- payload-less result-arm dropping (the wasi:cli/run shape) --------------

#[test]
fn ok_payload_into_payloadless_result_accepted() {
    // `run: func() -> result` with an `ok(0)` body: the ABI drops the payload.
    assert!(check_file(&prog("result", "ok(0)")).is_ok());
}

#[test]
fn present_ok_arm_still_checked() {
    // A declared present ok arm still type-checks against the value: an s32
    // payload does not narrow into an s16 ok arm.
    let err = check_file(&prog("result(s16)", "ok(The s32 100)")).unwrap_err();
    assert!(err.contains("type mismatch"), "got: {err}");
}
