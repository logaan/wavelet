//! 4.2 — payload-less results are spellable and cross the boundary.
//!
//! `result`, `result(t)`, and `result(_ e)` type text (the `_` placeholder
//! lexes); `ok()` / `err()` construct genuinely payload-less cases (no unit
//! payload — the unit value is gone, 2.4); the `(ok)` / `(err)` call-shaped
//! patterns destructure them. Lowering these at a bare-result boundary used to
//! ICE the compiler ("never a single flat value").

use wavelet::eval_snippet;
use wavelet::host::{HostComponent, Val};

#[test]
fn payloadless_results_evaluate() {
    let out = eval_snippet("Match ok() [((ok) \"o\") ((err) \"e\")]");
    assert!(out.ok, "{}", out.error);
    assert_eq!(out.value, "\"o\"");
    let out = eval_snippet("eq(err() err())");
    assert!(out.ok, "{}", out.error);
    assert_eq!(out.value, "true");
}

#[test]
fn absent_arms_synthesize_wit() {
    let src = "Package \"demo:t@0.1.0\"\n\
               Export {name: check params: {v: result} result: result(_ list(string))}\n\
               Def check Fn {v} Match v [((ok) err([\"flipped\"])) ((err e) err(e))]";
    let (arena, roots) = wavelet::reader::read_file(src).unwrap();
    let wit = wavelet::wit::synthesize(&arena, &roots).unwrap();
    assert!(
        wit.contains("check: func(v: result) -> result<_, list<string>>;"),
        "{wit}"
    );
}

fn results_component() -> HostComponent {
    let dir = std::env::temp_dir().join(format!("wvl-plr-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let app = r#"Package "demo:plr@0.1.0"

// Bare result: flip ok <-> err (the roundtrip:suite result-rt transform).
Export {name: flip params: {v: result} result: result}
Def flip Fn {v}
  Match v [((ok) err()) ((err) ok())]

// result<u32>: bump the ok payload, keep the payload-less err.
Export {name: bump params: {v: result(u32)} result: result(u32)}
Def bump Fn {v}
  Match v [((ok n) ok(add(n 1))) ((err) err())]

// result<_, string>: keep the payload-less ok, transform the err payload.
Export {name: tag-err params: {v: result(_ string)} result: result(_ string)}
Def tag-err Fn {v}
  Match v [((ok) ok()) ((err e) err(str-cat(e "!")))]
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();
    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the payload-less results component");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);
    HostComponent::from_bytes(&bytes).expect("instantiate")
}

const IFACE: &str = "demo:plr/api@0.1.0";

fn res(v: Result<Option<Val>, Option<Val>>) -> Val {
    match v {
        Ok(p) => Val::Result(Ok(p.map(Box::new))),
        Err(p) => Val::Result(Err(p.map(Box::new))),
    }
}

#[test]
fn payloadless_results_cross_the_boundary() {
    let mut c = results_component();

    let out = c
        .call_instance(IFACE, "flip", &[res(Ok(None))])
        .expect("flip(ok)");
    assert_eq!(out[0], res(Err(None)));
    let out = c
        .call_instance(IFACE, "flip", &[res(Err(None))])
        .expect("flip(err)");
    assert_eq!(out[0], res(Ok(None)));

    let out = c
        .call_instance(IFACE, "bump", &[res(Ok(Some(Val::U32(7))))])
        .expect("bump(ok(7))");
    assert_eq!(out[0], res(Ok(Some(Val::U32(8)))));
    let out = c
        .call_instance(IFACE, "bump", &[res(Err(None))])
        .expect("bump(err)");
    assert_eq!(out[0], res(Err(None)));

    let out = c
        .call_instance(IFACE, "tag-err", &[res(Ok(None))])
        .expect("tag-err(ok)");
    assert_eq!(out[0], res(Ok(None)));
    let out = c
        .call_instance(
            IFACE,
            "tag-err",
            &[res(Err(Some(Val::String("bad".into()))))],
        )
        .expect("tag-err(err(bad))");
    assert_eq!(out[0], res(Err(Some(Val::String("bad!".into())))));
}
