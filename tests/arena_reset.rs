use wavelet::host::{HostComponent, Val};

fn component() -> HostComponent {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wavelet-arena-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    let app = r#"Package "demo:app@0.1.0"

Def greeting str-cat("hello" " " "world")

Export {name: greet params: {n: u64} result: string}
Def greet Fn {n: u64}
  str-cat(greeting "-" to-string(n))

Export {name: spin params: {xs: list(u64)} result: u64}
Def spin Fn {xs: list(u64)}
  Match xs [
    ([] 0)
    ([h] h)
    (other add(head(other) spin(tail(other))))
  ]
"#;
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();
    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the arena-reset app");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);
    HostComponent::from_bytes(&bytes).expect("instantiate")
}

const IFACE: &str = "demo:app/api@0.1.0";

#[test]
fn repeated_calls_reset_the_arena_and_recompute_value_defs() {
    let mut c = component();
    for i in 0..50u64 {
        let got = c
            .call_instance(IFACE, "greet", &[Val::U64(i)])
            .unwrap_or_else(|e| panic!("greet call {i}: {e}"));
        assert_eq!(got[0], Val::String(format!("hello world-{i}")));
    }
}

#[test]
fn compound_args_survive_resets_across_calls() {
    let mut c = component();
    for round in 0..20u64 {
        let xs: Vec<Val> = (0..40).map(|k| Val::U64(round + k)).collect();
        let want: u64 = (0..40).map(|k| round + k).sum();
        let got = c
            .call_instance(IFACE, "spin", &[Val::List(xs)])
            .unwrap_or_else(|e| panic!("spin round {round}: {e}"));
        assert_eq!(got[0], Val::U64(want));
    }
}
