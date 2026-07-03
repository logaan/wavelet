//! Goal 5 (5.5 canonical lists/strings, dep-born slice): a dep call
//! returning a list or string is BORN canonical — the import's retptr area
//! already carries the canonical (ptr, len) pair over packed elements /
//! raw UTF-8 — and Let/Match consume it without a box lift. List patterns
//! destructure packed elements at stride offsets (the length check is a
//! runtime comparison, unlike a tuple's static arity); every boxed seam
//! rebuilds exactly the interpreter's value.

use wavelet::host::{HostComponent, Val};

/// Is `bin` runnable (`<bin> --version` succeeds)?
fn have(bin: &str) -> bool {
    std::process::Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Build the two-component project (a list/string-producing dep + a
/// consumer) and instantiate the composed app. `None` without `wac`.
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
// A list pattern over a dep-born canonical list destructures packed
// elements at stride offsets: scalar binders load typed locals directly
// from the element buffer, no boxes.
fn dep_born_list_destructures_by_stride() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "sum", &[]), Val::S64(6));
}

#[test]
// The list-length check is a runtime comparison (a value property): the
// wrong-arity clause falls through, the right one matches — like the
// oracle.
fn list_pattern_length_mismatch_falls_through() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "fall", &[]), Val::S64(1));
}

#[test]
// Records packed inside a canonical list destructure in place: each
// element's record pattern takes an interior pointer at its stride offset.
fn records_inside_canonical_list_destructure_in_place() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "deep", &[]), Val::S64(5));
}

#[test]
// A Let-bound dep-born list reboxes faithfully at the eq seam: the rebuilt
// box is exactly the interpreter's Value::Lst (element order, Int domain).
fn canonical_list_rebuild_is_faithful() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "same", &[]), Val::Bool(true));
}

#[test]
// Strings packed inside a canonical list bind lazily (interior (ptr, len)
// pointers) and rebox only where consumed.
fn strings_inside_canonical_list_bind_and_rebox() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "cat", &[]), Val::String("abee".into()));
}

#[test]
// A dep-born string is born canonical (raw UTF-8 behind (ptr, len)) and
// reboxes only at the consuming seam.
fn dep_born_string_reboxes_at_the_seam() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "shout", &[]), Val::String("hola!".into()));
}

#[test]
// A def forwarding a dep list/string carries a Mem result signature and
// the export wrapper returns the dep's canonical area directly (the 5.5
// retptr fast path): two boundary crossings, zero boxes.
fn list_and_string_exports_take_the_retptr_fast_path() {
    let Some(mut c) = composed() else { return };
    assert_eq!(
        ok(&mut c, "fwd", &[]),
        Val::List(vec![Val::S32(1), Val::S32(2), Val::S32(3)])
    );
    assert_eq!(ok(&mut c, "fwds", &[]), Val::String("hola".into()));
}

#[test]
// A list field inside a dep-born canonical record binds as an interior
// (ptr, len) pointer and destructures by stride from there.
fn list_field_inside_canonical_record_destructures() {
    let Some(mut c) = composed() else { return };
    assert_eq!(ok(&mut c, "field", &[]), Val::S64(15));
}
