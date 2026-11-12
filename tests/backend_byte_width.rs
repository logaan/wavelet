use wavelet::host::{HostComponent, Val};

fn byte_width_component() -> HostComponent {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-byte-width-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let app = r#"Package "demo:app@0.1.0"

Export {name: head8-wide params: {xs: list(u8)} result: s64}
Def head8-wide Fn {xs}
  to-s64(head(xs))

Export {name: u8s-ok params: {xs: list(u8)} result: bool}
Def u8s-ok Fn {xs}
  eq(xs [1 2 3])

Export {name: u16s-ok params: {xs: list(u16)} result: bool}
Def u16s-ok Fn {xs}
  eq(xs [513 2 65535])

Export {name: s8s-ok params: {xs: list(s8)} result: bool}
Def s8s-ok Fn {xs}
  eq(xs [-128 -1 127])

Export {name: s16s-ok params: {xs: list(s16)} result: bool}
Def s16s-ok Fn {xs}
  eq(xs [-32768 -1 32767])

Export {name: mixed-ok params: {xs: list(tuple(u8 u16 u8))} result: bool}
Def mixed-ok Fn {xs}
  eq(xs [tuple3(1 513 2) tuple3(255 65535 0)])

Export {name: opts-ok params: {xs: list(option(u8))} result: bool}
Def opts-ok Fn {xs}
  eq(xs [some(1) some(255)])

Export {name: echo8 params: {xs: list(u8)} result: list(u8)}
Def echo8 Fn {xs}
  xs

Export {name: echo-opt8 params: {xs: list(option(u8))} result: list(option(u8))}
Def echo-opt8 Fn {xs}
  xs

Export {name: mk-opt8 params: {} result: option(u8)}
Def mk-opt8 Fn {}
  some(7)
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();

    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the byte-width API component");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);

    HostComponent::from_bytes(&bytes).expect("instantiate the byte-width component")
}

const IFACE: &str = "demo:app/api@0.1.0";

fn ok(c: &mut HostComponent, f: &str, args: &[Val]) -> Val {
    c.call_instance(IFACE, f, args)
        .unwrap_or_else(|e| panic!("`{f}` should succeed: {e}"))[0]
        .clone()
}

fn assert_in_guest(c: &mut HostComponent, f: &str, arg: Val) {
    assert_eq!(
        ok(c, f, &[arg]),
        Val::Bool(true),
        "`{f}` saw a corrupt lift"
    );
}

#[test]
fn narrow_list_elements_lift_at_canonical_stride() {
    let mut c = byte_width_component();

    let u8s = Val::List(vec![Val::U8(1), Val::U8(2), Val::U8(3)]);
    assert_eq!(
        ok(&mut c, "head8-wide", std::slice::from_ref(&u8s)),
        Val::S64(1)
    );
    assert_in_guest(&mut c, "u8s-ok", u8s);

    let u16s = Val::List(vec![Val::U16(513), Val::U16(2), Val::U16(65535)]);
    assert_in_guest(&mut c, "u16s-ok", u16s);
}

#[test]
fn signed_narrow_elements_sign_extend_in_guest() {
    let mut c = byte_width_component();
    let s8s = Val::List(vec![Val::S8(-128), Val::S8(-1), Val::S8(127)]);
    assert_in_guest(&mut c, "s8s-ok", s8s);

    let s16s = Val::List(vec![Val::S16(-32768), Val::S16(-1), Val::S16(32767)]);
    assert_in_guest(&mut c, "s16s-ok", s16s);
}

#[test]
fn mixed_width_tuple_fields_sit_at_canonical_offsets() {
    let mut c = byte_width_component();

    let xs = Val::List(vec![
        Val::Tuple(vec![Val::U8(1), Val::U16(513), Val::U8(2)]),
        Val::Tuple(vec![Val::U8(255), Val::U16(65535), Val::U8(0)]),
    ]);
    assert_in_guest(&mut c, "mixed-ok", xs);
}

#[test]
fn option_u8_payload_sits_next_to_its_discriminant() {
    let mut c = byte_width_component();

    let somes = Val::List(vec![
        Val::Option(Some(Box::new(Val::U8(1)))),
        Val::Option(Some(Box::new(Val::U8(255)))),
    ]);
    assert_in_guest(&mut c, "opts-ok", somes);

    let with_none = Val::List(vec![
        Val::Option(Some(Box::new(Val::U8(1)))),
        Val::Option(None),
        Val::Option(Some(Box::new(Val::U8(255)))),
    ]);
    assert_eq!(
        ok(&mut c, "echo-opt8", std::slice::from_ref(&with_none)),
        with_none
    );

    assert_eq!(
        ok(&mut c, "mk-opt8", &[]),
        Val::Option(Some(Box::new(Val::U8(7))))
    );
}

#[test]
fn u8_list_echo_round_trips() {
    let mut c = byte_width_component();
    let xs = Val::List(vec![Val::U8(0), Val::U8(1), Val::U8(255)]);
    assert_eq!(ok(&mut c, "echo8", std::slice::from_ref(&xs)), xs);
}
