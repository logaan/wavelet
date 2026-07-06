//! Goal 5 (5.12): the backend `map` builtin. `map(f, list)` applies the
//! function value `f` to each element in source order and collects the results
//! in a length-preserving list, driving `f` through the boxed-closure
//! convention (call_indirect on the funcref table). Bare def references and
//! inline `Fn` literals are both usable as `f`; results equal the oracle's.

use wavelet::host::{HostComponent, Val};

fn built() -> HostComponent {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-mp-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let app = r#"Package "demo:mp@0.1.0"

Def inc Fn {n: s32} add(n 1)
Def bump Fn {s} str-cat(s "!")

Export {name: mapinc params: {} result: list(s32)}
Def mapinc Fn {} The list(s32) map(inc [1 2 3])

Export {name: maplam params: {} result: list(s32)}
Def maplam Fn {} The list(s32) map(Fn {n: s32} mul(n 2) [1 2 3])

Export {name: mapstr params: {} result: list(string)}
Def mapstr Fn {} The list(string) map(bump ["a" "bee"])

Export {name: mapempty params: {} result: list(s32)}
Def mapempty Fn {} The list(s32) map(inc [])

Export {name: mapnest params: {} result: list(list(s32))}
Def mapnest Fn {} The list(list(s32)) map(Fn {xs} map(inc xs) [[1 2] [3]])

Export {name: sumf params: {} result: s64}
Def sumf Fn {} fold(Fn {acc x} add(acc x) 0 [1 2 3 4 5])

Export {name: foldempty params: {} result: s64}
Def foldempty Fn {} fold(Fn {acc x} add(acc x) 7 [])
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();
    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the map app");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);
    HostComponent::from_bytes(&bytes).expect("instantiate")
}

const IFACE: &str = "demo:mp/api@0.1.0";

fn ok(c: &mut HostComponent, f: &str) -> Val {
    c.call_instance(IFACE, f, &[])
        .unwrap_or_else(|e| panic!("`{f}` should succeed: {e}"))[0]
        .clone()
}

#[test]
fn map_over_component_exports() {
    let mut c = built();
    assert_eq!(
        ok(&mut c, "mapinc"),
        Val::List(vec![Val::S32(2), Val::S32(3), Val::S32(4)])
    );
    assert_eq!(
        ok(&mut c, "maplam"),
        Val::List(vec![Val::S32(2), Val::S32(4), Val::S32(6)])
    );
    assert_eq!(
        ok(&mut c, "mapstr"),
        Val::List(vec![
            Val::String("a!".into()),
            Val::String("bee!".into())
        ])
    );
    assert_eq!(ok(&mut c, "mapempty"), Val::List(vec![]));
    assert_eq!(
        ok(&mut c, "mapnest"),
        Val::List(vec![
            Val::List(vec![Val::S32(2), Val::S32(3)]),
            Val::List(vec![Val::S32(4)]),
        ])
    );
    assert_eq!(ok(&mut c, "sumf"), Val::S64(15));
    assert_eq!(ok(&mut c, "foldempty"), Val::S64(7));
}

/// Oracle cross-check: the same bodies as interpreter snippets.
#[test]
fn oracle_agrees() {
    for (src, want) in [
        ("Def inc Fn {n: s32} add(n 1)\nmap(inc [1 2 3])", "[2, 3, 4]"),
        ("map(Fn {n: s32} mul(n 2) [1 2 3])", "[2, 4, 6]"),
        (
            "Def bump Fn {s} str-cat(s \"!\")\nmap(bump [\"a\" \"bee\"])",
            "[\"a!\", \"bee!\"]",
        ),
        ("Def inc Fn {n: s32} add(n 1)\nmap(inc [])", "[]"),
        (
            "Def inc Fn {n: s32} add(n 1)\nmap(Fn {xs} map(inc xs) [[1 2] [3]])",
            "[[2, 3], [4]]",
        ),
        ("fold(Fn {acc x} add(acc x) 0 [1 2 3 4 5])", "15"),
        ("fold(Fn {acc x} mul(acc x) 1 [1 2 3 4])", "24"),
        ("fold(Fn {acc x} add(acc x) 7 [])", "7"),
    ] {
        let r = wavelet::eval_snippet(src);
        assert!(r.ok, "{src}: {}", r.error);
        assert_eq!(r.value, want, "{src}");
    }
}
