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
    let dir = std::env::temp_dir().join(format!("wavelet-memvar-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let dep = r#"Package "demo:vr@0.1.0"

DefType pt {x: s32 y: s32}
DefType shape [circle(f64) square(f64) empty]
DefType wrapped [pt-case(pt) two(tuple(s32 s32)) nothing]

Export {name: pick params: {n: s32} result: shape}
Def pick Fn {n: s32}
  Match n [(0 circle(1.5)) (1 square(2.0)) (other empty)]

Export {name: find params: {b: bool} result: option(s32)}
Def find Fn {b: bool} If b some(7) none

Export {name: parse params: {b: bool} result: result(s32 string)}
Def parse Fn {b: bool} If b ok(3) err("bad")

Export {name: wrap params: {} result: wrapped}
Def wrap Fn {} pt-case({x: 3 y: 4})

Export {name: pair params: {} result: wrapped}
Def pair Fn {} two(tuple2(4 9))

Export {name: opt params: {n: s32} result: option(s32)}
Def opt Fn {n: s32} some(n)

DefType boxed {tag: s32 val: option(s32)}

Export {name: mkbox params: {b: bool} result: boxed}
Def mkbox Fn {b: bool} If b {tag: 1 val: some(5)} {tag: 0 val: none}
"#;

    let main = r#"Package "demo:main@0.1.0"

Import {pkg: "demo:vr/api" as: vr}

Export {name: area params: {n: s32} result: f64}
Def area Fn {n: s32}
  Match vr/pick(n)
    [((circle r) mul(r r)) ((square s) add(s s)) ((empty) 0.0) (other -1.0)]

Export {name: get params: {b: bool} result: s64}
Def get Fn {b: bool}
  Match vr/find(b) [((some x) x) (none -1)]

Export {name: chk params: {b: bool} result: string}
Def chk Fn {b: bool}
  Match vr/parse(b) [((ok n) to-string(n)) ((err m) m)]

Export {name: deep params: {} result: s64}
Def deep Fn {}
  Match vr/wrap() [((pt-case {x: xx}) xx) (other 0)]

Export {name: spread params: {} result: s64}
Def spread Fn {}
  Match vr/pair() [((two a b) add(a b)) (other 0)]

Export {name: fall params: {} result: s64}
Def fall Fn {}
  Match vr/pair() [((nothing) 0) ((pt-case p) 1) ((two a) 2) ((two a b) add(a b))]

Export {name: same params: {} result: bool}
Def same Fn {}
  Let {v: vr/find(true)} eq(v some(7))

Export {name: fwd params: {b: bool} result: option(s32)}
Def fwd Fn {b: bool} vr/find(b)

Export {name: v-eq params: {a: s32 b: s32} result: bool}
Def v-eq Fn {a: s32 b: s32} eq(vr/opt(a) vr/opt(b))

Export {name: v-eq-disc params: {b: bool} result: bool}
Def v-eq-disc Fn {b: bool} eq(vr/find(b) vr/opt(7))

Export {name: unbox params: {b: bool} result: s64}
Def unbox Fn {b: bool}
  Match vr/mkbox(b) [({val: none} -1) ({val: (some x)} x)]
"#;

    std::fs::write(src.join("vr.wlt"), dep).unwrap();
    std::fs::write(src.join("main.wlt"), main).unwrap();
    let out = dir.join("out");
    let sources = vec![
        src.join("vr.wlt").to_str().unwrap().to_string(),
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

fn variant_cases_select_by_discriminant() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "area", &[Val::S32(0)]), Val::Float64(2.25));
    assert_eq!(ok(&mut c, "area", &[Val::S32(1)]), Val::Float64(4.0));
    assert_eq!(ok(&mut c, "area", &[Val::S32(9)]), Val::Float64(0.0));
}

#[test]

fn option_cases_match_as_expected() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "get", &[Val::Bool(true)]), Val::S64(7));
    assert_eq!(ok(&mut c, "get", &[Val::Bool(false)]), Val::S64(-1));
}

#[test]

fn result_cases_match_as_expected() {
    let Some(mut c) = composed() else { return };
    assert_eq!(
        ok(&mut c, "chk", &[Val::Bool(true)]),
        Val::String("3".into())
    );
    assert_eq!(
        ok(&mut c, "chk", &[Val::Bool(false)]),
        Val::String("bad".into())
    );
}

#[test]

fn record_payload_destructures_in_place() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "deep", &[]), Val::S64(3));
}

#[test]

fn tuple_payload_spreads_across_sub_patterns() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "spread", &[]), Val::S64(13));
}

#[test]

fn case_mismatches_fall_through_and_one_binder_takes_the_payload() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "fall", &[]), Val::S64(2));
}

#[test]

fn canonical_variant_rebuild_is_faithful() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "same", &[]), Val::Bool(true));
}

