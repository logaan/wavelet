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

    assert_eq!(
        ok(&mut c, "nest", &[Val::S64(6), Val::S64(7), Val::S64(1)]),
        Val::S64(42)
    );

    assert_eq!(
        ok(&mut c, "divi", &[Val::S64(-7), Val::S64(2)]),
        Val::S64(-3)
    );
    assert_eq!(ok(&mut c, "negi", &[Val::S64(-42)]), Val::S64(42));
    assert_eq!(ok(&mut c, "negf", &[Val::Float64(1.5)]), Val::Float64(-1.5));
}

#[test]
fn typed_int_arithmetic_traps_where_the_interpreter_errors() {
    assert!(call(&mut component(), "negi", &[Val::S64(i64::MIN)]).is_err());

    assert!(call(&mut component(), "addi", &[Val::S64(i64::MAX), Val::S64(1)]).is_err());

    assert!(call(&mut component(), "divi", &[Val::S64(1), Val::S64(0)]).is_err());

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

    let a = 9_007_199_254_740_993i64;
    let b = 9_007_199_254_740_992i64;
    assert_eq!(ok(&mut c, "lti", &[Val::S64(b), Val::S64(a)]), {
        Val::Bool((b as f64) < (a as f64))
    });
    assert_eq!(
        ok(&mut c, "gef", &[Val::Float64(2.0), Val::Float64(2.0)]),
        Val::Bool(true)
    );

    assert_eq!(
        ok(&mut c, "ltc", &[Val::Char('a'), Val::Char('b')]),
        Val::Bool(true)
    );
    assert_eq!(
        ok(&mut c, "ltc", &[Val::Char('b'), Val::Char('a')]),
        Val::Bool(false)
    );

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

    assert_eq!(
        c.call_instance(iface, "next-char", &[Val::Char('a')])
            .unwrap()[0],
        Val::Char('b')
    );

    assert_eq!(
        c.call_instance(iface, "floor64", &[Val::Float64(-3.0)])
            .unwrap()[0],
        Val::S64(-3)
    );
    assert_eq!(
        c.call_instance(iface, "widen", &[Val::S64(7)]).unwrap()[0],
        Val::Float64(7.0)
    );

    let mut t1 = HostComponent::from_bytes(&bytes).unwrap();
    assert!(t1.call_instance(iface, "clamp8", &[Val::S64(256)]).is_err());
    let mut t2 = HostComponent::from_bytes(&bytes).unwrap();
    assert!(
        t2.call_instance(iface, "floor64", &[Val::Float64(1.5)])
            .is_err()
    );
}

#[test]
fn typed_defs_keep_tail_recursion() {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-tco-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let app = r#"Package "demo:app@0.1.0"

Def countdown Fn {n: u64 acc: u64}
  If eq(n 0) acc countdown(sub(n 1) add(acc 1))

Export {name: run params: {n: u64} result: u64}
Def run Fn {n: u64} countdown(n 0)
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();
    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the tco app");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);
    let mut c = HostComponent::from_bytes(&bytes).expect("instantiate");
    let got = c
        .call_instance("demo:app/api@0.1.0", "run", &[Val::U64(1_000_000)])
        .expect("a million-deep tail recursion must not overflow");
    assert_eq!(got[0], Val::U64(1_000_000));
}

#[test]
fn char_literals_match_as_patterns() {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-charpat-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let app = r#"Package "demo:app@0.1.0"

Export {name: kind params: {c: char} result: string}
Def kind Fn {c: char}
  Match c [
    ('a' "first")
    ('w' "double-u")
    (other "other")
  ]
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();
    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the char-pattern app");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);
    let mut c = HostComponent::from_bytes(&bytes).expect("instantiate");
    let iface = "demo:app/api@0.1.0";
    let call = |c: &mut HostComponent, ch: char| {
        c.call_instance(iface, "kind", &[Val::Char(ch)]).unwrap()[0].clone()
    };
    assert_eq!(call(&mut c, 'a'), Val::String("first".into()));
    assert_eq!(call(&mut c, 'w'), Val::String("double-u".into()));
    assert_eq!(call(&mut c, 'z'), Val::String("other".into()));
}
