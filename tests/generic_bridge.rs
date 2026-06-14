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

/// Step 6 — resource *operations*: `[constructor]`, `[method]`, `[static]`, and
/// the implicit `[resource-drop]` all lower through the *generic* bridge — the
/// generic counterpart to the hand-coded `http_call` `match fname`.
///
/// The synthetic `acme:wire` interface is shaped to exercise exactly the
/// function-kinds (and handle / retptr signatures) the WASI-http magic uses:
///
/// | synthetic op                          | `http_call` counterpart                       |
/// |---------------------------------------|-----------------------------------------------|
/// | `[constructor]bag` (no self)          | `[constructor]fields` (`http/fields`)         |
/// | `[constructor]packet(own<bag>)`       | `[constructor]outgoing-response`              |
/// | `[method]packet.open(self)`           | `[method]outgoing-response.body` — retptr `result<own<T>>` |
/// | `[method]packet.label(self)`          | `[method]incoming-request.path-with-query` — retptr `option<string>` |
/// | `[method]packet.put(self, list<u8>)`  | `[method]outgoing-body.write` — method + list |
/// | `[static]packet.deliver(result<…>)`   | `[static]response-outparam.set` — `result` arg |
/// | `[static]packet.seal(own<packet>)`    | `[static]outgoing-body.finish` (`finish`)     |
/// | `[resource-drop]packet`               | `[resource-drop]output-stream`                |
///
/// Each is reached from source by its **bare op name** (`wire/bag`, `wire/open`,
/// `wire/deliver`, …) — exactly how the magic exposes `http/fields`,
/// `http/body`, `http/set`. `[resource-drop]packet` is reached as `wire/packet`
/// (its op name is the resource name). Every op is called inside a body that
/// returns a primitive, so the `bag`/`packet`/`box` resources never appear on
/// the app's own exported WIT, and the whole component re-encodes/validates
/// with `wit-component`.
#[test]
fn generic_bridge_lowers_resource_methods_static_constructor_drop() {
    let wit = "package acme:wire@0.1.0;\n\
        interface api {\n  \
          resource bag;\n  \
          resource box;\n  \
          resource packet {\n    \
            constructor(headers: own<bag>);\n    \
            open: func() -> result<own<box>, s32>;\n    \
            label: func() -> option<string>;\n    \
            put: func(bytes: list<u8>) -> s32;\n    \
            deliver: static func(r: result<own<box>, s32>) -> s32;\n    \
            seal: static func(this: own<packet>) -> s32;\n  \
          }\n  \
          new-bag: func() -> own<bag>;\n\
        }\n";

    // Each export drives one operation kind through the generic bridge:
    //   ctor-trip   — `[constructor]bag` (no self) then `[constructor]packet`
    //                 (taking the bag handle), then a method on the packet.
    //   open-trip   — a method whose result is `result<own<box>, s32>` (retptr),
    //                 destructured with Match to a primitive.
    //   label-trip  — a method whose result is `option<string>` (retptr).
    //   put-trip    — a method taking `(self, list<u8>)`.
    //   static-trip — `[static]packet.seal` (a static over an `own` handle).
    //   drop-trip   — the implicit `[resource-drop]packet`, reached as
    //                 `wire/drop-packet`, then returns a primitive.
    // Inference can't see through a dep call, so each uses the explicit Export
    // record form with a primitive `result`.
    let app = "Package \"demo:app@0.1.0\"\n\n\
        Import {pkg: \"acme:wire/api\" as: w}\n\n\
        Export {name: put-trip params: {n: s32} result: s32}\n\
        Def put-trip Fn {n: s32}\n  \
          w/put[w/packet(w/new-bag[]) \"hi\"]\n\n\
        Export {name: label-trip params: {n: s32} result: s32}\n\
        Def label-trip Fn {n: s32}\n  \
          Match w/label(w/packet(w/new-bag[])) [\n    \
            (some(s)  len(s))\n    \
            (none     n)\n  \
          ]\n\n\
        Export {name: open-trip params: {n: s32} result: s32}\n\
        Def open-trip Fn {n: s32}\n  \
          Match w/open(w/packet(w/new-bag[])) [\n    \
            (ok(b)   n)\n    \
            (err(e)  0)\n  \
          ]\n\n\
        Export {name: deliver-trip params: {n: s32} result: s32}\n\
        Def deliver-trip Fn {n: s32}\n  \
          w/deliver(w/open(w/packet(w/new-bag[])))\n\n\
        Export {name: seal-trip params: {n: s32} result: s32}\n\
        Def seal-trip Fn {n: s32}\n  \
          w/seal(w/packet(w/new-bag[]))\n\n\
        Export {name: drop-trip params: {n: s32} result: s32}\n\
        Def drop-trip Fn {n: s32}\n  \
          Do [w/drop-packet(w/packet(w/new-bag[]))\n      \
              n]\n";

    let bytes = build_against_wit("wire", "acme-wire.wit", wit, app);
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("acme:wire/api"), "import not wired into the component");
    assert!(text.contains("demo:app/api"), "forwarding api not exported");
}

