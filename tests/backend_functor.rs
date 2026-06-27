//! Backend ↔ interpreter parity for the `Set` functor's *build* path (step 04).
//!
//! `CLAUDE.md` makes the interpreter the semantics oracle: "a wasm-backend change
//! that diverges from the interpreter is a bug." Steps 01–03 emitted the `set`
//! resource and got a validating component; step 04 routes the qualified
//! `alias/op` calls (`pts/new`, `pts/add`, `pts/contains`, `pts/size`) to those
//! emitted core funcs and lifts/lowers `own<set>` handles at the boundary. These
//! tests build a functor program **through the real emitter** and then *execute*
//! it in-process (the capability-free `wasmtime` host), asserting the backend
//! agrees with the interpreter on the same program.
//!
//! Two complementary shapes are covered here (step 05 formalises the full suite):
//!   * an export that *derives an ordinary result* (`u32`) from the set, over a
//!     derived **record** element — exercising intra-guest routing of new/add/size;
//!   * an export that *returns the `own<set>` handle* over a **primitive** element
//!     (`s32`) — exercising the handle return boundary plus host-side method calls
//!     on the returned resource. (A handle return over a *local record* element is
//!     a known WIT interface-cycle limitation; see `functor_runtime.rs`.)

use wavelet::host::{HostComponent, Val};

/// Build one `.wvl` source through the real `wavelet build` path in a throwaway
/// dir, returning the bytes of the single output component.
///
/// The per-call dir is keyed on `(pid, seq)`, not pid alone: two concurrent
/// builds in the same process otherwise `remove_dir_all` each other mid-flight
/// under a loaded `cargo test` (the bug `backend_numeric.rs` documents).
fn build_component(src_text: &str) -> HostComponent {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-functor-be-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let path = src.join("app.wvl");
    std::fs::write(&path, src_text).unwrap();
    let out = dir.join("out");
    let bytes = wavelet::build::build_files(
        &[path.to_string_lossy().into_owned()],
        &out.to_string_lossy(),
    )
    .map(|outputs| std::fs::read(&outputs[0]).expect("read built component"))
    .expect("functor program should build");
    let _ = std::fs::remove_dir_all(&dir);
    HostComponent::from_bytes(&bytes).expect("instantiate the functor component")
}

/// The single returned value of an export call. A Wavelet `Export <name>` lands
/// in the package's `api` interface (e.g. `demo:geo/api@0.1.0`), so it is called
/// through that instance, not as a world-level function.
fn one(c: &mut HostComponent, iface: &str, name: &str, args: &[Val]) -> Val {
    let out = c
        .call_instance(iface, name, args)
        .unwrap_or_else(|e| panic!("`{iface}#{name}` should run: {e}"));
    assert_eq!(out.len(), 1, "`{name}` should return one value, got {out:?}");
    out.into_iter().next().unwrap()
}

#[test]
// Intra-guest routing over a *record* element, deriving an ordinary `u32`.
// `count-distinct` builds a `point` set, adds three points (one a structural
// duplicate), and returns `pts/size(s)`. The backend must dedup by structural
// `Value` equality (`eq_raw`) exactly as the interpreter does, so the result is
// 2 — the same answer `wavelet run` gives for these adds (see the
// `element_equality_matches_eq_for_records` oracle test in `functor_runtime.rs`).
fn routed_record_set_size_matches_interpreter() {
    const SRC: &str = r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Derive {Eq Ord Show} point
Import {pkg: "wavelet:coll/set" elem: point as: pts}
Export count-distinct
Def count-distinct Fn {}
  Let {s: pts/new()}
    Do [ pts/add(s {x: 1 y: 2})
         pts/add(s {x: 3 y: 4})
         pts/add(s {x: 1 y: 2})
         pts/size(s) ]"#;

    let mut c = build_component(SRC);
    // Interpreter oracle: {(1,2),(3,4),(1,2)} dedups to 2 distinct points.
    assert_eq!(
        one(&mut c, "demo:geo/api@0.1.0", "count-distinct", &[]),
        Val::U32(2)
    );
}

