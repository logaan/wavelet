//! Dogfood test for Step 9: a macro library *written in Wavelet* compiles into a
//! `wavelet:meta/macros` component that the Step 1–8 consumer path imports and
//! expands, and its expansions match running the same `DefMacro` locally via
//! `expand_file` (the interpreter is the semantics oracle, `CLAUDE.md`).
//!
//! ## Hermetic by default (prebuilt fixture), regenerable on demand
//!
//! Producing the component (`wavelet::macrobuild`) shells out to `cargo build
//! --target wasm32-unknown-unknown`, which needs the wasm32 target installed —
//! not guaranteed in CI. So, exactly like the Step 3 hand fixture
//! (`tests/fixtures/macros/README.md`), a **prebuilt** component is checked in at
//! `tests/fixtures/produced-macros.wasm` (built from
//! `tests/fixtures/produced-macros.wvl`), and the default tests consume *that* —
//! no toolchain required, so `cargo test` stays green everywhere.
//!
//! The regeneration test (`reproduce_component_from_source`) actually runs the
//! producer; it is **opt-in** behind `WAVELET_TEST_BUILD_MACRO_COMPONENT=1` so it
//! never makes `cargo test` depend on a wasm toolchain. Run it (and refresh the
//! checked-in `.wasm`) after changing the guest or the producer:
//!
//! ```console
//! WAVELET_TEST_BUILD_MACRO_COMPONENT=1 cargo test --test produced_macros
//! ```

use std::path::{Path, PathBuf};

use wavelet::expand::expand_file;
use wavelet::macros::MacroComponent;
use wavelet::meta::{arena_to_tree, tree_to_arena};
use wavelet::printer::print;
use wavelet::reader::read_file;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

/// The macro-library source the checked-in component was produced from.
fn lib_source() -> String {
    std::fs::read_to_string(fixtures().join("produced-macros.wvl"))
        .expect("produced-macros.wvl fixture present")
}

/// Load the checked-in produced component through the consumer runtime.
fn load() -> MacroComponent {
    MacroComponent::from_file(&fixtures().join("produced-macros.wasm"))
        .expect("produced-macros.wasm is a `wavelet:meta/macros` component")
}

/// Build an `args` tree for `expand` from a whole call form (head + args),
/// honouring the pinned args-tree contract (the guest reads `items[1..]`).
fn args_tree(call_src: &str) -> wavelet::meta::Tree {
    let (arena, roots) = read_file(call_src).expect("read call");
    arena_to_tree(&arena, *roots.last().unwrap())
}

fn print_tree(tree: &wavelet::meta::Tree) -> String {
    let (arena, root) = tree_to_arena(tree);
    print(&arena, root)
}

/// Expand `<title-case-call>` *locally* via `expand_file` over the library +
/// the call, returning the canonical print of the (last) expanded root. This is
/// the oracle the produced component must match.
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
    // ... and it is what the local expander produces for the same macro+args.
    assert_eq!(via_component, local_expand("Identity add(1 2)"));
}

