use std::path::{Path, PathBuf};

use wavelet::expand::expand_file;
use wavelet::macros::MacroComponent;
use wavelet::meta::{arena_to_tree, tree_to_arena};
use wavelet::printer::print;
use wavelet::reader::read_file;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn lib_source() -> String {
    std::fs::read_to_string(fixtures().join("produced-macros.wlt"))
        .expect("produced-macros.wlt fixture present")
}

fn load() -> MacroComponent {
    MacroComponent::from_file(&fixtures().join("produced-macros.wasm"))
        .expect("produced-macros.wasm is a `wavelet:meta/macros` component")
}

fn args_tree(call_src: &str) -> wavelet::meta::Tree {
    let (arena, roots) = read_file(call_src).expect("read call");
    arena_to_tree(&arena, *roots.last().unwrap())
}

fn print_tree(tree: &wavelet::meta::Tree) -> String {
    let (arena, root) = tree_to_arena(tree);
    print(&arena, root)
}

fn local_expand(call_titlecase: &str) -> String {
    let src = format!("{}{call_titlecase}\n", lib_source());
    let (arena, roots) = read_file(&src).expect("read lib + call");
    let (out, new_roots) = expand_file(arena, &roots, None).expect("local expand");
    print(&out, *new_roots.last().unwrap())
}

#[test]
fn manifest_reports_the_library_macros() {
    let mut m = load();
    let mut got = m.manifest().expect("manifest call");
    got.sort();
    assert_eq!(
        got,
        vec![("identity".to_string(), 1u32), ("unless".to_string(), 2u32)],
        "produced component must publish the file's DefMacro arities"
    );
}

#[test]
fn expand_identity_matches_local() {
    let mut m = load();
    let args = args_tree("identity(add(1 2))");
    let via_component = print_tree(&m.expand("identity", &args).expect("identity expands"));
    assert_eq!(via_component, "(add, 1, 2)");

    assert_eq!(via_component, local_expand("Identity add(1 2)"));
}

#[test]
fn expand_unless_matches_local() {
    let mut m = load();
    let args = args_tree(r#"unless(false "ran")"#);

    let via_component = print_tree(&m.expand("unless", &args).expect("unless expands"));
    assert_eq!(via_component, r#"(if-MACRO, false, {}, "ran")"#);

    assert_eq!(
        local_expand(r#"Unless false "ran""#),
        r#"(match-MACRO, (the-MACRO, bool, false), [(true, {}), (false, "ran")])"#
    );
}

#[test]
fn expand_unknown_macro_is_an_error() {
    let mut m = load();
    let args = args_tree("nope()");
    let err = m.expand("nope", &args).expect_err("unknown macro errors");
    assert!(err.contains("nope"), "unexpected error: {err}");
}

#[test]
fn consumer_path_uses_produced_macro() {
    use wavelet::macrodep::{FileExpander, read_file_with_macros};

    let comp = fixtures().join("produced-macros.wasm");
    let src = format!(
        "Package \"demo:app@0.1.0\"\n\
         Import {{pkg: \"demo:macros/lib\" macros: true from: \"{}\"}}\n\
         Unless false \"ran\"\n",
        comp.to_str().unwrap()
    );

    let (arena, roots) = read_file_with_macros(&src, env!("CARGO_MANIFEST_DIR"))
        .expect("reads with foreign arities from the produced component");
    let mut fx = FileExpander::for_file(env!("CARGO_MANIFEST_DIR"), &arena, &roots)
        .expect("file imports the macro component");
    let (out, new_roots) =
        expand_file(arena, &roots, Some(&mut fx)).expect("expands via produced component");
    let via_consumer = print(&out, *new_roots.last().unwrap());

    assert_eq!(
        via_consumer,
        r#"(match-MACRO, (the-MACRO, bool, false), [(true, {}), (false, "ran")])"#
    );
    assert_eq!(via_consumer, local_expand(r#"Unless false "ran""#));
}

#[test]
fn worked_e2e_build_through_conventional_location() {
    let root = std::env::temp_dir().join(format!("wavelet-macro-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let src_dir = root.join("src");
    let macros_dir = root.join("wit").join("macros");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&macros_dir).unwrap();

    std::fs::copy(
        fixtures().join("produced-macros.wasm"),
        macros_dir.join("demo-macros.wasm"),
    )
    .unwrap();

    let app = "Package \"demo:app@0.1.0\"\n\n\
        Import {pkg: \"demo:macros/lib\" macros: true}\n\n\
        Export {name: echo-bare params: {n: s32} result: s32}\n\
        Def echo-bare Fn {n: s32}\n  \
          Identity n\n\n\
        Export {name: echo-qual params: {n: s32} result: s32}\n\
        Def echo-qual Fn {n: s32}\n  \
          lib/Identity n\n";
    let app_path = src_dir.join("app.wlt");
    std::fs::write(&app_path, app).unwrap();

    let out = root.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("consumer builds end-to-end against the conventional macro component");

    let consumer = outputs
        .iter()
        .find(|p| p.ends_with("demo-app.wasm"))
        .expect("emitted the consumer component");
    let bytes = std::fs::read(consumer).expect("read built consumer component");
    assert!(
        bytes.starts_with(b"\0asm"),
        "build output must be a real wasm component"
    );

    assert_eq!(local_expand("Identity n"), "n");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn reproduce_component_from_source() {
    let (arena, roots) = wavelet::reader::read_file(&lib_source()).expect("read macro lib");
    let bytes = wavelet::macrobuild::build_macro_component(&arena, &roots)
        .expect("producer builds the macro component");

    let mut m = MacroComponent::from_bytes(&bytes).expect("freshly built is a macro component");
    let mut got = m.manifest().expect("manifest");
    got.sort();
    assert_eq!(
        got,
        vec![("identity".to_string(), 1u32), ("unless".to_string(), 2u32)]
    );
    let args = args_tree(r#"unless(false "ran")"#);
    assert_eq!(
        print_tree(&m.expand("unless", &args).expect("unless expands")),
        r#"(if-MACRO, false, {}, "ran")"#
    );
}
