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
    let dir = std::env::temp_dir().join(format!("wavelet-memlst-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let dep = r#"Package "demo:lst@0.1.0"

DefType point {x: s32 y: s32}
DefType wrapper {data: list(s32) tag: string}

Export {name: nums params: {} result: list(s32)}
Def nums Fn {} [1 2 3]

Export {name: pts params: {} result: list(point)}
Def pts Fn {} [{x: 1 y: 2} {x: 3 y: 4}]

Export {name: strs params: {} result: list(string)}
Def strs Fn {} ["a" "bee"]

Export {name: gets params: {} result: string}
Def gets Fn {} "hola"

Export {name: wrap params: {} result: wrapper}
Def wrap Fn {} {data: [7 8] tag: "w"}
"#;

    let main = r#"Package "demo:main@0.1.0"

Import {pkg: "demo:lst/api" as: lv}

Export {name: sum params: {} result: s64}
Def sum Fn {}
  Match lv/nums() [([a b c] add(a add(b c))) (other 0)]

Export {name: fall params: {} result: s64}
Def fall Fn {}
  Match lv/nums() [([a b] 0) ([a b c] 1) (other 2)]

Export {name: deep params: {} result: s64}
Def deep Fn {}
  Match lv/pts() [([{x: x1} {y: y2}] add(x1 y2)) (other 0)]

Export {name: same params: {} result: bool}
Def same Fn {}
  Let {t: lv/nums()} eq(t [1 2 3])

Export {name: cat params: {} result: string}
Def cat Fn {}
  Match lv/strs() [([a b] str-cat(a b)) (other "no")]

Export {name: shout params: {} result: string}
Def shout Fn {}
  Let {s: lv/gets()} str-cat(s "!")

Export {name: fwd params: {} result: list(s32)}
Def fwd Fn {} lv/nums()

Export {name: fwds params: {} result: string}
Def fwds Fn {} lv/gets()

Export {name: field params: {} result: s64}
Def field Fn {}
  Match lv/wrap() [({data: [a b]} add(a b)) (other 0)]

Export {name: count params: {} result: s64}
Def count Fn {} len(lv/nums())

Export {name: first params: {} result: s64}
Def first Fn {} head(lv/nums())

Export {name: firstpt params: {} result: s64}
Def firstpt Fn {} Match head(lv/pts()) [({x: xx} xx) (other 0)]

Export {name: count2 params: {} result: s64}
Def count2 Fn {} len(tail(lv/nums()))

Export {name: firsttail params: {} result: s64}
Def firsttail Fn {} head(tail(lv/nums()))

Export {name: rest params: {} result: list(s32)}
Def rest Fn {} tail(lv/nums())

Export {name: mtail params: {} result: s64}
Def mtail Fn {} Match tail(lv/nums()) [([a b] add(a b)) (other -1)]
"#;

    std::fs::write(src.join("lst.wlt"), dep).unwrap();
    std::fs::write(src.join("main.wlt"), main).unwrap();
    let out = dir.join("out");
    let sources = vec![
        src.join("lst.wlt").to_str().unwrap().to_string(),
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

fn dep_born_list_destructures_by_stride() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "sum", &[]), Val::S64(6));
}

#[test]

fn list_pattern_length_mismatch_falls_through() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "fall", &[]), Val::S64(1));
}

#[test]

fn records_inside_canonical_list_destructure_in_place() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "deep", &[]), Val::S64(5));
}

#[test]

fn canonical_list_rebuild_is_faithful() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "same", &[]), Val::Bool(true));
}

#[test]

fn strings_inside_canonical_list_bind_and_rebox() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "cat", &[]), Val::String("abee".into()));
}

#[test]

fn dep_born_string_reboxes_at_the_seam() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "shout", &[]), Val::String("hola!".into()));
}

#[test]

