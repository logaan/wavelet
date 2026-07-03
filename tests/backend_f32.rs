//! f32 at the compiled boundary (goal 5 / 5.2): `f32` is a BOUNDARY-ONLY
//! representation — one f32 flat, 4 bytes in memory — carried internally as
//! the interpreter's f64 `Value::Dec` (promote on lift, demote on lower),
//! because the interpreter models every float as f64. These tests pin flat
//! params/results, record fields (memory layout), list elements, and option
//! payloads. Restores the conformance queue's f32-rt / every-primitive-rt
//! coverage in-tree.

use wavelet::host::{HostComponent, Val};

fn component() -> HostComponent {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-f32-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let app = r#"Package "demo:app@0.1.0"

DefType mixed {a: f32 b: u8 c: f64 d: string}

Export {name: bump params: {v: f32} result: f32}
Def bump Fn {v: f32} add(v 1.0)

Export {name: fields params: {m: mixed} result: mixed}
Def fields Fn {m: mixed}
  Match m [({a: a b: b c: c d: d}
            {a: add(a 1.0) b: add(b 1) c: add(c 1.0) d: str-cat(d "!")})]

Export {name: sum params: {xs: list(f32)} result: f32}
Def sum Fn {xs: list(f32)}
  Match xs [
    ([] 0.0)
    (other add(head(other) sum(tail(other))))
  ]

Export {name: maybe params: {v: option(f32)} result: option(f32)}
Def maybe Fn {v: option(f32)}
  Match v [
    (some(x) some(add(x 1.0)))
    (none none)
  ]
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();
    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the f32 app");
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
fn f32_flat_param_and_result() {
    let mut c = component();
    assert_eq!(
        ok(&mut c, "bump", &[Val::Float32(-0.75)]),
        Val::Float32(0.25)
    );
}

#[test]
fn f32_record_fields_lay_out_at_canonical_offsets() {
    let mut c = component();
    let m = Val::Record(vec![
        ("a".into(), Val::Float32(1.5)),
        ("b".into(), Val::U8(7)),
        ("c".into(), Val::Float64(2.5)),
        ("d".into(), Val::String("x".into())),
    ]);
    let want = Val::Record(vec![
        ("a".into(), Val::Float32(2.5)),
        ("b".into(), Val::U8(8)),
        ("c".into(), Val::Float64(3.5)),
        ("d".into(), Val::String("x!".into())),
    ]);
    assert_eq!(ok(&mut c, "fields", &[m]), want);
}

#[test]
fn f32_list_elements_pack_at_stride_4() {
    let mut c = component();
    let xs = Val::List(vec![
        Val::Float32(0.5),
        Val::Float32(1.5),
        Val::Float32(-0.25),
    ]);
    assert_eq!(ok(&mut c, "sum", &[xs]), Val::Float32(1.75));
}

#[test]
fn f32_option_payloads_round_trip() {
    let mut c = component();
    assert_eq!(
        ok(
            &mut c,
            "maybe",
            &[Val::Option(Some(Box::new(Val::Float32(0.5))))]
        ),
        Val::Option(Some(Box::new(Val::Float32(1.5))))
    );
    assert_eq!(ok(&mut c, "maybe", &[Val::Option(None)]), Val::Option(None));
}
