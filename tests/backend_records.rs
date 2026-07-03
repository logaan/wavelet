//! Goal 5 (5.3 canonical records): a `Let`-bound record whose construction
//! is provably faithful to its static type lives in canonical ABI layout
//! (`Repr::Mem`) instead of a `TAG_REC` box; boxed consumers rebuild the box
//! at the reference seam. These tests pin the *faithfulness* of that
//! representation against the interpreter's record semantics, which are
//! field-order-sensitive (`Value::Rec` is a `Vec`; `eq` compares
//! positionally): a canonical-layout value must rebuild, at every boxed
//! seam, exactly the box the interpreter would have carried.

use wavelet::host::{HostComponent, Val};

fn component() -> HostComponent {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-memrec-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let app = r#"Package "demo:app@0.1.0"

DefType point {x: s32 y: s32}

Export {name: mk-point params: {a: s32 b: s32} result: point}
Def mk-point Fn {a: s32 b: s32}
  Let {p: {x: a y: b}} p

Export {name: same-order params: {} result: bool}
Def same-order Fn {}
  Let {p: {x: 1 y: 2}} eq(p {x: 1 y: 2})

Export {name: diff-order params: {} result: bool}
Def diff-order Fn {}
  Let {p: {x: 1 y: 2}} eq(p {y: 2 x: 1})

Export {name: self-eq params: {} result: bool}
Def self-eq Fn {}
  Let {p: {x: 1 y: 2}} eq(p p)

Export {name: nested-roundtrip params: {} result: bool}
Def nested-roundtrip Fn {}
  Let {r: {a: {x: 1 y: 2} s: "hi" n: 300}}
    eq(r {a: {x: 1 y: 2} s: "hi" n: 300})

Export {name: narrow params: {a: u8 b: u16} result: s64}
Def narrow Fn {a: u8 b: u16}
  Let {p: {lo: a hi: b}}
    Match p [({lo: l hi: h} add(l h))]

Export {name: destructure params: {} result: s64}
Def destructure Fn {}
  Let {r: {a: {x: 5 y: 7} n: 300}}
    Match r [({a: {x: xx} n: nn} add(xx nn)) (other 0)]

Export {name: fallthrough params: {} result: s64}
Def fallthrough Fn {}
  Let {p: {x: 1 y: 2}}
    Match p [({z: q} 0) ({x: 9} 1) ({x: 1} 2) (other 3)]

Export {name: not-a-variant params: {} result: s64}
Def not-a-variant Fn {}
  Let {p: {x: 1 y: 2}}
    Match p [((some q) 0) (other 1)]

Export {name: pick-point params: {c: bool a: s32 b: s32} result: point}
Def pick-point Fn {c: bool a: s32 b: s32}
  If c {x: a y: b} {x: b y: a}

Def mk Fn {a: s32 b: s32} Let {p: {x: a y: b}} p

Export {name: use-internal params: {} result: bool}
Def use-internal Fn {} eq(mk(1 2) {x: 1 y: 2})

Export {name: through-closure params: {} result: bool}
Def through-closure Fn {}
  Let {p: {x: 1 y: 2}}
    Let {f: Fn {} p}
      eq(f() {x: 1 y: 2})
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();
    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the canonical-records app");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);
    HostComponent::from_bytes(&bytes).expect("instantiate")
}

const IFACE: &str = "demo:app/api@0.1.0";

fn ok(c: &mut HostComponent, f: &str, args: &[Val]) -> Val {
    c.call_instance(IFACE, f, args)
        .unwrap_or_else(|e| panic!("`{f}` should succeed: {e}"))[0]
        .clone()
}

#[test]
// A canonical-layout binding lowers at the boundary exactly like the boxed
// value would: the host sees the declared record.
fn canonical_record_lowers_to_the_declared_record() {
    let mut c = component();
    let got = ok(&mut c, "mk-point", &[Val::S32(7), Val::S32(-3)]);
    let Val::Record(fields) = got else {
        panic!("mk-point should return a record, got {got:?}");
    };
    assert_eq!(
        fields,
        vec![
            ("x".to_string(), Val::S32(7)),
            ("y".to_string(), Val::S32(-3)),
        ]
    );
}

#[test]
// The rebuilt box is exactly the interpreter's value: eq is field-order
// sensitive (Value::Rec is a Vec), so the same order compares true and a
// permuted literal compares false — precisely the oracle's answers.
fn canonical_record_rebuild_preserves_field_order() {
    let mut c = component();
    assert_eq!(ok(&mut c, "same-order", &[]), Val::Bool(true));
    assert_eq!(ok(&mut c, "diff-order", &[]), Val::Bool(false));
    assert_eq!(ok(&mut c, "self-eq", &[]), Val::Bool(true));
}

#[test]
// Nested record fields construct in place (no intermediate boxes) and
// rebuild faithfully: nested structure, a string field, and an int-literal
// field (which lays out as s64 — the interpreter's full Value::Int domain).
fn nested_canonical_record_roundtrips() {
    let mut c = component();
    assert_eq!(ok(&mut c, "nested-roundtrip", &[]), Val::Bool(true));
}

