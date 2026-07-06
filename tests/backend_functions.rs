//! Goal 5 (5.8 defunctionalization, zero-capture case): a `Let` binding
//! initialised with a bare module-level def reference whose checked type is a
//! concrete arrow is statically known to hold exactly that function value, so
//! an apply through the binding compiles to a DIRECT call (the same path a
//! direct `inc(…)` call takes) instead of bundling a payload box and calling
//! through the TAG_FN funcref table. Every non-apply use of the binding still
//! reads the boxed closure value, and shadowing/rebinding fall back to the
//! indirect path — results must equal the oracle's in all cases.

use wavelet::host::{HostComponent, Val};

fn built() -> HostComponent {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-fns-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let app = r#"Package "demo:fns@0.1.0"

Def inc Fn {n: s32} add(n 1)
Def double Fn {n: s32} mul(n 2)

Export {name: direct params: {} result: s64}
Def direct Fn {} Let {f: inc} f(41)

Export {name: shadow params: {} result: s64}
Def shadow Fn {}
  Let {f: inc}
    Let {f: double} f(10)

Export {name: reborrow params: {} result: s64}
Def reborrow Fn {}
  Let {f: inc}
    Let {g: f} g(6)

Export {name: lam params: {} result: s64}
Def lam Fn {}
  Let {g: Fn {n: s32} add(n 100)} g(1)
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();
    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the function-values app");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);
    HostComponent::from_bytes(&bytes).expect("instantiate")
}

const IFACE: &str = "demo:fns/api@0.1.0";

fn ok(c: &mut HostComponent, f: &str) -> Val {
    c.call_instance(IFACE, f, &[])
        .unwrap_or_else(|e| panic!("`{f}` should succeed: {e}"))[0]
        .clone()
}

/// The devirtualized apply (`Let {f: inc} f(41)`), shadowing to another known
/// def, re-binding through a local (indirect boxed path), and a Fn-literal
/// binding (indirect path) all agree with the oracle.
#[test]
fn known_def_bindings_apply_like_the_oracle() {
    let mut c = built();
    assert_eq!(ok(&mut c, "direct"), Val::S64(42));
    assert_eq!(ok(&mut c, "shadow"), Val::S64(20));
    assert_eq!(ok(&mut c, "reborrow"), Val::S64(7));
    assert_eq!(ok(&mut c, "lam"), Val::S64(101));
}

/// Oracle cross-check: the same bodies as snippets in the interpreter.
#[test]
fn oracle_agrees() {
    for (src, want) in [
        ("Def inc Fn {n: s32} add(n 1)\nLet {f: inc} f(41)", "42"),
        (
            "Def inc Fn {n: s32} add(n 1)\nDef double Fn {n: s32} mul(n 2)\nLet {f: inc} Let {f: double} f(10)",
            "20",
        ),
        (
            "Def inc Fn {n: s32} add(n 1)\nLet {f: inc} Let {g: f} g(6)",
            "7",
        ),
        ("Let {g: Fn {n: s32} add(n 100)} g(1)", "101"),
    ] {
        let r = wavelet::eval_snippet(src);
        assert!(r.ok, "{src}: {}", r.error);
        assert_eq!(r.value, want, "{src}");
    }
}
