//! End-to-end tests for the strategy-B macro component
//! ([`wavelet::emit::emit_macro_component`]): compile a macro-library file's
//! `DefMacro`s into a `wavelet:meta/macros` component and drive it through the
//! same [`wavelet::macros::MacroComponent`] consumer the build uses, asserting
//! the compiled `manifest`/`expand` match the interpreter's reference output.

use wavelet::emit::emit_macro_component;
use wavelet::macros::MacroComponent;
use wavelet::meta::{arena_to_tree, tree_to_arena};
use wavelet::printer::print;
use wavelet::reader::read_file;

const LIB: &str = "\
Package \"demo:macros@0.1.0\"\n\
DefMacro identity {x} x\n\
DefMacro unless {cond body}\n\
  Quasi If Unquote(cond) {} Unquote(body)\n\
";

fn component(src: &str) -> MacroComponent {
    let (arena, roots) = read_file(src).expect("read macro lib");
    let bytes = emit_macro_component(&arena, &roots).expect("emit macro component");
    MacroComponent::from_bytes(&bytes).expect("loads as a wavelet:meta/macros component")
}

/// The whole call form, shipped as the pinned `args` tree (element 0 is the
/// head, `1..` the argument forms).
fn args_tree(call_src: &str) -> wavelet::meta::Tree {
    let (arena, roots) = read_file(call_src).expect("read call");
    assert_eq!(roots.len(), 1);
    arena_to_tree(&arena, roots[0])
}

fn print_tree(tree: &wavelet::meta::Tree) -> String {
    let (arena, root) = tree_to_arena(tree);
    print(&arena, root)
}

#[test]
fn manifest_lists_compiled_macros() {
    let mut m = component(LIB);
    let mut got = m.manifest().expect("manifest call");
    got.sort();
    assert_eq!(got, vec![("identity".to_string(), 1u32), ("unless".to_string(), 2u32)]);
}

#[test]
fn expand_identity_returns_argument_unchanged() {
    let mut m = component(LIB);
    let out = m
        .expand("identity", &args_tree("identity(add(1 2))"))
        .expect("identity expands");
    assert_eq!(print_tree(&out), "(add, 1, 2)");
}

#[test]
fn expand_unless_rewrites_to_if_via_quasi() {
    let mut m = component(LIB);
    let out = m
        .expand("unless", &args_tree(r#"unless(false "ran")"#))
        .expect("unless expands");
    assert_eq!(print_tree(&out), r#"(if-MACRO, false, {}, "ran")"#);
}

#[test]
fn expand_unknown_macro_is_an_error() {
    let mut m = component(LIB);
    let err = m
        .expand("nope", &args_tree("nope()"))
        .expect_err("unknown macro errors");
    assert!(err.contains("nope"), "unexpected error: {err}");
}