#[test]
// Narrow int fields (u8/u16) store at WIT width — the gate proved the
// static range fits — and reload into the interpreter's Value::Int domain.
fn narrow_int_fields_roundtrip_losslessly() {
    let mut c = component();
    assert_eq!(
        ok(&mut c, "narrow", &[Val::U8(200), Val::U16(60000)]),
        Val::S64(60200)
    );
}

#[test]
// Record patterns destructure canonical layout at despec offsets: nested
// record patterns recurse in place, scalar binders load typed locals.
fn canonical_record_patterns_destructure_by_offset() {
    let mut c = component();
    assert_eq!(ok(&mut c, "destructure", &[]), Val::S64(305));
}

#[test]
// Clause fallthrough matches the oracle: a statically-absent field fails
// the clause, a literal field pattern compares structurally, and the first
// matching clause wins.
fn canonical_record_pattern_fallthrough_matches_the_oracle() {
    let mut c = component();
    assert_eq!(ok(&mut c, "fallthrough", &[]), Val::S64(2));
}

#[test]
// A non-record pattern (variant case) against a canonical record scrutinee
// fails that clause — the rebuilt-box fallback preserves the uniform
// matcher's semantics — and the bare-binder clause matches.
fn non_record_pattern_falls_through_on_canonical_scrutinee() {
    let mut c = component();
    assert_eq!(ok(&mut c, "not-a-variant", &[]), Val::S64(1));
}

#[test]
// A def whose BODY is provably canonical carries a Mem result signature:
// the export wrapper returns the canonical pointer directly when the
// layout equals the boundary type (the 5.3 retptr fast path), and an If
// threads the Mem want through both branches.
fn mem_result_def_through_if_lowers_correctly() {
    let mut c = component();
    for (cond, want) in [(true, (7, -3)), (false, (-3, 7))] {
        let got = ok(
            &mut c,
            "pick-point",
            &[Val::Bool(cond), Val::S32(7), Val::S32(-3)],
        );
        let Val::Record(fields) = got else {
            panic!("pick-point should return a record, got {got:?}");
        };
        assert_eq!(
            fields,
            vec![
                ("x".to_string(), Val::S32(want.0)),
                ("y".to_string(), Val::S32(want.1)),
            ]
        );
    }
}

#[test]
// An internal caller of a Mem-result def reboxes at the call seam: the
// rebuilt box is exactly the interpreter's record value.
fn internal_call_of_mem_result_def_reboxes_faithfully() {
    let mut c = component();
    assert_eq!(ok(&mut c, "use-internal", &[]), Val::Bool(true));
}

#[test]
// A canonical-layout local captured by a closure boxes at the capture seam
// (capture slots hold box pointers) with its exact field order.
fn canonical_record_closure_capture_is_faithful() {
    let mut c = component();
    assert_eq!(ok(&mut c, "through-closure", &[]), Val::Bool(true));
}

/// Is `bin` runnable (`<bin> --version` succeeds)?
fn have(bin: &str) -> bool {
    std::process::Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
// A dep call returning a record consumed by Match: the retptr area the
// callee wrote is already the canonical layout (declared field order —
// exactly what the interpreter's boundary lift produces), so the value is
// born canonical and destructures by offset with no boxes at all.
fn dep_call_record_result_is_born_canonical() {
    if !have("wac") {
        eprintln!("skipping: wac not on PATH");
        return;
    }
    let dir = std::env::temp_dir().join(format!("wavelet-memdep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("pts.wlt"),
        "Package \"demo:pts@0.1.0\"\n\n\
         DefType point {x: s32 y: s32}\n\n\
         Export {name: mk params: {a: s32 b: s32} result: point}\n\
         Def mk Fn {a: s32 b: s32} {x: a y: b}\n",
    )
    .unwrap();
    std::fs::write(
        src.join("main.wlt"),
        "Package \"demo:main@0.1.0\"\n\n\
         Import {pkg: \"demo:pts/api\" as: pts}\n\n\
         Export {name: run params: {} result: s64}\n\
         Def run Fn {}\n  \
           Match pts/mk(4 9) [({x: xx y: yy} add(xx yy)) (other 0)]\n",
    )
    .unwrap();
    let out = dir.join("out");
    let sources = vec![
        src.join("pts.wlt").to_str().unwrap().to_string(),
        src.join("main.wlt").to_str().unwrap().to_string(),
    ];
    wavelet::build::build_files(&sources, out.to_str().unwrap())
        .expect("build the two-component project");
    let app = out.join("app.wasm");
    let bytes = std::fs::read(&app).expect("read composed app.wasm");
    let _ = std::fs::remove_dir_all(&dir);
    let mut c = HostComponent::from_bytes(&bytes).expect("instantiate composed app");
    let got = c
        .call_instance("demo:main/api@0.1.0", "run", &[])
        .expect("call run")[0]
        .clone();
    assert_eq!(got, Val::S64(13));
}