#[test]
// The handle return boundary + host-side method calls, over a *primitive* (`s32`)
// element (no local-record WIT cycle). `build-ints` adds 1, 2, 1 (a duplicate)
// and *returns the set handle* (`own<set>`); the host then calls `size`/`contains`
// on the returned resource. The deduped size is 2 and membership is exact —
// matching the interpreter's `set-add`/`size`/`contains` (see `functor_runtime.rs`
// `add_is_observed_on_the_same_handle_and_dedups`).
fn returned_handle_methods_match_interpreter() {
    const SRC: &str = r#"Package "demo:app@0.1.0"
Import {pkg: "wavelet:coll/set" elem: s32 as: ints}
Export build-ints
Def build-ints Fn {}
  Let {s: ints/new()}
    Do [ ints/add(s 1)
         ints/add(s 2)
         ints/add(s 1)
         s ]"#;
    const IFACE: &str = "demo:app/s32-set@0.1.0";
    const API: &str = "demo:app/api@0.1.0";

    let mut c = build_component(SRC);
    let handle = match one(&mut c, API, "build-ints", &[]) {
        v @ Val::Resource(_) => v,
        other => panic!("`build-ints` should return a set resource, got {other:?}"),
    };

    // size(self): deduped count is 2.
    let size = c
        .call_instance(IFACE, "[method]set.size", &[handle.clone()])
        .expect("size call should succeed");
    assert_eq!(size, vec![Val::U32(2)], "deduped size should be 2");

    // contains(self, value): 1 and 2 present, 9 absent.
    let has = |c: &mut HostComponent, v: i32| {
        c.call_instance(IFACE, "[method]set.contains", &[handle.clone(), Val::S32(v)])
            .expect("contains call should succeed")
    };
    assert_eq!(has(&mut c, 1), vec![Val::Bool(true)], "1 should be present");
    assert_eq!(has(&mut c, 2), vec![Val::Bool(true)], "2 should be present");
    assert_eq!(has(&mut c, 9), vec![Val::Bool(false)], "9 should be absent");

    // Drop the returned own<set> handle host-side; the no-op dtor runs cleanly.
    c.drop_resource(handle)
        .expect("dropping the returned set handle should run the dtor cleanly");
}

#[test]
// TWO functor instantiations in ONE world, both returning handles. Exercises the
// `set as <iface>-handle` aliasing fix (commit 0444f6a): the two `set` resources
// must land in distinct interfaces (`s32-set`, `string-set`) without colliding.
// `build-ints` adds 1 twice (-> size 1, the "same element twice" edge);
// `build-words` adds "a","b","a" (-> size 2). Both built once, called on the
// same component, matching the interpreter.
fn two_instantiations_in_one_world_match_interpreter() {
    const SRC: &str = r#"Package "demo:multi@0.1.0"
Import {pkg: "wavelet:coll/set" elem: s32 as: ints}
Import {pkg: "wavelet:coll/set" elem: string as: words}
Export build-ints
Def build-ints Fn {}
  Let {s: ints/new()}
    Do [ ints/add(s 1) ints/add(s 1) s ]
Export build-words
Def build-words Fn {}
  Let {s: words/new()}
    Do [ words/add(s "a") words/add(s "b") words/add(s "a") s ]"#;
    const API: &str = "demo:multi/api@0.1.0";
    const INTS_IFACE: &str = "demo:multi/s32-set@0.1.0";
    const WORDS_IFACE: &str = "demo:multi/string-set@0.1.0";

    let mut c = build_component(SRC);

    // build-ints: 1 added twice dedups to size 1.
    let ints = match one(&mut c, API, "build-ints", &[]) {
        v @ Val::Resource(_) => v,
        other => panic!("`build-ints` should return a set resource, got {other:?}"),
    };
    let ints_size = c
        .call_instance(INTS_IFACE, "[method]set.size", &[ints.clone()])
        .expect("ints size call should succeed");
    assert_eq!(ints_size, vec![Val::U32(1)], "1 added twice dedups to 1");
    c.drop_resource(ints)
        .expect("dropping the ints set handle should run the dtor cleanly");

    // build-words: "a","b","a" dedups to size 2.
    let words = match one(&mut c, API, "build-words", &[]) {
        v @ Val::Resource(_) => v,
        other => panic!("`build-words` should return a set resource, got {other:?}"),
    };
    let words_size = c
        .call_instance(WORDS_IFACE, "[method]set.size", &[words.clone()])
        .expect("words size call should succeed");
    assert_eq!(words_size, vec![Val::U32(2)], "\"a\",\"b\",\"a\" dedups to 2");
    c.drop_resource(words)
        .expect("dropping the words set handle should run the dtor cleanly");
}