/// Step 7 — generic *export* of an arbitrary interface. A component exports a
/// function into an external interface (`acme:greet/greeter`) named directly via
/// the explicit `Export {iface: …}` form, with its WIT signature coming from the
/// package vendored under `wit/deps` — no `is_command`/`is_http` branch, no
/// compiler knowledge of `acme:greet`. The export wrapper lifts the incoming
/// params and lowers the result entirely off the parsed signature, and
/// `wit-component` re-validates the exported interface against the real WIT.
#[test]
fn generic_bridge_exports_arbitrary_interface() {
    // The target interface the component implements. Its function takes a
    // string and a record and returns a string — exercising param lifting
    // (string + record) and a retptr-string result through the export wrapper.
    let wit = "package acme:greet@0.1.0;\n\
        interface greeter {\n  \
          record who { name: string, times: s32 }\n  \
          greet: func(prefix: string, w: who) -> string;\n\
        }\n";

    // `greet` is exported into `acme:greet/greeter` (not the local `api`). The
    // body just returns the prefix, so the value flows back out through the
    // generic string retptr path.
    let app = "Package \"demo:app@0.1.0\"\n\n\
        DefType who {name: string times: s32}\n\n\
        Export {name: greet iface: \"acme:greet/greeter\" \
          params: {prefix: string w: who} result: string}\n\
        Def greet Fn {prefix: string w: who}\n  \
          prefix\n";

    let bytes = build_against_wit("greet", "acme-greet.wit", wit, app);
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("acme:greet/greeter"),
        "external interface not exported through the generic path"
    );
}

/// Step 7 — the `wasi:cli/run` `() -> result` wrapper is just "export this
/// function into `wasi:cli/run` with its WIT signature", reproduced through the
/// *generic* export path with no `is_command` branch. The function returns a
/// `result` value (`ok(0)`); the export wrapper lowers it to the canonical
/// single-i32 `result` discriminant off the parsed `func() -> result` signature.
/// A synthetic `acme:cli` package mirrors `wasi:cli/run`'s shape so the test
/// stays hermetic, and `wit-component` re-validates the `() -> result` export.
#[test]
fn generic_bridge_exports_run_style_unit_result() {
    let wit = "package acme:cli@0.1.0;\n\
        interface run {\n  \
          run: func() -> result;\n\
        }\n";

    // No params, returns a bare `result` — exactly the `wasi:cli/run` shape.
    // `ok(0)` builds the ok arm (its payload is dropped: `result` is unit-armed).
    let app = "Package \"demo:app@0.1.0\"\n\n\
        Export {name: run iface: \"acme:cli/run\" result: result}\n\
        Def run Fn {}\n  \
          ok(0)\n";

    let bytes = build_against_wit("cli-run", "acme-cli.wit", wit, app);
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("acme:cli/run"),
        "run-style interface not exported through the generic path"
    );
}

/// Step 8 — two bridge completions that routing http over the generic path
/// needs, locked in hermetically (no live `wkg`):
///
/// 1. **Variant flat-join with numeric widening.** `put` takes a
///    `result<own<thing>, error>` whose `error` variant mixes `i32`- and
///    `i64`-flattened arms (`small(u32)` vs `big(u64)`), so the canonical-ABI
///    `join` must widen the shared payload slot to `i64`. Previously the backend
///    rejected this ("arms with differing flat shapes"); now it widens (the same
///    shape the real `wasi:http` `error-code` argument to `response-outparam.set`
///    needs).
/// 2. **A Wavelet string lowers as `list<u8>`.** `blast` takes a `list<u8>` and
///    is called with a string literal — exactly how an http body is written via
///    `output-stream.blocking-write-and-flush(list<u8>)`.
///
/// Both flow through the generic bridge and the component re-encodes/validates.
#[test]
fn generic_bridge_widens_variant_arms_and_strings_as_byte_lists() {
    let wit = "package acme:wire2@0.1.0;\n\
        interface api {\n  \
          resource thing;\n  \
          variant error {\n    \
            small(u32),\n    \
            big(u64),\n    \
            none,\n  \
          }\n  \
          open: func() -> result<own<thing>, error>;\n  \
          put: func(r: result<own<thing>, error>) -> s32;\n  \
          blast: func(bytes: list<u8>) -> s32;\n\
        }\n";

    // `put-trip` round-trips the widened `result<own<thing>, error>` (built as
    // `ok(open())`) straight back into `put` — exercising lower+lift of the
    // i32/i64-mixed variant. `blast-trip` passes a string where `list<u8>` is
    // expected. Inference can't see through dep calls, so each uses the explicit
    // Export record form with a primitive result.
    let app = "Package \"demo:app@0.1.0\"\n\n\
        Import {pkg: \"acme:wire2/api\" as: w}\n\n\
        Export {name: put-trip params: {n: s32} result: s32}\n\
        Def put-trip Fn {n: s32}\n  \
          w/put(ok(w/open[]))\n\n\
        Export {name: blast-trip params: {n: s32} result: s32}\n\
        Def blast-trip Fn {n: s32}\n  \
          w/blast(\"hello\")\n";

    let bytes = build_against_wit("wire2", "acme-wire2.wit", wit, app);
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("acme:wire2/api"), "import not wired into the component");
}
