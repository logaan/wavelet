//! Goal 5 (5.2 unboxed scalars + 5.6.1 static arithmetic): when operand
//! types are statically known, the backend compiles the polymorphic scalar
//! builtins to unboxed per-type code (shared `arith_int`/`cmp_f64` semantic
//! cores). The interpreter is the semantics oracle — these tests pin the
//! typed path to the interpreter's exact behaviour, including its edges:
//! checked i64 arithmetic (trap where the interpreter errors), f64-widened
//! comparisons (ints included), wrapping `neg`, codepoint char order, and
//! the boxed fallback for strings/mixed kinds.

use wavelet::host::{HostComponent, Val};

fn component() -> HostComponent {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-typedscalar-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let app = r#"Package "demo:app@0.1.0"

Export {name: addi params: {a: s64 b: s64} result: s64}
Def addi Fn {a: s64 b: s64} add(a b)

Export {name: nest params: {a: s64 b: s64 c: s64} result: s64}
Def nest Fn {a: s64 b: s64 c: s64} add(mul(a b) sub(c 1))

Export {name: mixed params: {a: s64 x: f64} result: f64}
Def mixed Fn {a: s64 x: f64} add(a x)

Export {name: negi params: {a: s64} result: s64}
Def negi Fn {a: s64} neg(a)

Export {name: negf params: {x: f64} result: f64}
Def negf Fn {x: f64} neg(x)

Export {name: divi params: {a: s64 b: s64} result: s64}
Def divi Fn {a: s64 b: s64} div(a b)

Export {name: remf params: {x: f64 y: f64} result: f64}
Def remf Fn {x: f64 y: f64} rem(x y)

Export {name: lti params: {a: s64 b: s64} result: bool}
Def lti Fn {a: s64 b: s64} lt(a b)

Export {name: gef params: {x: f64 y: f64} result: bool}
Def gef Fn {x: f64 y: f64} ge(x y)

Export {name: ltc params: {a: char b: char} result: bool}
Def ltc Fn {a: char b: char} lt(a b)

Export {name: eqc params: {a: char b: char} result: bool}
Def eqc Fn {a: char b: char} eq(a b)

Export {name: eqi params: {a: s64 b: s64} result: bool}
Def eqi Fn {a: s64 b: s64} eq(a b)

Export {name: eqf params: {x: f64 y: f64} result: bool}
Def eqf Fn {x: f64 y: f64} eq(x y)

Export {name: eqmix params: {a: s64 x: f64} result: bool}
Def eqmix Fn {a: s64 x: f64} eq(a x)

Export {name: pick params: {a: u32 b: u32} result: u32}
Def pick Fn {a: u32 b: u32} If lt(a b) a b

Export {name: notb params: {p: bool} result: bool}
Def notb Fn {p: bool} not(p)
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();
    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the typed-scalar app");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);
    HostComponent::from_bytes(&bytes).expect("instantiate")
}

const IFACE: &str = "demo:app/api@0.1.0";

fn call(c: &mut HostComponent, f: &str, args: &[Val]) -> Result<Vec<Val>, String> {
    c.call_instance(IFACE, f, args)
}

fn ok(c: &mut HostComponent, f: &str, args: &[Val]) -> Val {
    call(c, f, args).unwrap_or_else(|e| panic!("`{f}` should succeed: {e}"))[0].clone()
}

#[test]
fn typed_int_arithmetic_matches_the_interpreter() {
    let mut c = component();
    assert_eq!(
        ok(&mut c, "addi", &[Val::S64(40), Val::S64(2)]),
        Val::S64(42)
    );
    // nested chain computes unboxed end-to-end
    assert_eq!(
        ok(&mut c, "nest", &[Val::S64(6), Val::S64(7), Val::S64(1)]),
        Val::S64(42)
    );
    // int division truncates like the interpreter's checked_div
    assert_eq!(
        ok(&mut c, "divi", &[Val::S64(-7), Val::S64(2)]),
        Val::S64(-3)
    );
    // wrapping neg, exactly neg_raw / the interpreter's `-n`
    assert_eq!(
        ok(&mut c, "negi", &[Val::S64(i64::MIN)]),
        Val::S64(i64::MIN)
    );
    assert_eq!(ok(&mut c, "negf", &[Val::Float64(1.5)]), Val::Float64(-1.5));
}

#[test]
fn typed_int_arithmetic_traps_where_the_interpreter_errors() {
    // a trap poisons the component instance, so each case gets a fresh one
    // i64 overflow — interpreter: `add` error; backend: trap
    assert!(call(&mut component(), "addi", &[Val::S64(i64::MAX), Val::S64(1)]).is_err());
    // div by zero
    assert!(call(&mut component(), "divi", &[Val::S64(1), Val::S64(0)]).is_err());
    // INT_MIN / -1
    assert!(
        call(
            &mut component(),
            "divi",
            &[Val::S64(i64::MIN), Val::S64(-1)]
        )
        .is_err()
    );
}

#[test]
fn typed_mixed_arithmetic_widens_to_f64() {
    let mut c = component();
    assert_eq!(
        ok(&mut c, "mixed", &[Val::S64(1), Val::Float64(2.5)]),
        Val::Float64(3.5)
    );
    // f64 rem matches Rust `%`
    assert_eq!(
        ok(&mut c, "remf", &[Val::Float64(5.5), Val::Float64(2.0)]),
        Val::Float64(1.5)
    );
}

