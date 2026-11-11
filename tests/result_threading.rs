use wavelet::{check, expand, read_file};

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

#[test]
fn signed_widening_accepted() {
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

#[test]
fn narrowing_rejected() {
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

#[test]
fn ok_payload_into_payloadless_result_accepted() {
    assert!(check_file(&prog("result", "ok(0)")).is_ok());
}

#[test]
fn present_ok_arm_still_checked() {
    let err = check_file(&prog("result(s16)", "ok(The s32 100)")).unwrap_err();
    assert!(err.contains("type mismatch"), "got: {err}");
}
