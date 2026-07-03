//! Backend ↔ interpreter parity for user-declared resources (4.5, DefResource).
//!
//! `CLAUDE.md` makes the interpreter the semantics oracle: a wasm-backend change
//! that diverges from it is a bug. Session 1 landed read/interp/check/wit and
//! verified the counter's behaviour at the interpreter (`counter(10)` /
//! `counter/next` / `counter/value` / `counter/sum` / identity). These tests
//! build the *same* resource program through the real emitter and execute it
//! in-process (the capability-free `wasmtime` host), asserting the backend
//! agrees. Each would fail against the pre-emit backend (which had no resource
//! machinery beyond the functor `set`).

use wavelet::host::{HostComponent, Val};

fn build_component(src_text: &str) -> HostComponent {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-resource-be-{}-{n}", std::process::id()));
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
    .expect("resource program should build");
    let _ = std::fs::remove_dir_all(&dir);
    HostComponent::from_bytes(&bytes).expect("instantiate the resource component")
}

const API: &str = "demo:counter/api@0.1.0";

const SRC: &str = r#"Package "demo:counter@0.1.0"
DefResource counter {
  New: Fn {start: u32} cell-new(start)
  next: Fn {self: counter} The u32
    Let {v: cell-get(self)}
      Do [cell-set(self add(v 1)) v]
  value: Fn {self: counter} The u32 cell-get(self)
  sum: Static Fn {values: list(u32)} The counter counter(sum-list(values))
}
Export counter

Def sum-list Fn {vs}
  If eq(vs []) 0 add(head(vs) sum-list(tail(vs)))

Export make-counter
Def make-counter Fn {start: u32} The counter counter(start)

Export bump-counter
Def bump-counter Fn {c: borrow(counter)} The u32 counter/next(c)

Export take-counter
Def take-counter Fn {c: counter} The u32 counter/value(c)

Export counter-round-trip
Def counter-round-trip Fn {c: counter} The counter counter(add(counter/value(c) 1))
"#;

fn ctor(c: &mut HostComponent, start: u32) -> Val {
    match c
        .call_instance(API, "[constructor]counter", &[Val::U32(start)])
        .expect("constructor should run")
        .into_iter()
        .next()
        .expect("constructor returns one handle")
    {
        v @ Val::Resource(_) => v,
        other => panic!("constructor should return a counter handle, got {other:?}"),
    }
}

fn next(c: &mut HostComponent, h: &Val) -> Val {
    c.call_instance(API, "[method]counter.next", std::slice::from_ref(h))
        .expect("next should run")
        .into_iter()
        .next()
        .unwrap()
}

fn value(c: &mut HostComponent, h: &Val) -> Val {
    c.call_instance(API, "[method]counter.value", std::slice::from_ref(h))
        .expect("value should run")
        .into_iter()
        .next()
        .unwrap()
}

#[test]
fn constructor_next_value_match_interpreter() {
    let mut c = build_component(SRC);
    let h = ctor(&mut c, 5);
    assert_eq!(next(&mut c, &h), Val::U32(5), "first next returns 5");
    assert_eq!(next(&mut c, &h), Val::U32(6), "second next returns 6");
    assert_eq!(value(&mut c, &h), Val::U32(7), "value reads 7 without advancing");
    assert_eq!(value(&mut c, &h), Val::U32(7), "value does not advance");
    c.drop_resource(h)
        .expect("dropping the counter handle runs the no-op dtor cleanly");
}

#[test]
fn static_sum_alt_constructor_matches_interpreter() {
    let mut c = build_component(SRC);
    let h = match c
        .call_instance(
            API,
            "[static]counter.sum",
            &[Val::List(vec![Val::U32(1), Val::U32(2), Val::U32(3)])],
        )
        .expect("sum static should run")
        .into_iter()
        .next()
        .unwrap()
    {
        v @ Val::Resource(_) => v,
        other => panic!("sum should return a counter handle, got {other:?}"),
    };
    assert_eq!(value(&mut c, &h), Val::U32(6), "sum([1 2 3]) starts at 6");
    c.drop_resource(h).expect("dtor runs cleanly");
}

#[test]
fn own_and_borrow_free_functions_match_interpreter() {
    let mut c = build_component(SRC);
    let h = match c
        .call_instance(API, "make-counter", &[Val::U32(5)])
        .expect("make-counter should run")
        .into_iter()
        .next()
        .unwrap()
    {
        v @ Val::Resource(_) => v,
        other => panic!("make-counter should return a counter, got {other:?}"),
    };
    assert_eq!(value(&mut c, &h), Val::U32(5), "make-counter(5) starts at 5");

    let bumped = c
        .call_instance(API, "bump-counter", std::slice::from_ref(&h))
        .expect("bump-counter should run");
    assert_eq!(bumped, vec![Val::U32(5)], "bump returns the pre-advance value 5");
    assert_eq!(
        value(&mut c, &h),
        Val::U32(6),
        "borrow advanced the same handle in place"
    );

    let rt = match c
        .call_instance(API, "counter-round-trip", std::slice::from_ref(&h))
        .expect("counter-round-trip should run")
        .into_iter()
        .next()
        .unwrap()
    {
        v @ Val::Resource(_) => v,
        other => panic!("round-trip should return a counter, got {other:?}"),
    };
    assert_eq!(value(&mut c, &rt), Val::U32(7), "round-trip starts at value+1 = 7");
    c.drop_resource(rt).expect("dtor runs cleanly");
}

#[test]
fn take_counter_consumes_and_reads() {
    let mut c = build_component(SRC);
    let h = ctor(&mut c, 42);
    let taken = c
        .call_instance(API, "take-counter", std::slice::from_ref(&h))
        .expect("take-counter should run");
    assert_eq!(taken, vec![Val::U32(42)], "take-counter reads 42");
}
