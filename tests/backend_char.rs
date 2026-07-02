//! Backend regression: chars at the component boundary and `to-char`.
//!
//! Chars box as `TAG_CHAR` in compiled code, but the boundary used to lower
//! them through `unbox_int` — which traps unless the tag is `TAG_INT` — so
//! calling any imported/exported function with a char argument hit a wasm
//! `unreachable`. Lifting had the mirror bug: an incoming char was boxed as an
//! *int*, so it stopped being a char in-guest (`eq(c 'a')` false, `lt` on it
//! trapped). These tests cross the real component boundary with chars in
//! every placement (flat argument, flat-ish result, list element, option
//! payload) and exercise the `to-char` builtin's int and char paths.

use wavelet::host::{HostComponent, Val};

/// Build the char API and instantiate it. The app is self-contained (no
/// imports), so it builds and runs with no external toolchain.
fn char_component() -> HostComponent {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-char-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let app = r#"Package "demo:app@0.1.0"

Export {name: echo-char params: {c: char} result: char}
Def echo-char Fn {c}
  c

Export {name: is-a params: {c: char} result: bool}
Def is-a Fn {c}
  eq(c 'a')

Export {name: lt-chars params: {a: char b: char} result: bool}
Def lt-chars Fn {a b}
  lt(a b)

Export {name: mk-char params: {} result: char}
Def mk-char Fn {}
  to-char(98)

Export {name: char-through params: {} result: char}
Def char-through Fn {}
  to-char('q')

Export {name: chars-ok params: {cs: list(char)} result: bool}
Def chars-ok Fn {cs}
  eq(cs ['a' '☃'])

Export {name: echo-opt-char params: {x: option(char)} result: option(char)}
Def echo-opt-char Fn {x}
  x
"#;
    let app_path = src.join("app.wvl");
    std::fs::write(&app_path, app).unwrap();

    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the char API component");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);

    HostComponent::from_bytes(&bytes).expect("instantiate the char component")
}

const IFACE: &str = "demo:app/api@0.1.0";

fn ok(c: &mut HostComponent, f: &str, args: &[Val]) -> Val {
    c.call_instance(IFACE, f, args)
        .unwrap_or_else(|e| panic!("`{f}` should succeed: {e}"))[0]
        .clone()
}

#[test]
fn char_arguments_cross_the_boundary_without_trapping() {
    let mut c = char_component();
    // The original failure: any char argument trapped in `unbox_int`.
    assert_eq!(ok(&mut c, "echo-char", &[Val::Char('a')]), Val::Char('a'));
    assert_eq!(ok(&mut c, "echo-char", &[Val::Char('☃')]), Val::Char('☃'));
}

#[test]
fn lifted_chars_stay_chars_in_guest() {
    let mut c = char_component();
    // Pre-fix a lifted char boxed as an int, so `eq(c 'a')` was false.
    assert_eq!(ok(&mut c, "is-a", &[Val::Char('a')]), Val::Bool(true));
    assert_eq!(ok(&mut c, "is-a", &[Val::Char('b')]), Val::Bool(false));
}

#[test]
fn chars_order_by_codepoint() {
    let mut c = char_component();
    let lt = |c: &mut HostComponent, a: char, b: char| {
        ok(c, "lt-chars", &[Val::Char(a), Val::Char(b)])
    };
    assert_eq!(lt(&mut c, 'a', 'b'), Val::Bool(true));
    assert_eq!(lt(&mut c, 'b', 'a'), Val::Bool(false));
    assert_eq!(lt(&mut c, 'a', 'a'), Val::Bool(false));
    assert_eq!(lt(&mut c, 'z', '☃'), Val::Bool(true));
}

#[test]
fn to_char_builds_and_passes_through() {
    let mut c = char_component();
    assert_eq!(ok(&mut c, "mk-char", &[]), Val::Char('b'));
    assert_eq!(ok(&mut c, "char-through", &[]), Val::Char('q'));
}

#[test]
fn chars_in_memory_payloads() {
    let mut c = char_component();
    let cs = Val::List(vec![Val::Char('a'), Val::Char('☃')]);
    assert_eq!(ok(&mut c, "chars-ok", &[cs]), Val::Bool(true));

    let some = Val::Option(Some(Box::new(Val::Char('w'))));
    assert_eq!(ok(&mut c, "echo-opt-char", &[some.clone()]), some);
    let none = Val::Option(None);
    assert_eq!(ok(&mut c, "echo-opt-char", &[none.clone()]), none);
}