#[test]
// COMPOUND (list) element: a functor over `list(s32)`, deriving a `u32`.
// `count-groups` adds `[1 2]`, `[3 4]`, `[1 2]` (the third a structural dup) and
// returns `groups/size(s)`. The structural `eq_raw` (step 04) dedups lists by
// value, order-sensitively, exactly as the interpreter does, so the result is 2.
// This is the compound-element coverage standing in for the brief's worked
// example (a handle return over `list<point>` is the known WIT interface-cycle
// limit; see `functor_runtime.rs` and summary 04 §7).
fn compound_list_element_dedups_like_interpreter() {
    const SRC: &str = r#"Package "demo:cmp@0.1.0"
DefType nums list(s32)
Import {pkg: "wavelet:coll/set" elem: nums as: groups}
Export count-groups
Def count-groups Fn {}
  Let {s: groups/new()}
    Do [ groups/add(s [1 2])
         groups/add(s [3 4])
         groups/add(s [1 2])
         groups/size(s) ]"#;

    let mut c = build_component(SRC);
    // Interpreter oracle: {[1 2],[3 4],[1 2]} dedups to 2 distinct lists.
    assert_eq!(
        one(&mut c, "demo:cmp/api@0.1.0", "count-groups", &[]),
        Val::U32(2)
    );
}

#[test]
// Edge: a freshly constructed, empty set has size 0. `count-empty` calls
// `ints/new()` then `ints/size(s)` with no adds, matching the interpreter.
fn empty_set_size_is_zero() {
    const SRC: &str = r#"Package "demo:empty@0.1.0"
Import {pkg: "wavelet:coll/set" elem: s32 as: ints}
Export count-empty
Def count-empty Fn {}
  Let {s: ints/new()}
    ints/size(s)"#;

    let mut c = build_component(SRC);
    assert_eq!(
        one(&mut c, "demo:empty/api@0.1.0", "count-empty", &[]),
        Val::U32(0)
    );
}

#[test]
// STRING element with a handle return + host-side methods. `build-words` adds
// "hi", "yo", "hi" (one a structural dup) and *returns* the `own<set>` handle;
// the host then calls `size`/`contains` on the returned resource. Strings dedup
// by value in both the interpreter and `eq_raw`, so size is 2 and membership is
// exact. Complements the s32 handle-return test with a non-primitive scalar.
fn string_element_handle_methods_match_interpreter() {
    const SRC: &str = r#"Package "demo:str@0.1.0"
Import {pkg: "wavelet:coll/set" elem: string as: words}
Export build-words
Def build-words Fn {}
  Let {s: words/new()}
    Do [ words/add(s "hi")
         words/add(s "yo")
         words/add(s "hi")
         s ]"#;
    const IFACE: &str = "demo:str/string-set@0.1.0";
    const API: &str = "demo:str/api@0.1.0";

    let mut c = build_component(SRC);
    let handle = match one(&mut c, API, "build-words", &[]) {
        v @ Val::Resource(_) => v,
        other => panic!("`build-words` should return a set resource, got {other:?}"),
    };

    // size(self): "hi","yo","hi" dedups to 2.
    let size = c
        .call_instance(IFACE, "[method]set.size", &[handle.clone()])
        .expect("size call should succeed");
    assert_eq!(size, vec![Val::U32(2)], "deduped size should be 2");

    // contains(self, value): "hi" present, "nope" absent.
    let has = |c: &mut HostComponent, v: &str| {
        c.call_instance(
            IFACE,
            "[method]set.contains",
            &[handle.clone(), Val::String(v.to_string())],
        )
        .expect("contains call should succeed")
    };
    assert_eq!(
        has(&mut c, "hi"),
        vec![Val::Bool(true)],
        "\"hi\" should be present"
    );
    assert_eq!(
        has(&mut c, "nope"),
        vec![Val::Bool(false)],
        "\"nope\" should be absent"
    );

    // Drop the returned own<set> handle host-side; the no-op dtor runs cleanly.
    c.drop_resource(handle)
        .expect("dropping the returned set handle should run the dtor cleanly");
}
