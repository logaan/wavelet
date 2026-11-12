use wavelet::expand::{ForeignExpander, expand_file};
use wavelet::macrodep::FileExpander;
use wavelet::printer::print;
use wavelet::reader::read_file;

fn local_expand(src: &str) -> String {
    let (arena, roots) = read_file(src).expect("read");
    let (out, new_roots) = expand_file(arena, &roots, None).expect("local expansion");
    new_roots
        .iter()
        .map(|&r| print(&out, r))
        .collect::<Vec<_>>()
        .join("\n")
}

fn compiled_expand(src: &str) -> String {
    let (arena, roots) = read_file(src).expect("read");
    let mut fx = FileExpander::for_file(".", &arena, &roots).expect("file defines local macros");
    let (out, new_roots) = expand_file(arena, &roots, Some(&mut fx as &mut dyn ForeignExpander))
        .expect("compiled expand");
    new_roots
        .iter()
        .map(|&r| print(&out, r))
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_agree(src: &str) {
    let local = local_expand(src);
    let compiled = compiled_expand(src);
    assert_eq!(
        compiled, local,
        "compiled macro expansion diverged from the local macro expansion\n--- src ---\n{src}"
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
    assert_agree(
        "Package \"demo:m@0.1.0\"\n\
         DefMacro fresh {} gensym()\n\
         Fresh\n",
    );
}

#[test]
fn char_forms_agree() {
    assert_agree(
        "Package \"demo:m@0.1.0\"\n\
         DefMacro chars {x} Quasi [Unquote(x) 'z']\n\
         Chars 'a'\n",
    );
}

#[test]
fn payloaded_variant_output_agrees() {
    assert_agree(
        "Package \"demo:m@0.1.0\"\n\
         DefMacro mkok {x} ok(x)\n\
         Mkok y\n",
    );
}

#[test]
fn slash_symbol_normalization_agrees() {
    assert_agree(
        "Package \"demo:m@0.1.0\"\n\
         DefMacro q {} Quote dsl/elem\n\
         Q\n",
    );
}

#[test]
fn expand_inside_macro_agrees() {
    assert_agree(
        "Package \"demo:m@0.1.0\"\n\
         DefMacro and {a b} Quasi If Unquote(a) Unquote(b) false\n\
         DefMacro both {x} expand(Quasi And Unquote(x) Unquote(x))\n\
         Both p\n",
    );
}

#[test]
fn try_let_from_spec_agrees() {
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

#[test]
fn capturing_callbacks_in_macros_agree() {
    assert_agree(
        r#"Package "demo:m@0.1.0"
DefMacro wrap {items}
  Let {prefix: Quote before}
    map(Fn {item} Quasi [Unquote(prefix) Unquote(item)] items)
Wrap [1 2]
Wrap []
Wrap ["last"]
"#,
    );
}
