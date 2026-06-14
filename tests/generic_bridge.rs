//! Step 3 of the WASI-decoupling plan: the generic canonical-ABI bridge.
//!
//! A component that imports a *synthetic* WIT interface (vendored under
//! `wit/deps`, exactly as `wkg` would place it) and calls its functions must
//! compile through the generic lowering — the one `emit::dep_call` drives off a
//! parsed WIT signature — and re-encode/validate cleanly with `wit-component`.
//!
//! The bridge is parameterised by the signature, not by a `match fname`: there
//! is no compiler knowledge of `acme:shapes`. This locks in coverage of the
//! Step 3 value kinds — primitives (ints, bool, char), records, and tuples,
//! with parameter flattening and `retptr` results — built *alongside* the
//! hand-coded http/cli magic, which this test does not touch.

/// A fresh temp directory unique to this test, cleaned on entry and exit.
fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("wavelet-bridge-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Build a one-component project whose only import is the given synthetic WIT
/// package (written into `wit/deps`), returning the built component bytes.
/// `build_files` runs the source through the component encoder with validation
/// on, so a wrong canonical-ABI lowering for any value kind fails here.
fn build_against_wit(tag: &str, wit_file: &str, wit: &str, app: &str) -> Vec<u8> {
    let dir = scratch(tag);
    let src = dir.join("src");
    let deps = dir.join("wit/deps");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(deps.join(wit_file), wit).unwrap();

    let app_path = src.join("app.wvl");
    std::fs::write(&app_path, app).unwrap();

    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build the importer through the generic bridge");
    assert_eq!(outputs.len(), 1, "expected one component");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");

    let _ = std::fs::remove_dir_all(&dir);
    bytes
}

/// A synthetic interface whose functions cover the Step 3 value kinds —
/// primitives (s32, char, bool), a record, and tuples (including a tuple with a
/// heterogeneous string element and a tuple of records) — is callable through
/// the generic bridge: the importer flattens params, returns via `retptr`, and
/// the result re-encodes/validates with `wit-component`.
#[test]
fn generic_bridge_lowers_primitives_records_tuples() {
    let wit = "package acme:shapes@0.1.0;\n\
        interface api {\n  \
          record point { x: s32, y: s32 }\n  \
          scale: func(p: point, by: s32) -> point;\n  \
          even: func(n: s32) -> bool;\n  \
          next-char: func(c: char) -> char;\n  \
          swap: func(pair: tuple<s32, string>) -> tuple<string, s32>;\n  \
          midpoint: func(seg: tuple<point, point>) -> point;\n\
        }\n";

    // Each exported function forwards straight to the imported one, so every
    // param and result flows through the generic lower/lift path unchanged.
    let app = "Package \"demo:app@0.1.0\"\n\n\
        Import {pkg: \"acme:shapes/api\" as: sh}\n\n\
        DefType point {x: s32 y: s32}\n\n\
        Export {name: do-scale params: {p: point by: s32} result: point}\n\
        Def do-scale Fn {p: point by: s32}\n  \
          sh/scale[p by]\n\n\
        Export {name: do-even params: {n: s32} result: bool}\n\
        Def do-even Fn {n: s32}\n  \
          sh/even(n)\n\n\
        Export {name: do-next params: {c: char} result: char}\n\
        Def do-next Fn {c: char}\n  \
          sh/next-char(c)\n\n\
        Export {name: do-swap params: {pair: tuple[s32 string]} result: tuple[string s32]}\n\
        Def do-swap Fn {pair: tuple[s32 string]}\n  \
          sh/swap(pair)\n\n\
        Export {name: do-mid params: {seg: tuple[point point]} result: point}\n\
        Def do-mid Fn {seg: tuple[point point]}\n  \
          sh/midpoint(seg)\n";

    let bytes = build_against_wit("shapes", "acme-shapes.wit", wit, app);

    // The built component imports the synthetic interface and exports the
    // forwarding API — proof the generic import lowering and export wrapper both
    // ran (and `wit-component` re-validated their canonical-ABI signatures).
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("acme:shapes/api"), "import not wired into the component");
    assert!(text.contains("demo:app/api"), "forwarding api not exported");
}

/// A tuple *returned* by an imported function (a multi-flat, `retptr` aggregate)
/// is lifted back into a value: exercises the retptr-aggregate path for tuples
/// specifically, distinct from the record path.
#[test]
fn generic_bridge_lifts_tuple_results_via_retptr() {
    let wit = "package acme:pairs@0.1.0;\n\
        interface api {\n  \
          divmod: func(a: s32, b: s32) -> tuple<s32, s32>;\n\
        }\n";

    let app = "Package \"demo:app@0.1.0\"\n\n\
        Import {pkg: \"acme:pairs/api\" as: p}\n\n\
        Export {name: dm params: {a: s32 b: s32} result: tuple[s32 s32]}\n\
        Def dm Fn {a: s32 b: s32}\n  \
          p/divmod[a b]\n";

    let bytes = build_against_wit("pairs", "acme-pairs.wit", wit, app);
    assert!(
        String::from_utf8_lossy(&bytes).contains("acme:pairs/api"),
        "import not wired into the component"
    );
}