#[test]

fn option_export_takes_the_retptr_fast_path() {
    let Some(mut c) = composed() else { return };
    assert_eq!(
        ok(&mut c, "fwd", &[Val::Bool(true)]),
        Val::Option(Some(Box::new(Val::S32(7))))
    );
    assert_eq!(ok(&mut c, "fwd", &[Val::Bool(false)]), Val::Option(None));
}

#[test]

fn nested_nullary_field_pattern_matches_as_expected() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "unbox", &[Val::Bool(true)]), Val::S64(5));
    assert_eq!(ok(&mut c, "unbox", &[Val::Bool(false)]), Val::S64(-1));
}

fn constructed() -> HostComponent {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-memvarc-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let app = r#"Package "demo:app@0.1.0"

DefType shape [circle(f64) square(f64) empty]

Export {name: c-var params: {r: f64} result: f64}
Def c-var Fn {r: f64}
  Let {s: circle(r)}
    Match s [((circle x) x) ((empty) 0.0) (other -1.0)]

Export {name: c-some params: {v: s32} result: option(s32)}
Def c-some Fn {v: s32} some(v)

Export {name: c-okpair params: {a: s32 b: s32} result: result(tuple(s32 s32) string)}
Def c-okpair Fn {a: s32 b: s32} ok(a b)

Export {name: c-err params: {m: string} result: result(s32 string)}
Def c-err Fn {m: string} err(m)

Export {name: c-eq params: {} result: bool}
Def c-eq Fn {} Let {v: some(7)} eq(v some(7))

Export {name: c-field params: {} result: bool}
Def c-field Fn {}
  Let {r: {s: some(3) t: "x"}}
    eq(r {s: some(3) t: "x"})

Export {name: c-shadow params: {} result: s64}
Def c-shadow Fn {}
  Let {ok: Fn {x: s64} some(x)}
    Match ok(5) [((some x) x) (other -1)]
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();
    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the canonical-variants app");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);
    HostComponent::from_bytes(&bytes).expect("instantiate")
}

const APP_IFACE: &str = "demo:app/api@0.1.0";

fn okc(c: &mut HostComponent, f: &str, args: &[Val]) -> Val {
    c.call_instance(APP_IFACE, f, args)
        .unwrap_or_else(|e| panic!("`{f}` should succeed: {e}"))[0]
        .clone()
}

#[test]

fn local_case_constructor_builds_canonically() {
    let mut c = constructed();
    assert_eq!(
        okc(&mut c, "c-var", &[Val::Float64(2.5)]),
        Val::Float64(2.5)
    );
}

#[test]

fn some_constructor_exports_through_the_fast_path() {
    let mut c = constructed();
    assert_eq!(
        okc(&mut c, "c-some", &[Val::S32(3)]),
        Val::Option(Some(Box::new(Val::S32(3))))
    );
}

#[test]

fn ok_bundles_arguments_into_a_tuple_payload() {
    let mut c = constructed();
    let got = okc(&mut c, "c-okpair", &[Val::S32(4), Val::S32(9)]);
    let Val::Result(Ok(Some(inner))) = got else {
        panic!("c-okpair should return ok(tuple), got {got:?}");
    };
    assert_eq!(*inner, Val::Tuple(vec![Val::S32(4), Val::S32(9)]));
}

#[test]

fn err_string_payload_stores_canonically() {
    let mut c = constructed();
    let got = okc(&mut c, "c-err", &[Val::String("bad".into())]);
    let Val::Result(Err(Some(inner))) = got else {
        panic!("c-err should return err(string), got {got:?}");
    };
    assert_eq!(*inner, Val::String("bad".into()));
}

#[test]

fn constructed_variant_rebuild_is_faithful() {
    let mut c = constructed();
    assert_eq!(okc(&mut c, "c-eq", &[]), Val::Bool(true));
    assert_eq!(okc(&mut c, "c-field", &[]), Val::Bool(true));
}

#[test]

fn local_binding_shadows_constructor_names() {
    let mut c = constructed();
    assert_eq!(okc(&mut c, "c-shadow", &[]), Val::S64(5));
}

#[test]

fn eq_over_canonical_variants_is_structural() {
    let Some(mut c) = composed() else { return };

    assert_eq!(
        ok(&mut c, "v-eq", &[Val::S32(5), Val::S32(5)]),
        Val::Bool(true)
    );

    assert_eq!(
        ok(&mut c, "v-eq", &[Val::S32(1), Val::S32(2)]),
        Val::Bool(false)
    );

    assert_eq!(ok(&mut c, "v-eq-disc", &[Val::Bool(true)]), Val::Bool(true));

    assert_eq!(
        ok(&mut c, "v-eq-disc", &[Val::Bool(false)]),
        Val::Bool(false)
    );
}