fn list_and_string_exports_take_the_retptr_fast_path() {
    let Some(mut c) = composed() else { return };
    assert_eq!(
        ok(&mut c, "fwd", &[]),
        Val::List(vec![Val::S32(1), Val::S32(2), Val::S32(3)])
    );
    assert_eq!(ok(&mut c, "fwds", &[]), Val::String("hola".into()));
}

#[test]

fn list_field_inside_canonical_record_destructures() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "field", &[]), Val::S64(15));
}

#[test]

fn len_over_canonical_list_reads_the_length_word() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "count", &[]), Val::S64(3));
}

#[test]

fn head_over_canonical_list_loads_the_first_element() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "first", &[]), Val::S64(1));
    assert_eq!(ok(&mut c, "firstpt", &[]), Val::S64(1));
}

fn constructed() -> HostComponent {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-memlstc-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let app = r#"Package "demo:app@0.1.0"

Export {name: mk params: {a: s32 b: s32} result: list(s32)}
Def mk Fn {a: s32 b: s32} Let {t: [a b]} t

Export {name: c-same params: {} result: bool}
Def c-same Fn {} Let {t: [1 2 3]} eq(t [1 2 3])

Export {name: c-diff params: {} result: bool}
Def c-diff Fn {} Let {t: [1 2 3]} eq(t [3 2 1])

Export {name: c-pts params: {} result: bool}
Def c-pts Fn {}
  Let {t: [{x: 1 y: 2} {x: 3 y: 4}]} eq(t [{x: 1 y: 2} {x: 3 y: 4}])

Export {name: c-nested params: {} result: s64}
Def c-nested Fn {}
  Let {t: [[1 2] [3]]}
    Match t [([[a b] [c]] add(a add(b c))) (other 0)]

Export {name: c-strs params: {} result: string}
Def c-strs Fn {}
  Let {t: ["x" "y"]} Match t [([a b] str-cat(a b)) (other "no")]

Export {name: c-rec-field params: {} result: bool}
Def c-rec-field Fn {}
  Let {r: {data: [7 8] tag: "w"}}
    eq(r {data: [7 8] tag: "w"})
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();
    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the canonical-lists app");
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

fn constructed_list_exports_through_the_fast_path() {
    let mut c = constructed();
    assert_eq!(
        okc(&mut c, "mk", &[Val::S32(4), Val::S32(9)]),
        Val::List(vec![Val::S32(4), Val::S32(9)])
    );
}

#[test]

fn constructed_list_rebuild_preserves_elements() {
    let mut c = constructed();
    assert_eq!(okc(&mut c, "c-same", &[]), Val::Bool(true));
    assert_eq!(okc(&mut c, "c-diff", &[]), Val::Bool(false));
}

#[test]

fn record_elements_pack_and_rebuild_faithfully() {
    let mut c = constructed();
    assert_eq!(okc(&mut c, "c-pts", &[]), Val::Bool(true));
}

#[test]

fn nested_list_literals_pack_and_destructure() {
    let mut c = constructed();
    assert_eq!(okc(&mut c, "c-nested", &[]), Val::S64(6));
}

#[test]

fn string_elements_store_and_rebox() {
    let mut c = constructed();
    assert_eq!(okc(&mut c, "c-strs", &[]), Val::String("xy".into()));
}

#[test]

fn list_field_inside_record_literal_constructs() {
    let mut c = constructed();
    assert_eq!(okc(&mut c, "c-rec-field", &[]), Val::Bool(true));
}

#[test]

fn tail_over_canonical_list_is_zero_copy() {
    let Some(mut c) = composed() else { return };

    assert_eq!(ok(&mut c, "count2", &[]), Val::S64(2));
    assert_eq!(ok(&mut c, "firsttail", &[]), Val::S64(2));
    assert_eq!(ok(&mut c, "mtail", &[]), Val::S64(5));
    assert_eq!(
        ok(&mut c, "rest", &[]),
        Val::List(vec![Val::S32(2), Val::S32(3)])
    );
}
