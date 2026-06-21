//! F4 — the permanent differential harness for macro expansion.
//!
//! For a corpus of macro-using programs, expand each **two** ways and assert the
//! results are identical forms:
//!
//! 1. the **interpreter** (`expand_file` with no `ForeignExpander`) — the
//!    semantics oracle (`CLAUDE.md`), kept for the life of the project; and
//! 2. the **compiled** strategy-B path (`expand_file` with a `FileExpander`,
//!    which compiles the file's `DefMacro`s to a `wavelet:meta/macros`
//!    component and expands through it).
//!
//! This guards every later backend change: a compiled expansion that diverges
//! from the interpreter is a bug.

use wavelet::expand::{expand_file, ForeignExpander};
use wavelet::macrodep::FileExpander;
use wavelet::printer::print;
use wavelet::reader::read_file;

/// Expand `src` via the interpreter (the oracle), returning the printed forms.
fn interp_expand(src: &str) -> String {
    let (arena, roots) = read_file(src).expect("read");
    let (out, new_roots) = expand_file(arena, &roots, None).expect("interpreter expand");
    new_roots.iter().map(|&r| print(&out, r)).collect::<Vec<_>>().join("\n")
}

/// Expand `src` via the compiled local-macro component (strategy B), returning
/// the printed forms.
fn compiled_expand(src: &str) -> String {
    let (arena, roots) = read_file(src).expect("read");
    let mut fx = FileExpander::for_file(".", &arena, &roots)
        .expect("file defines local macros");
    let (out, new_roots) =
        expand_file(arena, &roots, Some(&mut fx as &mut dyn ForeignExpander))
            .expect("compiled expand");
    new_roots.iter().map(|&r| print(&out, r)).collect::<Vec<_>>().join("\n")
}

/// Assert the compiled expansion matches the interpreter oracle for `src`.
fn assert_agree(src: &str) {
    let oracle = interp_expand(src);
    let compiled = compiled_expand(src);
    assert_eq!(
        compiled, oracle,
        "compiled macro expansion diverged from the interpreter oracle\n--- src ---\n{src}"
    );
}

#[test]
fn identity_agrees() {
    assert_agree(
        "Package \"demo:m@0.1.0\"\n\
         DefMacro identity {x} x\n\
         Identity (add 1 2)\n",
    );
}

#[test]
fn unless_via_quasi_agrees() {
    assert_agree(
        "Package \"demo:m@0.1.0\"\n\
         DefMacro unless {cond body}\n\
           Quasi If Unquote(cond) {} Unquote(body)\n\
         Unless false \"ran\"\n",
    );
}

#[test]
fn splice_into_sequence_agrees() {
    assert_agree(
        "Package \"demo:m@0.1.0\"\n\
         DefMacro wrap {items}\n\
           Quasi [before Splice(items) after]\n\
         Wrap [1 2 3]\n",
    );
}

#[test]
fn nested_quasi_agrees() {
    assert_agree(
        "Package \"demo:m@0.1.0\"\n\
         DefMacro outer {x}\n\
           Quasi Quasi (a Unquote(Unquote(x)))\n\
         Outer y\n",
    );
}

#[test]
fn gensym_agrees() {
    // A single gensym use: both the interpreter and the freshly-instantiated
    // component start their counter at 0, so each yields `g0-gen`.
    assert_agree(
        "Package \"demo:m@0.1.0\"\n\
         DefMacro fresh {} gensym()\n\
         Fresh\n",
    );
}

#[test]
fn char_forms_agree() {
    // A char arrives as an argument (lifted from a wire `char-val`), is unquoted,
    // and a char literal is quoted — all must round-trip as chars, not ints.
    assert_agree(
        "Package \"demo:m@0.1.0\"\n\
         DefMacro chars {x} Quasi [Unquote(x) 'z']\n\
         Chars 'a'\n",
    );
}

#[test]
fn payloaded_variant_output_agrees() {
    // A macro body that yields a payloaded variant value `ok(x)` must serialize
    // back like value_to_form: a 1-argument call `(ok, x)`.
    assert_agree(
        "Package \"demo:m@0.1.0\"\n\
         DefMacro mkok {x} ok(x)\n\
         Mkok y\n",
    );
}

#[test]
fn qualified_symbol_output_agrees() {
    // A quoted qualified symbol must round-trip as a `qsym`, not a flat `sym`.
    assert_agree(
        "Package \"demo:m@0.1.0\"\n\
         DefMacro q {} Quote dsl/elem\n\
         Q\n",
    );
}

#[test]
fn expand_inside_macro_agrees() {
    // `both` builds an `and`-macro call form and `expand`s it one step from
    // inside its own body — exercising the in-macro `expand` builtin.
    assert_agree(
        "Package \"demo:m@0.1.0\"\n\
         DefMacro and {a b} Quasi If Unquote(a) Unquote(b) false\n\
         DefMacro both {x} expand(Quasi And Unquote(x) Unquote(x))\n\
         Both p\n",
    );
}

#[test]
fn try_let_from_spec_agrees() {
    // §7.2 try-let: exercises rec-key/rec-val, a Let in the macro body, and a
    // quasi Match with nested unquotes.
    assert_agree(
        "Package \"demo:m@0.1.0\"\n\
         DefMacro try-let {binding body}\n\
           Let {name: rec-key(binding) expr: rec-val(binding)}\n\
             Quasi Match Unquote(expr) [\n\
               (ok(Unquote(name))  Unquote(body))\n\
               (err(e)             err(e))\n\
             ]\n\
         Try-let {h: half(n)} ok(h)\n",
    );
}
