use wavelet::host::{HostComponent, Val};

fn component() -> HostComponent {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-flags-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let app = r#"Package "demo:app@0.1.0"

DefType perms {read write exec}

Export {name: keep params: {p: perms} result: perms}
Def keep Fn {p: perms} p

Export {name: writable params: {} result: perms}
Def writable Fn {} {write}

Export {name: same params: {p: perms} result: bool}
Def same Fn {p: perms} eq(p {read exec})

Export {name: label params: {p: perms} result: string}
Def label Fn {p: perms}
  Match p [
    ({} "empty")
    ({write} "write-only")
    (other "other")
  ]

DefType access {read write exec admin}
DefType entry {lead: u8 perms: access trail: u32}

Export {name: mk-entry params: {} result: entry}
Def mk-entry Fn {} The entry {lead: 7 perms: {read exec} trail: 42}
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();
    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the flags app");
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
fn flags_round_trip_through_the_boundary() {
    let mut c = component();
    let p = Val::Flags(vec!["read".into(), "exec".into()]);
    assert_eq!(ok(&mut c, "keep", std::slice::from_ref(&p)), p);

    assert_eq!(
        ok(&mut c, "keep", &[Val::Flags(vec![])]),
        Val::Flags(vec![])
    );
}

#[test]
fn flags_literal_lowers_as_a_result() {
    let mut c = component();
    assert_eq!(
        ok(&mut c, "writable", &[]),
        Val::Flags(vec!["write".into()])
    );
}

#[test]
fn flags_eq_is_the_interpreters_value_flg_equality() {
    let mut c = component();

    assert_eq!(
        ok(
            &mut c,
            "same",
            &[Val::Flags(vec!["read".into(), "exec".into()])]
        ),
        Val::Bool(true)
    );
    assert_eq!(
        ok(&mut c, "same", &[Val::Flags(vec!["read".into()])]),
        Val::Bool(false)
    );
}

#[test]
fn flags_literals_match_as_patterns() {
    let mut c = component();
    assert_eq!(
        ok(&mut c, "label", &[Val::Flags(vec![])]),
        Val::String("empty".into())
    );
    assert_eq!(
        ok(&mut c, "label", &[Val::Flags(vec!["write".into()])]),
        Val::String("write-only".into())
    );
    assert_eq!(
        ok(
            &mut c,
            "label",
            &[Val::Flags(vec!["read".into(), "write".into()])]
        ),
        Val::String("other".into())
    );
}

#[test]
fn canonical_flags_field_uses_declared_bit_positions_and_alignment() {
    let mut c = component();
    let got = ok(&mut c, "mk-entry", &[]);
    let Val::Record(fields) = got else {
        panic!("mk-entry should return a record, got {got:?}");
    };
    assert_eq!(
        fields,
        vec![
            ("lead".to_string(), Val::U8(7)),
            (
                "perms".to_string(),
                Val::Flags(vec!["read".into(), "exec".into()])
            ),
            ("trail".to_string(), Val::U32(42)),
        ]
    );
}
