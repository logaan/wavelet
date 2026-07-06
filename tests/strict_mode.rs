//! 3.9 — strict mode: `Type::Unknown` is an error rather than a gradual top.
//!
//! Strict ships opt-in (`wavelet --strict …`, `check::set_strict`). The
//! accepted gate for flipping the default: the documented example suite and
//! the conformance suite pass under strict; until then the failure list under
//! strict IS the burn-down backlog (run the ignored `strict_backlog` test to
//! print it). Once flipped, gradual mode is deleted.
//!
//! The flag is process-wide, and every other integration-test binary runs
//! with it off, so all strict assertions live in this one serialized test.

#[test]
fn strict_mode_flips_unknown_from_top_to_error() {
    // Baseline (gradual): an untyped higher-order def is accepted.
    let higher_order = "Def twice Fn {f x} f(f(x))\nDef inc Fn {n} add(n 1)\ntwice(inc 10)";
    let r = wavelet::eval_snippet(higher_order);
    assert!(r.ok, "gradual baseline failed: {}", r.error);

    wavelet::check::set_strict(true);
    // Closure values have no static type yet (function types are future
    // work), so under strict the same program is rejected.
    let r = wavelet::eval_snippet(higher_order);
    wavelet::check::set_strict(false);
    assert!(!r.ok, "strict mode must reject Unknown-typed expressions");
    assert!(
        r.error.contains("strict mode"),
        "expected the strict diagnostic, got: {}",
        r.error
    );

    // Fully typed programs pass under strict.
    wavelet::check::set_strict(true);
    let r = wavelet::eval_snippet(
        "Def shout Fn {phrase: string} str-cat(upper(phrase) \"!\")\nshout(\"hi\")",
    );
    wavelet::check::set_strict(false);
    assert!(r.ok, "typed program must pass under strict: {}", r.error);

    // 5.8: a function value whose arrow type is fully concrete (a closure with
    // typed parameters, applied indirectly) now passes under strict — the
    // "function values" backlog category clears once the arrow is concrete.
    wavelet::check::set_strict(true);
    let r = wavelet::eval_snippet("Let {g: Fn {n: s32} add(n 1)} g(41)");
    wavelet::check::set_strict(false);
    assert!(r.ok, "typed closure must pass under strict: {}", r.error);
    assert_eq!(r.value, "42");
}

/// Diagnostic, not a gate: print the strict-mode burn-down backlog over the
/// documented example suite. Run with
/// `cargo test --test strict_mode -- --ignored --nocapture`.
#[test]
#[ignore = "diagnostic: prints the strict burn-down backlog"]
fn strict_backlog() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/examples.json");
    let text = std::fs::read_to_string(path).expect("read examples.json");
    let examples: serde_json::Value = serde_json::from_str(&text).expect("parse");
    let map = examples.as_object().expect("object");

    wavelet::check::set_strict(true);
    let mut failures = Vec::new();
    for (id, entry) in map {
        let code = entry["code"].as_str().expect("code");
        let expect_error = entry.get("error").is_some();
        let r = wavelet::eval_snippet(code);
        if !expect_error && !r.ok {
            failures.push(format!("{id}: {}", r.error));
        }
    }
    wavelet::check::set_strict(false);

    println!(
        "strict burn-down backlog: {} of {} examples fail under --strict",
        failures.len(),
        map.len()
    );
    for f in &failures {
        println!("  {f}");
    }
}
