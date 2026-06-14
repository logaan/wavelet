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

/// Step 4 kinds — `enum`, `variant`, `flags`, plus `list`/`string`/`option`/
/// `result` — all flow through the *generic* bridge. Each is threaded entirely
/// *inside* a body: a dep function that produces the value (the lift path) feeds
/// another dep function that consumes it (the lower path), so the value crosses
/// the boundary in both directions while the app's exported signature mentions
/// only primitives. That keeps the dep-defined types (`color`, `shape`, `perms`)
/// off the app's own interface — Wavelet source has no enum/variant/flags type
/// syntax to re-declare them — yet still exercises every new lowering, and the
/// whole component re-encodes/validates with `wit-component`.
#[test]
fn generic_bridge_lowers_enum_variant_flags_lists_options() {
    // `make-*` lifts a host-returned value into a box; `*-code` lowers a box
    // back across the boundary. Variant cases carry mixed payloads (one with a
    // string, one payload-less) to exercise the join + payload-offset paths.
    let wit = "package acme:kinds@0.1.0;\n\
        interface api {\n  \
          enum color { red, green, blue }\n  \
          flags perms { read, write, exec }\n  \
          variant shape { circle(s32), point, label(string) }\n  \
          make-color: func(n: s32) -> color;\n  \
          color-code: func(c: color) -> s32;\n  \
          make-perms: func(n: s32) -> perms;\n  \
          perms-code: func(p: perms) -> s32;\n  \
          make-shape: func(n: s32) -> shape;\n  \
          shape-code: func(s: shape) -> s32;\n  \
          make-list: func(n: s32) -> list<s32>;\n  \
          list-sum: func(xs: list<s32>) -> s32;\n  \
          make-text: func(n: s32) -> string;\n  \
          text-len: func(s: string) -> s32;\n  \
          make-opt: func(n: s32) -> option<string>;\n  \
          opt-len: func(o: option<string>) -> s32;\n  \
          make-res: func(n: s32) -> result<s32, string>;\n  \
          res-code: func(r: result<s32, string>) -> s32;\n\
        }\n";

    // Each exported `*-trip` forwards `make-X` straight into `X-code`, so the
    // dep value is lifted then lowered without ever appearing in this package's
    // own WIT. Inference can't see through a dep call, so each uses the explicit
    // Export record form with a primitive `result`.
    let app = "Package \"demo:app@0.1.0\"\n\n\
        Import {pkg: \"acme:kinds/api\" as: k}\n\n\
        Export {name: color-trip params: {n: s32} result: s32}\n\
        Def color-trip Fn {n: s32}\n  \
          k/color-code(k/make-color(n))\n\n\
        Export {name: perms-trip params: {n: s32} result: s32}\n\
        Def perms-trip Fn {n: s32}\n  \
          k/perms-code(k/make-perms(n))\n\n\
        Export {name: shape-trip params: {n: s32} result: s32}\n\
        Def shape-trip Fn {n: s32}\n  \
          k/shape-code(k/make-shape(n))\n\n\
        Export {name: list-trip params: {n: s32} result: s32}\n\
        Def list-trip Fn {n: s32}\n  \
          k/list-sum(k/make-list(n))\n\n\
        Export {name: text-trip params: {n: s32} result: s32}\n\
        Def text-trip Fn {n: s32}\n  \
          k/text-len(k/make-text(n))\n\n\
        Export {name: opt-trip params: {n: s32} result: s32}\n\
        Def opt-trip Fn {n: s32}\n  \
          k/opt-len(k/make-opt(n))\n\n\
        Export {name: res-trip params: {n: s32} result: s32}\n\
        Def res-trip Fn {n: s32}\n  \
          k/res-code(k/make-res(n))\n";

    let bytes = build_against_wit("kinds", "acme-kinds.wit", wit, app);
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("acme:kinds/api"), "import not wired into the component");
    assert!(text.contains("demo:app/api"), "forwarding api not exported");
}

/// Step 5 kinds — resource *handles* (`own`/`borrow`, and a bare resource-name
/// reference) — flow through the *generic* bridge from parsed WIT, with no
/// `is_resource_name` allowlist entry for the dep's resource. A handle is a
/// single i32 flat (own and borrow lower/lift identically), carried in an int
/// box so ordinary code can pass it around without inspecting it.
///
/// As with the Step 4 kinds, each handle is round-tripped entirely *inside* a
/// body — a dep fn that returns a handle feeds a dep fn that takes one — so the
/// dep-defined `widget` resource never appears in this package's own WIT. That
/// keeps the app interface over primitives (which inference can produce) while
/// still exercising both the lift (handle out of the host) and lower (handle
/// back in) paths. The whole component re-encodes/validates with `wit-component`.
#[test]
fn generic_bridge_passes_resource_handles_own_borrow() {
    // `open` mints an `own<widget>`; `tag` reads a `borrow<widget>`; `peek`
    // takes the resource *by bare name* (no `own`/`borrow` wrapper), which only
    // types as a handle if the boundary resolves `widget` as a resource through
    // the generic path — i.e. with `is_resource_name` retired here.
    let wit = "package acme:res@0.1.0;\n\
        interface api {\n  \
          resource widget;\n  \
          open: func(seed: s32) -> own<widget>;\n  \
          tag: func(w: borrow<widget>) -> s32;\n  \
          peek: func(w: widget) -> s32;\n\
        }\n";

    // `tag-trip` lifts an `own<widget>` then lowers it as a `borrow<widget>`;
    // `peek-trip` lowers it against a bare-name parameter. Both keep `widget`
    // off this package's own exported WIT.
    let app = "Package \"demo:app@0.1.0\"\n\n\
        Import {pkg: \"acme:res/api\" as: r}\n\n\
        Export {name: tag-trip params: {n: s32} result: s32}\n\
        Def tag-trip Fn {n: s32}\n  \
          r/tag(r/open(n))\n\n\
        Export {name: peek-trip params: {n: s32} result: s32}\n\
        Def peek-trip Fn {n: s32}\n  \
          r/peek(r/open(n))\n";

    let bytes = build_against_wit("res", "acme-res.wit", wit, app);
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("acme:res/api"), "import not wired into the component");
    assert!(text.contains("demo:app/api"), "forwarding api not exported");
}
