//! One-step interpreter macro expansion, retained as the differential **oracle**
//! (`CLAUDE.md`) the compiled macro components are validated against.
//!
//! A macro library written in Wavelet is a `.wvl` file whose top level is a
//! `Package` declaration plus `DefMacro`s (and nothing the file exports as a
//! runtime function — see [`crate::macrobuild`] for the build trigger). Producing
//! one now **compiles each macro body to wasm** (strategy B, via
//! [`crate::emit::emit_macro_component`]) instead of bundling this interpreter
//! (the former strategy A).
//!
//! ## Why this stays in the (non-gated) shared crate
//!
//! `interp.rs` is the semantics oracle (`CLAUDE.md`): a macro expanded by the
//! compiled component must mean *exactly* what it would mean expanded by the
//! interpreter via [`crate::expand::expand_file`]. The "register the file's
//! `DefMacro`s, then run one expansion step for a named macro over a call form"
//! logic is written **once**, here, and used as that reference — the differential
//! harness (`tests/macro_differential.rs`) expands a corpus both ways and asserts
//! they agree. Because it speaks only `reader`/`interp`/`expand`/`value`/`form`,
//! it is **not** `cfg(not(target_arch = "wasm32"))`-gated.
//!
//! ## Equivalence with local expansion (the pinned contract)
//!
//! [`crate::expand::expand_file`] expands a local macro call by looking the
//! macro up in an env seeded with the builtins and the file's `DefMacro`s, then
//! calling [`crate::interp::Interp::expand_once`] with the call's argument forms
//! (`items[1..]`) and recursing on the result to fixpoint. A produced component
//! must reproduce a **single** such step (the consumer recurses to fixpoint on
//! its side — see [`crate::macrodep::FileExpander::expand_call`]). So
//! [`expand_one`] mirrors `expand_file`'s local-macro arm precisely: same env
//! seeding, same `expand_once`, same one-step contract.
//!
//! ## The args-tree contract (PINNED — shared with Steps 3/9)
//!
//! `expand`'s `args` is the **whole call form**: a `tup` whose element 0 is the
//! `<name>-MACRO` head and elements `1..` are the argument forms. [`expand_one`]
//! therefore reads the call tup's elements from index 1, exactly as the
//! hand-built fixture (`tests/fixtures/macros/src/lib.rs`) does.

use std::rc::Rc;

use crate::form::{Arena, Node, NodeId};
use crate::interp::Interp;
use crate::value::{Env, Value};

/// Build an [`Env`] seeded with the builtins and every `DefMacro` in `src`,
/// exactly the way [`crate::expand::expand_file`] seeds its local-macro env.
///
/// Returns the env plus the read arena (kept alive because each registered
/// `Value::Macro` closes over it). Errors as a `String` if the source fails to
/// read or a `DefMacro` fails to evaluate.
fn macro_env(src: &str) -> Result<(Env, Rc<Arena>), String> {
    let (arena, roots) = crate::reader::read_file(src).map_err(|e| e.to_string())?;
    let arena = Rc::new(arena);
    let interp = Interp::new();
    let env = Env::root();
    crate::builtins::install(&env);
    for &root in &roots {
        if is_def_macro(&arena, root) {
            interp.eval(&arena, root, &env).map_err(|e| e.to_string())?;
        }
    }
    Ok((env, arena))
}

fn is_def_macro(arena: &Arena, id: NodeId) -> bool {
    if let Node::Tup(items) = arena.node(id) {
        if let Some(&head) = items.first() {
            return matches!(arena.node(head), Node::Sym(s) if s == "defmacro-MACRO");
        }
    }
    false
}

/// The `(name, arity)` pairs a macro file publishes — the data
/// `wavelet:meta/macros`'s `manifest` returns.
///
/// This computes the *same* arities the reader's `register_if_def_macro` does
/// (the `{params}` field count of each top-level `DefMacro`), but reported with
/// the **unsuffixed** macro name (`unless`, not `unless-MACRO`) to match the
/// manifest contract the consumer registers from
/// ([`crate::macrodep`]). Reading the source is enough — no evaluation — so a
/// macro whose *body* would fail still appears in the manifest.
pub fn manifest(src: &str) -> Result<Vec<(String, u32)>, String> {
    let (arena, roots) = crate::reader::read_file(src).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for &root in &roots {
        // A top-level `DefMacro name {params} body` reads as the 4-tuple
        // `Tup[defmacro-MACRO, name, params, body]` (mirrors the reader).
        let Node::Tup(items) = arena.node(root) else { continue };
        if items.len() != 4 {
            continue;
        }
        let Node::Sym(h) = arena.node(items[0]) else { continue };
        if h != "defmacro-MACRO" {
            continue;
        }
        let Node::Sym(name) = arena.node(items[1]) else { continue };
        let arity = match arena.node(items[2]) {
            Node::Flg(names) => names.len(),
            Node::Rec(fields) => fields.len(),
            _ => continue,
        };
        out.push((name.clone(), arity as u32));
    }
    Ok(out)
}

