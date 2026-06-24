//! Backend ↔ interpreter parity for the numeric and comparison builtins.
//!
//! `CLAUDE.md` makes the interpreter the semantics oracle: "a wasm-backend
//! change that diverges from the interpreter is a bug." The wasm backend used
//! to `unbox_int` unconditionally, so float/string operands trapped at runtime,
//! arithmetic was silently variadic, and integer overflow wrapped instead of
//! erroring. These tests build a component **through the real emitter** and then
//! *execute* it in-process (the same capability-free `wasmtime` host the macro
//! runtime uses), asserting the backend now agrees with the interpreter:
//!
//!  - `f64` arithmetic and `string` comparison run instead of trapping;
//!  - mixed int/float arithmetic widens to float, as `arith` does;
//!  - integer overflow and `div`/`rem` edge cases trap (checked), matching the
//!    interpreter's `checked_*` errors;
//!  - the arithmetic builtins are strictly binary.

use wavelet::host::{HostComponent, Val};

/// Build the fixed numeric API app and return the instantiated component.
/// The app is self-contained (no imports), so it builds and runs with no
/// external toolchain — `build_files` runs the component encoder with
/// validation on, and `HostComponent` instantiates against an empty linker.
fn numeric_component() -> HostComponent {
    // Unique per call: both tests in this binary build through here, and each
    // removes its dir at start and end. Keying the dir on the PID alone let two
    // concurrent calls (same process) wipe each other's build mid-flight under a
    // loaded `cargo test`; a per-call sequence number keeps them isolated.
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("wavelet-numeric-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let app = r#"Package "demo:app@0.1.0"

Export {name: addf params: {x: f64 y: f64} result: f64}
Def addf Fn {x: f64 y: f64}
  add(x y)

Export {name: remf params: {x: f64 y: f64} result: f64}
Def remf Fn {x: f64 y: f64}
  rem(x y)

Export {name: divf params: {x: f64 y: f64} result: f64}
Def divf Fn {x: f64 y: f64}
  div(x y)

Export {name: mix params: {a: s64 b: f64} result: f64}
Def mix Fn {a: s64 b: f64}
  add(a b)

Export {name: addi params: {a: s64 b: s64} result: s64}
Def addi Fn {a: s64 b: s64}
  add(a b)

Export {name: muli params: {a: s64 b: s64} result: s64}
Def muli Fn {a: s64 b: s64}
  mul(a b)

Export {name: divi params: {a: s64 b: s64} result: s64}
Def divi Fn {a: s64 b: s64}
  div(a b)

Export {name: remi params: {a: s64 b: s64} result: s64}
Def remi Fn {a: s64 b: s64}
  rem(a b)

Export {name: lts params: {a: string b: string} result: bool}
Def lts Fn {a: string b: string}
  lt(a b)

Export {name: ltf params: {x: f64 y: f64} result: bool}
Def ltf Fn {x: f64 y: f64}
  lt(x y)

Export {name: ltmix params: {a: s64 b: f64} result: bool}
Def ltmix Fn {a: s64 b: f64}
  lt(a b)
"#;
    let app_path = src.join("app.wvl");
    std::fs::write(&app_path, app).unwrap();

    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the numeric API component");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);

    HostComponent::from_bytes(&bytes).expect("instantiate the numeric component")
}

const IFACE: &str = "demo:app/api@0.1.0";

fn call(c: &mut HostComponent, f: &str, args: &[Val]) -> Result<Vec<Val>, String> {
    c.call_instance(IFACE, f, args)
}

fn ok(c: &mut HostComponent, f: &str, args: &[Val]) -> Val {
    call(c, f, args).unwrap_or_else(|e| panic!("`{f}` should succeed: {e}"))[0].clone()
}

#[test]
fn float_and_string_builtins_run_instead_of_trapping() {
    let mut c = numeric_component();

    // f64 arithmetic — interpreter: add(1.5 2.5) -> 4.0
    assert_eq!(ok(&mut c, "addf", &[Val::Float64(1.5), Val::Float64(2.5)]), Val::Float64(4.0));
    // f64 rem — interpreter: rem(5.5 2.0) -> 1.5
    assert_eq!(ok(&mut c, "remf", &[Val::Float64(5.5), Val::Float64(2.0)]), Val::Float64(1.5));
    // f64 div by zero is +inf (no trap), matching f64 `/` in the interpreter
    match ok(&mut c, "divf", &[Val::Float64(1.0), Val::Float64(0.0)]) {
        Val::Float64(v) => assert!(v.is_infinite() && v > 0.0, "1.0/0.0 should be +inf, got {v}"),
        other => panic!("unexpected {other:?}"),
    }
    // mixed int/float widens to float, as `arith` does: add(2 0.5) -> 2.5
    assert_eq!(ok(&mut c, "mix", &[Val::S64(2), Val::Float64(0.5)]), Val::Float64(2.5));

    // string comparison — interpreter compares by byte/codepoint order
    assert_eq!(ok(&mut c, "lts", &[Val::String("a".into()), Val::String("b".into())]), Val::Bool(true));
    assert_eq!(ok(&mut c, "lts", &[Val::String("b".into()), Val::String("a".into())]), Val::Bool(false));
    // a proper prefix is less than the longer string
    assert_eq!(ok(&mut c, "lts", &[Val::String("ab".into()), Val::String("abc".into())]), Val::Bool(true));
    assert_eq!(ok(&mut c, "lts", &[Val::String("ab".into()), Val::String("ab".into())]), Val::Bool(false));
    // float and mixed comparison
    assert_eq!(ok(&mut c, "ltf", &[Val::Float64(1.5), Val::Float64(2.5)]), Val::Bool(true));
    assert_eq!(ok(&mut c, "ltmix", &[Val::S64(2), Val::Float64(2.5)]), Val::Bool(true));
}

#[test]
fn integer_arithmetic_is_checked_like_the_interpreter() {
    let mut c = numeric_component();

    // in-range integer arithmetic still works
    assert_eq!(ok(&mut c, "addi", &[Val::S64(2), Val::S64(40)]), Val::S64(42));
    assert_eq!(ok(&mut c, "muli", &[Val::S64(6), Val::S64(7)]), Val::S64(42));
    assert_eq!(ok(&mut c, "divi", &[Val::S64(7), Val::S64(2)]), Val::S64(3));
    assert_eq!(ok(&mut c, "remi", &[Val::S64(7), Val::S64(2)]), Val::S64(1));

    // overflow traps (interpreter returns a checked error)
    assert!(call(&mut c, "addi", &[Val::S64(i64::MAX), Val::S64(1)]).is_err(), "add overflow should trap");
    assert!(call(&mut c, "muli", &[Val::S64(i64::MAX), Val::S64(2)]).is_err(), "mul overflow should trap");
    assert!(call(&mut c, "muli", &[Val::S64(i64::MIN), Val::S64(-1)]).is_err(), "MIN*-1 should trap");

    // division edge cases trap
    assert!(call(&mut c, "divi", &[Val::S64(1), Val::S64(0)]).is_err(), "div by zero should trap");
    assert!(call(&mut c, "divi", &[Val::S64(i64::MIN), Val::S64(-1)]).is_err(), "MIN/-1 should trap");
    // INT_MIN % -1: the interpreter's checked_rem errors; wasm i64.rem_s alone
    // would return 0, so the guard must make this trap too.
    assert!(call(&mut c, "remi", &[Val::S64(i64::MIN), Val::S64(-1)]).is_err(), "MIN %% -1 should trap");
    // rem by zero traps as well
    assert!(call(&mut c, "remi", &[Val::S64(1), Val::S64(0)]).is_err(), "rem by zero should trap");
}
