fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("wavelet-bridge-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn build_against_wit(tag: &str, wit_file: &str, wit: &str, app: &str) -> Vec<u8> {
    let dir = scratch(tag);
    let src = dir.join("src");
    let deps = dir.join("wit/deps");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(deps.join(wit_file), wit).unwrap();

    let app_path = src.join("app.wlt");
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

    let app = "Package \"demo:app@0.1.0\"\n\n\
        Import {pkg: \"acme:shapes/api\" as: sh}\n\n\
        DefType point {x: s32 y: s32}\n\n\
        Export {name: do-scale params: {p: point by: s32} result: point}\n\
        Def do-scale Fn {p: point by: s32}\n  \
          sh/scale(p by)\n\n\
        Export {name: do-even params: {n: s32} result: bool}\n\
        Def do-even Fn {n: s32}\n  \
          sh/even(n)\n\n\
        Export {name: do-next params: {c: char} result: char}\n\
        Def do-next Fn {c: char}\n  \
          sh/next-char(c)\n\n\
        Export {name: do-swap params: {pair: tuple(s32 string)} result: tuple(string s32)}\n\
        Def do-swap Fn {pair: tuple(s32 string)}\n  \
          sh/swap(pair)\n\n\
        Export {name: do-mid params: {seg: tuple(point point)} result: point}\n\
        Def do-mid Fn {seg: tuple(point point)}\n  \
          sh/midpoint(seg)\n";

    let bytes = build_against_wit("shapes", "acme-shapes.wit", wit, app);

    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("acme:shapes/api"),
        "import not wired into the component"
    );
    assert!(text.contains("demo:app/api"), "forwarding api not exported");
}

#[test]
fn generic_bridge_lifts_tuple_results_via_retptr() {
    let wit = "package acme:pairs@0.1.0;\n\
        interface api {\n  \
          divmod: func(a: s32, b: s32) -> tuple<s32, s32>;\n\
        }\n";

    let app = "Package \"demo:app@0.1.0\"\n\n\
        Import {pkg: \"acme:pairs/api\" as: p}\n\n\
        Export {name: dm params: {a: s32 b: s32} result: tuple(s32 s32)}\n\
        Def dm Fn {a: s32 b: s32}\n  \
          p/divmod(a b)\n";

    let bytes = build_against_wit("pairs", "acme-pairs.wit", wit, app);
    assert!(
        String::from_utf8_lossy(&bytes).contains("acme:pairs/api"),
        "import not wired into the component"
    );
}

#[test]
fn generic_bridge_lowers_enum_variant_flags_lists_options() {
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
    assert!(
        text.contains("acme:kinds/api"),
        "import not wired into the component"
    );
    assert!(text.contains("demo:app/api"), "forwarding api not exported");
}

#[test]
fn generic_bridge_passes_resource_handles_own_borrow() {
    let wit = "package acme:res@0.1.0;\n\
        interface api {\n  \
          resource widget;\n  \
          open: func(seed: s32) -> own<widget>;\n  \
          tag: func(w: borrow<widget>) -> s32;\n  \
          peek: func(w: widget) -> s32;\n\
        }\n";

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
    assert!(
        text.contains("acme:res/api"),
        "import not wired into the component"
    );
    assert!(text.contains("demo:app/api"), "forwarding api not exported");
}

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

    let app = "Package \"demo:app@0.1.0\"\n\n\
        Import {pkg: \"acme:wire/api\" as: w}\n\n\
        Export {name: put-trip params: {n: s32} result: s32}\n\
        Def put-trip Fn {n: s32}\n  \
          w/put(w/packet(w/new-bag()) \"hi\")\n\n\
        Export {name: label-trip params: {n: s32} result: s32}\n\
        Def label-trip Fn {n: s32}\n  \
          Match w/label(w/packet(w/new-bag())) [\n    \
            (some(s)  len(s))\n    \
            (none     n)\n  \
          ]\n\n\
        Export {name: open-trip params: {n: s32} result: s32}\n\
        Def open-trip Fn {n: s32}\n  \
          Match w/open(w/packet(w/new-bag())) [\n    \
            (ok(b)   n)\n    \
            (err(e)  0)\n  \
          ]\n\n\
        Export {name: deliver-trip params: {n: s32} result: s32}\n\
        Def deliver-trip Fn {n: s32}\n  \
          w/deliver(w/open(w/packet(w/new-bag())))\n\n\
        Export {name: seal-trip params: {n: s32} result: s32}\n\
        Def seal-trip Fn {n: s32}\n  \
          w/seal(w/packet(w/new-bag()))\n\n\
        Export {name: drop-trip params: {n: s32} result: s32}\n\
        Def drop-trip Fn {n: s32}\n  \
          Do [w/drop-packet(w/packet(w/new-bag()))\n      \
              n]\n";

    let bytes = build_against_wit("wire", "acme-wire.wit", wit, app);
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("acme:wire/api"),
        "import not wired into the component"
    );
    assert!(text.contains("demo:app/api"), "forwarding api not exported");
}

#[test]
fn generic_bridge_exports_arbitrary_interface() {
    let wit = "package acme:greet@0.1.0;\n\
        interface greeter {\n  \
          record who { name: string, times: s32 }\n  \
          greet: func(prefix: string, w: who) -> string;\n\
        }\n";

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

#[test]
fn generic_bridge_exports_run_style_unit_result() {
    let wit = "package acme:cli@0.1.0;\n\
        interface run {\n  \
          run: func() -> result;\n\
        }\n";

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

    let app = "Package \"demo:app@0.1.0\"\n\n\
        Import {pkg: \"acme:wire2/api\" as: w}\n\n\
        Export {name: put-trip params: {n: s32} result: s32}\n\
        Def put-trip Fn {n: s32}\n  \
          w/put(w/open())\n\n\
        Export {name: blast-trip params: {n: s32} result: s32}\n\
        Def blast-trip Fn {n: s32}\n  \
          w/blast(\"hello\")\n";

    let bytes = build_against_wit("wire2", "acme-wire2.wit", wit, app);
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("acme:wire2/api"),
        "import not wired into the component"
    );
}
