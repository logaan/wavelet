use wavelet::host::{HostComponent, Val};

fn have(bin: &str) -> bool {
    std::process::Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn composed() -> Option<HostComponent> {
    if !have("wac") {
        eprintln!("skipping: wac not on PATH");
        return None;
    }
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-memtup-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let dep = r#"Package "demo:tup@0.1.0"

DefType point {x: s32 y: s32}

Export {name: mk params: {a: s32 b: s32} result: tuple(s32 s32)}
Def mk Fn {a: s32 b: s32} Quasi (Unquote(a) Unquote(b))

Export {name: mix params: {} result: tuple(s32 string)}
Def mix Fn {} Quote (7 "hi")

Export {name: nested params: {} result: tuple(point s64)}
Def nested Fn {} Quote ({x: 5 y: 7} 300)
"#;

    let main = r#"Package "demo:main@0.1.0"

Import {pkg: "demo:tup/api" as: tp}

Export {name: run params: {} result: s64}
Def run Fn {}
  Match tp/mk(4 9) [((a b) add(a b)) (other 0)]

Export {name: fall params: {} result: s64}
Def fall Fn {}
  Match tp/mk(4 9) [((a b c) 0) ((a b) 1) (other 2)]

Export {name: same params: {} result: bool}
Def same Fn {}
  Let {t: tp/mk(1 2)} eq(t Quote (1 2))

Export {name: mixed params: {} result: string}
Def mixed Fn {}
  Match tp/mix() [((7 s) s) (other "no")]

Export {name: deep params: {} result: s64}
Def deep Fn {}
  Match tp/nested() [(({x: xx} n) add(xx n)) (other 0)]

Export {name: fwd params: {a: s32 b: s32} result: tuple(s32 s32)}
Def fwd Fn {a: s32 b: s32} tp/mk(a b)

Def mk2 Fn {} tp/mk(1 2)

Export {name: use-internal params: {} result: bool}
Def use-internal Fn {} eq(mk2() Quote (1 2))

Export {name: t-eq params: {a: s32 b: s32 c: s32 d: s32} result: bool}
Def t-eq Fn {a: s32 b: s32 c: s32 d: s32} eq(tp/mk(a b) tp/mk(c d))
"#;

    std::fs::write(src.join("tup.wlt"), dep).unwrap();
    std::fs::write(src.join("main.wlt"), main).unwrap();
    let out = dir.join("out");
    let sources = vec![
        src.join("tup.wlt").to_str().unwrap().to_string(),
        src.join("main.wlt").to_str().unwrap().to_string(),
    ];
    wavelet::build::build_files(&sources, out.to_str().unwrap())
        .expect("build the two-component project");
    let bytes = std::fs::read(out.join("app.wasm")).expect("read composed app.wasm");
    let _ = std::fs::remove_dir_all(&dir);
    Some(HostComponent::from_bytes(&bytes).expect("instantiate composed app"))
}

const IFACE: &str = "demo:main/api@0.1.0";

fn ok(c: &mut HostComponent, f: &str, args: &[Val]) -> Val {
    c.call_instance(IFACE, f, args)
        .unwrap_or_else(|e| panic!("`{f}` should succeed: {e}"))[0]
        .clone()
}

#[test]

fn dep_born_tuple_destructures_element_wise() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "run", &[]), Val::S64(13));
}

#[test]

fn tuple_pattern_length_mismatch_falls_through() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "fall", &[]), Val::S64(1));
}

#[test]

fn canonical_tuple_rebuild_is_faithful() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "same", &[]), Val::Bool(true));
    assert_eq!(ok(&mut c, "use-internal", &[]), Val::Bool(true));
}

#[test]

fn mixed_tuple_elements_match_and_bind() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "mixed", &[]), Val::String("hi".into()));
}

#[test]

fn record_inside_tuple_destructures_in_place() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "deep", &[]), Val::S64(305));
}

#[test]

fn tuple_export_takes_the_retptr_fast_path() {
    let Some(mut c) = composed() else { return };
    let got = ok(&mut c, "fwd", &[Val::S32(2), Val::S32(3)]);
    let Val::Tuple(items) = got else {
        panic!("fwd should return a tuple, got {got:?}");
    };
    assert_eq!(items, vec![Val::S32(2), Val::S32(3)]);
}

#[test]

fn eq_over_canonical_tuples_is_structural() {
    let Some(mut c) = composed() else { return };
    assert_eq!(
        ok(
            &mut c,
            "t-eq",
            &[Val::S32(1), Val::S32(2), Val::S32(1), Val::S32(2)]
        ),
        Val::Bool(true)
    );
    assert_eq!(
        ok(
            &mut c,
            "t-eq",
            &[Val::S32(1), Val::S32(2), Val::S32(1), Val::S32(3)]
        ),
        Val::Bool(false)
    );
}
