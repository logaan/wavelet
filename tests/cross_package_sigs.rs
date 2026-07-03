//! 4.3 — a local export signature can name dep-defined types.
//!
//! The synthesized WIT brings each referenced dep type into scope with
//! `use <pkg>/<iface>@<ver>.{name};`, versioned from the resolved dependency
//! (never the hardcoded wasi fallback). Covers records, enums, variants, and
//! aliases, referenced directly and inside type constructors, plus a local
//! record whose field type is dep-defined.

fn build(tag: &str, wit: &str, app: &str) -> Vec<u8> {
    let dir = std::env::temp_dir().join(format!("wvl-xpkg-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    let deps = dir.join("wit/deps");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(deps.join("acme-pts.wit"), wit).unwrap();
    let app_path = src.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();
    let out = dir.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("build with cross-package signature references");
    let bytes = std::fs::read(&outputs[0]).expect("read built component");
    let _ = std::fs::remove_dir_all(&dir);
    bytes
}

const WIT: &str = "package acme:pts@0.3.1;\n\
    interface types {\n  \
      record point { x: s32, y: s32 }\n  \
      enum tone { light, dark }\n  \
      variant mark { at(point), off }\n  \
      type track = list<point>;\n\
    }\n";

#[test]
fn local_export_signatures_name_dep_types() {
    let app = "Package \"demo:geo@0.1.0\"\n\n\
        Import {pkg: \"acme:pts/types\" as: t}\n\n\
        // dep record, directly and inside constructors\n\
        Export {name: origin params: {} result: point}\n\
        Def origin Fn {} {x: 0 y: 0}\n\n\
        Export {name: path params: {ps: list(point)} result: option(point)}\n\
        Def path Fn {ps} If eq(len(ps) 0) none some(head(ps))\n\n\
        // dep enum and variant, constructed through the alias (4.1)\n\
        Export {name: darken params: {v: tone} result: tone}\n\
        Def darken Fn {v} t/dark\n\n\
        Export {name: mark-at params: {p: point} result: mark}\n\
        Def mark-at Fn {p} t/at(p)\n\n\
        // dep type alias (4.4) in a signature\n\
        Export {name: pair params: {p: point} result: track}\n\
        Def pair Fn {p} [p p]\n";
    let bytes = build("sigs", WIT, app);
    // The component re-encodes and validates (build_files already ran
    // wit-component); check the synthesized `use` is present and versioned.
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("acme:pts/types@0.3.1"),
        "dep types interface not referenced at its own version"
    );
}

#[test]
fn local_record_fields_name_dep_types() {
    let app = "Package \"demo:geo@0.1.0\"\n\n\
        Import {pkg: \"acme:pts/types\" as: t}\n\n\
        DefType holder {label: string p: point}\n\n\
        Export {name: hold params: {p: point} result: holder}\n\
        Def hold Fn {p} {label: \"x\" p: p}\n";
    build("fields", WIT, app);
}
