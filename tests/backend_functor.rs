use wavelet::host::{HostComponent, Val};

fn build_component(src_text: &str) -> HostComponent {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-functor-be-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let path = src.join("app.wlt");
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

fn one(c: &mut HostComponent, iface: &str, name: &str, args: &[Val]) -> Val {
    let out = c
        .call_instance(iface, name, args)
        .unwrap_or_else(|e| panic!("`{iface}#{name}` should run: {e}"));
    assert_eq!(
        out.len(),
        1,
        "`{name}` should return one value, got {out:?}"
    );
    out.into_iter().next().unwrap()
}

#[test]

fn routed_record_set_size_matches_interpreter() {
    const SRC: &str = r#"Package "demo:geo@0.1.0"
DefType point {x: s32 y: s32}
Derive {Eq Ord Show} point
Instantiate {pkg: "wavelet:coll/set" with: {elem: point} as: pts}
Export count-distinct
Def count-distinct Fn {}
  Let {s: pts/new()}
    Do [ pts/add(s {x: 1 y: 2})
         pts/add(s {x: 3 y: 4})
         pts/add(s {x: 1 y: 2})
         pts/size(s) ]"#;

    let mut c = build_component(SRC);

    assert_eq!(
        one(&mut c, "demo:geo/api@0.1.0", "count-distinct", &[]),
        Val::U32(2)
    );
}

#[test]

fn returned_handle_methods_match_interpreter() {
    const SRC: &str = r#"Package "demo:app@0.1.0"
Instantiate {pkg: "wavelet:coll/set" with: {elem: s32} as: ints}
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

    let size = c
        .call_instance(IFACE, "[method]set.size", std::slice::from_ref(&handle))
        .expect("size call should succeed");
    assert_eq!(size, vec![Val::U32(2)], "deduped size should be 2");

    let has = |c: &mut HostComponent, v: i32| {
        c.call_instance(
            IFACE,
            "[method]set.contains",
            &[handle.clone(), Val::S32(v)],
        )
        .expect("contains call should succeed")
    };
    assert_eq!(has(&mut c, 1), vec![Val::Bool(true)], "1 should be present");
    assert_eq!(has(&mut c, 2), vec![Val::Bool(true)], "2 should be present");
    assert_eq!(has(&mut c, 9), vec![Val::Bool(false)], "9 should be absent");

    c.drop_resource(handle)
        .expect("dropping the returned set handle should run the dtor cleanly");
}

#[test]

fn two_instantiations_in_one_world_match_interpreter() {
    const SRC: &str = r#"Package "demo:multi@0.1.0"
Instantiate {pkg: "wavelet:coll/set" with: {elem: s32} as: ints}
Instantiate {pkg: "wavelet:coll/set" with: {elem: string} as: words}
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

    let ints = match one(&mut c, API, "build-ints", &[]) {
        v @ Val::Resource(_) => v,
        other => panic!("`build-ints` should return a set resource, got {other:?}"),
    };
    let ints_size = c
        .call_instance(INTS_IFACE, "[method]set.size", std::slice::from_ref(&ints))
        .expect("ints size call should succeed");
    assert_eq!(ints_size, vec![Val::U32(1)], "1 added twice dedups to 1");
    c.drop_resource(ints)
        .expect("dropping the ints set handle should run the dtor cleanly");

    let words = match one(&mut c, API, "build-words", &[]) {
        v @ Val::Resource(_) => v,
        other => panic!("`build-words` should return a set resource, got {other:?}"),
    };
    let words_size = c
        .call_instance(
            WORDS_IFACE,
            "[method]set.size",
            std::slice::from_ref(&words),
        )
        .expect("words size call should succeed");
    assert_eq!(
        words_size,
        vec![Val::U32(2)],
        "\"a\",\"b\",\"a\" dedups to 2"
    );
    c.drop_resource(words)
        .expect("dropping the words set handle should run the dtor cleanly");
}

#[test]

fn compound_list_element_dedups_like_interpreter() {
    const SRC: &str = r#"Package "demo:cmp@0.1.0"
DefType nums list(s32)
Instantiate {pkg: "wavelet:coll/set" with: {elem: nums} as: groups}
Export count-groups
Def count-groups Fn {}
  Let {s: groups/new()}
    Do [ groups/add(s [1 2])
         groups/add(s [3 4])
         groups/add(s [1 2])
         groups/size(s) ]"#;

    let mut c = build_component(SRC);

    assert_eq!(
        one(&mut c, "demo:cmp/api@0.1.0", "count-groups", &[]),
        Val::U32(2)
    );
}

#[test]

fn empty_set_size_is_zero() {
    const SRC: &str = r#"Package "demo:empty@0.1.0"
Instantiate {pkg: "wavelet:coll/set" with: {elem: s32} as: ints}
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

fn string_element_handle_methods_match_interpreter() {
    const SRC: &str = r#"Package "demo:str@0.1.0"
Instantiate {pkg: "wavelet:coll/set" with: {elem: string} as: words}
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

    let size = c
        .call_instance(IFACE, "[method]set.size", std::slice::from_ref(&handle))
        .expect("size call should succeed");
    assert_eq!(size, vec![Val::U32(2)], "deduped size should be 2");

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

    c.drop_resource(handle)
        .expect("dropping the returned set handle should run the dtor cleanly");
}

#[test]

fn record_element_handle_return_hoists_the_element() {
    const SRC: &str = r#"Package "demo:app@0.1.0"
DefType point {x: s32 y: s32}
Derive {Eq Ord Show} point
Instantiate {pkg: "wavelet:coll/set" with: {elem: point} as: pts}
Export build-points
Def build-points Fn {}
  Let {s: pts/new()}
    Do [ pts/add(s {x: 1 y: 2})
         pts/add(s {x: 1 y: 2})
         pts/add(s {x: 3 y: 4})
         s ]"#;
    const IFACE: &str = "demo:app/point-set@0.1.0";
    const API: &str = "demo:app/api@0.1.0";

    let mut c = build_component(SRC);
    let handle = match one(&mut c, API, "build-points", &[]) {
        v @ Val::Resource(_) => v,
        other => panic!("`build-points` should return a set resource, got {other:?}"),
    };

    let size = c
        .call_instance(IFACE, "[method]set.size", std::slice::from_ref(&handle))
        .expect("size call should succeed");
    assert_eq!(size, vec![Val::U32(2)], "deduped size should be 2");

    let point = |x: i32, y: i32| {
        Val::Record(vec![
            ("x".to_string(), Val::S32(x)),
            ("y".to_string(), Val::S32(y)),
        ])
    };
    let has = |c: &mut HostComponent, p: Val| {
        c.call_instance(IFACE, "[method]set.contains", &[handle.clone(), p])
            .expect("contains call should succeed")
    };
    assert_eq!(has(&mut c, point(1, 2)), vec![Val::Bool(true)]);
    assert_eq!(has(&mut c, point(3, 4)), vec![Val::Bool(true)]);
    assert_eq!(has(&mut c, point(9, 9)), vec![Val::Bool(false)]);

    c.drop_resource(handle)
        .expect("dropping the returned set handle should run the dtor cleanly");
}