#[test]
fn typed_comparisons_match_the_interpreter() {
    let mut c = component();
    assert_eq!(
        ok(&mut c, "lti", &[Val::S64(1), Val::S64(2)]),
        Val::Bool(true)
    );
    // ints compare WIDENED TO F64 like the interpreter's `compare`: these two
    // huge ints are equal at f64 precision, so neither is less
    let a = 9_007_199_254_740_993i64; // 2^53 + 1
    let b = 9_007_199_254_740_992i64; // 2^53
    assert_eq!(ok(&mut c, "lti", &[Val::S64(b), Val::S64(a)]), {
        // oracle: interpreter compare = (b as f64).partial_cmp(&(a as f64))
        Val::Bool((b as f64) < (a as f64))
    });
    assert_eq!(
        ok(&mut c, "gef", &[Val::Float64(2.0), Val::Float64(2.0)]),
        Val::Bool(true)
    );
    // chars order by codepoint
    assert_eq!(
        ok(&mut c, "ltc", &[Val::Char('a'), Val::Char('b')]),
        Val::Bool(true)
    );
    assert_eq!(
        ok(&mut c, "ltc", &[Val::Char('b'), Val::Char('a')]),
        Val::Bool(false)
    );
    // NaN compare: interpreter "values are not comparable" -> backend trap
    // (last: the trap poisons the instance)
    assert!(call(&mut c, "gef", &[Val::Float64(f64::NAN), Val::Float64(1.0)]).is_err());
}

#[test]
fn typed_eq_matches_the_interpreter() {
    let mut c = component();
    assert_eq!(
        ok(&mut c, "eqi", &[Val::S64(7), Val::S64(7)]),
        Val::Bool(true)
    );
    assert_eq!(
        ok(&mut c, "eqf", &[Val::Float64(0.5), Val::Float64(0.5)]),
        Val::Bool(true)
    );
    // NaN != NaN, like Value::Dec's PartialEq
    assert_eq!(
        ok(
            &mut c,
            "eqf",
            &[Val::Float64(f64::NAN), Val::Float64(f64::NAN)]
        ),
        Val::Bool(false)
    );
    assert_eq!(
        ok(&mut c, "eqc", &[Val::Char('x'), Val::Char('x')]),
        Val::Bool(true)
    );
    // mixed int/float eq is FALSE at the value level (Value::Int != Value::Dec),
    // via the boxed eq_raw fallback
    assert_eq!(
        ok(&mut c, "eqmix", &[Val::S64(1), Val::Float64(1.0)]),
        Val::Bool(false)
    );
}

#[test]
fn typed_bool_conditions_and_not() {
    let mut c = component();
    assert_eq!(ok(&mut c, "pick", &[Val::U32(3), Val::U32(9)]), Val::U32(3));
    assert_eq!(ok(&mut c, "pick", &[Val::U32(9), Val::U32(3)]), Val::U32(3));
    assert_eq!(ok(&mut c, "notb", &[Val::Bool(true)]), Val::Bool(false));
}

/// Numeric conversion builtins compile per-kind (5.6): range checks trap
/// exactly where the interpreter errors, chars convert through their
/// codepoint (the char-rt "next scalar" transform), whole floats truncate.
#[test]
fn typed_numeric_conversions_match_the_interpreter() {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-conv-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let app = r#"Package "demo:app@0.1.0"

Export {name: clamp8 params: {a: s64} result: u8}
Def clamp8 Fn {a: s64} to-u8(a)

Export {name: next-char params: {c: char} result: char}
Def next-char Fn {c: char} to-char(add(to-u32(c) 1))

Export {name: floor64 params: {x: f64} result: s64}
Def floor64 Fn {x: f64} to-s64(x)

Export {name: widen params: {a: s64} result: f64}
Def widen Fn {a: s64} to-f64(a)
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();
    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the conversions app");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);

    let iface = "demo:app/api@0.1.0";
    let mut c = HostComponent::from_bytes(&bytes).expect("instantiate");
    assert_eq!(
        c.call_instance(iface, "clamp8", &[Val::S64(200)]).unwrap()[0],
        Val::U8(200)
    );
    // the char-rt transform: next Unicode scalar
    assert_eq!(
        c.call_instance(iface, "next-char", &[Val::Char('a')])
            .unwrap()[0],
        Val::Char('b')
    );
    // whole float truncates; s64 result
    assert_eq!(
        c.call_instance(iface, "floor64", &[Val::Float64(-3.0)])
            .unwrap()[0],
        Val::S64(-3)
    );
    assert_eq!(
        c.call_instance(iface, "widen", &[Val::S64(7)]).unwrap()[0],
        Val::Float64(7.0)
    );
    // traps where the interpreter errors (fresh instances per trap)
    let mut t1 = HostComponent::from_bytes(&bytes).unwrap();
    assert!(t1.call_instance(iface, "clamp8", &[Val::S64(256)]).is_err());
    let mut t2 = HostComponent::from_bytes(&bytes).unwrap();
    assert!(
        t2.call_instance(iface, "floor64", &[Val::Float64(1.5)])
            .is_err()
    );
}