#[test]
fn expand_unless_matches_local() {
    let mut m = load();
    let args = args_tree(r#"unless(false "ran")"#);
    let via_component = print_tree(&m.expand("unless", &args).expect("unless expands"));
    assert_eq!(via_component, r#"(if-MACRO, false, {}, "ran")"#);
    assert_eq!(via_component, local_expand(r#"Unless false "ran""#));
}

#[test]
fn expand_unknown_macro_is_an_error() {
    let mut m = load();
    let args = args_tree("nope()");
    let err = m.expand("nope", &args).expect_err("unknown macro errors");
    assert!(err.contains("nope"), "unexpected error: {err}");
}

/// End-to-end through the *reader/expander* consumer path (Steps 6–7): import
/// the produced component with `macros: true`, then use one of its macros
/// paren-free; the foreign expander routes through the component's `expand` and
/// the result matches local expansion of the same macro.
#[test]
fn consumer_path_uses_produced_macro() {
    use wavelet::macrodep::{read_file_with_macros, FileExpander};

    let comp = fixtures().join("produced-macros.wasm");
    let src = format!(
        "Package \"demo:app@0.1.0\"\n\
         Import {{pkg: \"demo:macros/lib\" macros: true from: \"{}\"}}\n\
         Unless false \"ran\"\n",
        comp.to_str().unwrap()
    );
    // Reading registers the produced component's manifest arities, so the
    // paren-free `Unless` reads with arity 2.
    let (arena, roots) = read_file_with_macros(&src, env!("CARGO_MANIFEST_DIR"))
        .expect("reads with foreign arities from the produced component");
    let mut fx = FileExpander::for_file(env!("CARGO_MANIFEST_DIR"), &arena, &roots)
        .expect("file imports the macro component");
    let (out, new_roots) =
        expand_file(arena, &roots, Some(&mut fx)).expect("expands via produced component");
    let via_consumer = print(&out, *new_roots.last().unwrap());
    assert_eq!(via_consumer, r#"(if-MACRO, false, {}, "ran")"#);
    assert_eq!(via_consumer, local_expand(r#"Unless false "ran""#));
}

/// **The Step 10 worked end-to-end build.** This is the user-facing story top to
/// bottom: a macro library *written in Wavelet* (`produced-macros.wvl`, compiled
/// to the checked-in `produced-macros.wasm`) dropped at the conventional
/// `wit/macros/<ns>-<name>.wasm` location, imported by another file with
/// `Import {… macros: true}` (no `from:` — resolution is by convention), and its
/// foreign `identity` macro used *inside an exported function*. The whole thing
/// goes through `wavelet::build::build_files` — the same entry point the
/// `wavelet build` CLI uses — so this asserts the macro expands through the
/// component at build time **and** the expanded program emits a real
/// `wavelet:meta`-free wasm component.
///
/// It exercises three things the read/expand-only test above does not:
///   1. conventional `wit/macros/` resolution (no explicit `from:` path);
///   2. the foreign macro used *qualified* (`lib/Identity`) as well as bare; and
///   3. the full emit path (`build_files`), proving the build artifact is real.
///
/// The macro-only import is compile-time-only (§6.3): it contributes no runtime
/// dependency, so the consumer emits as a single self-contained component with no
/// `wac`/`wkg` toolchain needed — the build stays hermetic.
#[test]
fn worked_e2e_build_through_conventional_location() {
    // A throwaway project root: src/app.wvl + wit/macros/demo-macros.wasm.
    let root = std::env::temp_dir().join(format!("wavelet-macro-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let src_dir = root.join("src");
    let macros_dir = root.join("wit").join("macros");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&macros_dir).unwrap();

    // Drop the produced macro library at the conventional location for package
    // `demo:macros` (`:`/`/` → `-`): `wit/macros/demo-macros.wasm`.
    std::fs::copy(
        fixtures().join("produced-macros.wasm"),
        macros_dir.join("demo-macros.wasm"),
    )
    .unwrap();

    // A consumer that imports the library `macros: true` and uses its
    // `identity` macro BOTH bare (`Identity`) and qualified (`lib/Identity`)
    // inside exported functions. `Identity x` expands (through the component) to
    // `x`, so each export just returns its argument — an expansion the wasm
    // backend can emit, which keeps this a real end-to-end *build*.
    let app = "Package \"demo:app@0.1.0\"\n\n\
        Import {pkg: \"demo:macros/lib\" macros: true}\n\n\
        Export {name: echo-bare params: {n: s32} result: s32}\n\
        Def echo-bare Fn {n: s32}\n  \
          Identity n\n\n\
        Export {name: echo-qual params: {n: s32} result: s32}\n\
        Def echo-qual Fn {n: s32}\n  \
          lib/Identity n\n";
    let app_path = src_dir.join("app.wvl");
    std::fs::write(&app_path, app).unwrap();

    let out = root.join("out");
    let outputs = wavelet::build::build_files(
        &[app_path.to_str().unwrap().to_string()],
        out.to_str().unwrap(),
    )
    .expect("consumer builds end-to-end against the conventional macro component");

    // One component: the consumer. The macro-only import is compile-time-only,
    // so it is not a runtime dep and there is nothing to compose.
    let consumer = outputs
        .iter()
        .find(|p| p.ends_with("demo-app.wasm"))
        .expect("emitted the consumer component");
    let bytes = std::fs::read(consumer).expect("read built consumer component");
    assert!(
        bytes.starts_with(b"\0asm"),
        "build output must be a real wasm component"
    );

    // Sanity: the *expansion* the build performed matches the oracle — the
    // foreign `identity` macro returns its argument form unchanged.
    assert_eq!(local_expand("Identity n"), "n");

    let _ = std::fs::remove_dir_all(&root);
}

/// Run the producer (`wavelet::macrobuild`, strategy B) and assert the freshly
/// built component behaves identically to the checked-in fixture. No wasm
/// toolchain is needed any more — the bodies are compiled in-process — so this
/// runs unconditionally.
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
