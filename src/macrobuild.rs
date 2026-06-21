//! Produce a `wavelet:meta/macros` component from a Wavelet macro file
//! (design.md §6.3; **strategy B: compile the bodies**).
//!
//! The payoff of the macro-component feature is that a macro library can be
//! *written in Wavelet itself*: a `.wvl` file whose top level is `DefMacro`s is
//! compiled into a component exporting `wavelet:meta/macros`, which the Step 1–8
//! consumer then imports with `macros: true` and uses exactly like a hand-built
//! macro component. Wavelet thereby dogfoods its own macro system.
//!
//! ## Strategy B: compile each macro body to wasm
//!
//! The produced component carries **no interpreter**: each `DefMacro` body is
//! compiled to a wasm function by [`crate::emit::emit_macro_component`], and the
//! component's `manifest`/`expand` exports are themselves compiled (with
//! compiled `tree`⇄form adapters at the boundary). This is a pure in-process
//! emit + componentize — no `cargo`, no `wasm32` target, and no sibling guest
//! crate. The interpreter (`interp.rs`/`macrolib.rs`) stays the differential
//! oracle the compiled output is validated against (`CLAUDE.md`), not part of
//! the produced artifact.
//!
//! ## How a file declares it is a macro library
//!
//! A file is a macro library iff its top level is a `Package` declaration and
//! `DefMacro`s only — it declares **no `Export`** and defines no runtime
//! functions/values ([`is_macro_library`]). This needs no new syntax or lexer
//! token: "a file that only defines macros and exports none of its own runtime
//! funcs" (the step's suggested trigger). `wavelet build` routes such a file
//! here instead of through the ordinary [`crate::emit`] path.
//!
//! This module is native-only, gated `#[cfg(not(target_arch = "wasm32"))]` in
//! `lib.rs` alongside the rest of the producer/consumer build machinery.

use crate::form::{Arena, Node, NodeId};

/// Does this file declare itself a macro library?
///
/// True iff every top-level form is a `Package` declaration or a `DefMacro`,
/// there is at least one `DefMacro`, and there is no `Export` (nor any runtime
/// `Def`/`DefType`/`Import`). Such a file has no runtime surface of its own — its
/// whole purpose is to publish macros — so `wavelet build` compiles it into a
/// `wavelet:meta/macros` component rather than an ordinary component.
pub fn is_macro_library(arena: &Arena, roots: &[NodeId]) -> bool {
    let mut saw_macro = false;
    let mut saw_package = false;
    for &root in roots {
        let Node::Tup(items) = arena.node(root) else { return false };
        let Some(&head) = items.first() else { return false };
        let Node::Sym(head_name) = arena.node(head) else { return false };
        match head_name.as_str() {
            "package-MACRO" => saw_package = true,
            "defmacro-MACRO" => saw_macro = true,
            // Anything else — Export, Def, DefType, Import, a bare expression —
            // means the file has a runtime surface, so it is NOT a pure macro
            // library and takes the ordinary build path.
            _ => return false,
        }
    }
    saw_package && saw_macro
}

/// Build a macro-library file's forms into a `wavelet:meta/macros` component,
/// returning the component bytes.
///
/// **Strategy B: compile the bodies.** Each `DefMacro` body is compiled to wasm
/// by [`crate::emit::emit_macro_component`], so the produced component carries no
/// interpreter — its `manifest`/`expand` are compiled functions. This needs no
/// `cargo`, no `wasm32` target, and no sibling guest crate; it is a pure
/// in-process emit + componentize, like an ordinary `wavelet build`.
///
/// `arena`/`roots` are the read form tree of the macro file (its top level is a
/// `Package` plus `DefMacro`s — see [`is_macro_library`]). Errors are actionable
/// strings from the emitter.
pub fn build_macro_component(arena: &Arena, roots: &[NodeId]) -> Result<Vec<u8>, String> {
    crate::emit::emit_macro_component(arena, roots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::read_file;

    fn roots(src: &str) -> (Arena, Vec<NodeId>) {
        read_file(src).expect("read")
    }

    #[test]
    fn pure_macro_file_is_a_macro_library() {
        let (a, r) = roots(
            "Package \"demo:macros@0.1.0\"\n\
             DefMacro identity {x} x\n\
             DefMacro unless {cond body}\n  Quasi If Unquote(cond) {} Unquote(body)\n",
        );
        assert!(is_macro_library(&a, &r));
    }

    #[test]
    fn file_with_export_is_not_a_macro_library() {
        let (a, r) = roots(
            "Package \"demo:app@0.1.0\"\n\
             DefMacro identity {x} x\n\
             Export greet\n\
             Def greet Fn {} \"hi\"\n",
        );
        assert!(!is_macro_library(&a, &r));
    }

    #[test]
    fn file_with_no_macros_is_not_a_macro_library() {
        let (a, r) = roots("Package \"demo:app@0.1.0\"\n");
        assert!(!is_macro_library(&a, &r));
    }

    #[test]
    fn file_with_runtime_def_is_not_a_macro_library() {
        let (a, r) = roots(
            "Package \"demo:app@0.1.0\"\n\
             DefMacro identity {x} x\n\
             Def x 1\n",
        );
        assert!(!is_macro_library(&a, &r));
    }
}