/// Run **one** expansion step of the macro named `name` (unsuffixed) over the
/// call form `call_id` in `call_arena`, returning the rewritten form in a fresh
/// arena.
///
/// `call_id` must be the whole call `tup` (head + args, the pinned args-tree
/// contract); the macro receives `items[1..]` as its argument forms. The macro
/// itself is looked up under its `<name>-MACRO` key in an env seeded from `src`
/// — so this is byte-for-byte the local-macro arm of
/// [`crate::expand::expand_file`], guaranteeing component-expansion equals local
/// expansion.
///
/// Errors as a `String` (surfaced to the consumer as the `expand` `err` arm):
/// an unknown macro name, or an `expand_once` failure (wrong arity, a body that
/// errors), with the macro author's message preserved.
pub fn expand_one(
    src: &str,
    name: &str,
    call_arena: Arena,
    call_id: NodeId,
) -> Result<(Arena, NodeId), String> {
    let (env, _src_arena) = macro_env(src)?;
    let key = format!("{name}-MACRO");
    let Some(Value::Macro(mac)) = env.lookup(&key) else {
        return Err(format!("unknown macro `{name}`"));
    };
    // The pinned contract: `call_id` is the whole call tup; the macro's
    // arguments are its elements from index 1.
    let Node::Tup(items) = call_arena.node(call_id) else {
        return Err("args tree root is not a call (tup)".to_string());
    };
    let args: Vec<NodeId> = items[1..].to_vec();
    // `expand_once` takes the call's arena as an `Rc<Arena>`; we own it, so move
    // it in. The macro body's own arena is the source arena it closed over.
    let call_arena = Rc::new(call_arena);
    let interp = Interp::new();
    let (out, root) = interp
        .expand_once(&mac, &call_arena, &args)
        .map_err(|e| format!("expanding `{name}`: {e}"))?;
    // `expand_once` always returns its result in a fresh, uniquely-owned arena.
    let out = Rc::try_unwrap(out)
        .map_err(|_| "internal: expansion arena unexpectedly shared".to_string())?;
    Ok((out, root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printer::print;

    const LIB: &str = "\
Package \"demo:macros@0.1.0\"\n\
DefMacro identity {x} x\n\
DefMacro unless {cond body}\n\
  Quasi If Unquote(cond) {} Unquote(body)\n\
";

    #[test]
    fn manifest_reports_unsuffixed_names_and_arities() {
        let mut got = manifest(LIB).expect("manifest");
        got.sort();
        assert_eq!(
            got,
            vec![("identity".to_string(), 1u32), ("unless".to_string(), 2u32)]
        );
    }

    /// Read a call form (the whole call tup) for `expand_one`.
    fn call(src: &str) -> (Arena, NodeId) {
        let (arena, roots) = crate::reader::read_file(src).expect("read call");
        (arena, *roots.last().unwrap())
    }

    #[test]
    fn expand_one_identity_returns_argument() {
        let (a, id) = call(&format!("{LIB}identity(add(1 2))"));
        let (out, root) = expand_one(LIB, "identity", a, id).expect("identity expands");
        assert_eq!(print(&out, root), "(add, 1, 2)");
    }

    #[test]
    fn expand_one_unless_expands_via_quasiquote() {
        let (a, id) = call(&format!("{LIB}unless(false \"ran\")"));
        let (out, root) = expand_one(LIB, "unless", a, id).expect("unless expands");
        assert_eq!(print(&out, root), r#"(if-MACRO, false, {}, "ran")"#);
    }

    #[test]
    fn expand_one_unknown_macro_errors() {
        let (a, id) = call(&format!("{LIB}nope()"));
        let err = expand_one(LIB, "nope", a, id).expect_err("unknown errors");
        assert!(err.contains("nope"), "unexpected: {err}");
    }

    /// The same `DefMacro` expanded locally (via `expand_file`) and via
    /// `expand_one` must agree — the semantics-oracle equivalence the step is
    /// about, checked here with no component in the loop.
    #[test]
    fn expand_one_matches_local_expand_file() {
        // TitleCase `Unless` so the reader recognises it as a macro use (head
        // `unless-MACRO`) and `expand_file` actually expands it locally.
        let src = format!("{LIB}Unless false \"ran\"");
        let (arena, roots) = crate::reader::read_file(&src).expect("read");
        let (out, new_roots) =
            crate::expand::expand_file(arena, &roots, None).expect("local expand");
        let local = print(&out, *new_roots.last().unwrap());

        let (a, id) = call(&src);
        let (cout, croot) = expand_one(LIB, "unless", a, id).expect("expand_one");
        // `expand_file` recurses to fixpoint; `expand_one` is a single step. For
        // `unless` the single step is already a fixpoint (`If` is a core macro
        // head, not a user macro), so the two coincide.
        assert_eq!(print(&cout, croot), local);
    }
}
