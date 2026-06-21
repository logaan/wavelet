//! Ahead-of-time macro expansion (§2.4): rewrite a file's form tree with all
//! user macros expanded, so later pipeline stages (WIT synthesis, the wasm
//! emitter) never see them. The interpreter still expands lazily at eval
//! time; this pass exists for the compile pipeline.
//!
//! `DefMacro` forms are evaluated (registering the macro) and dropped from
//! the output. Macro bodies run in an environment containing the builtins
//! and previously defined macros only — calling file-local *functions* at
//! expand time is future work (macro components, §6.3).
//!
//! ## Local *and* foreign macros (§6.3)
//!
//! A macro head can resolve two ways at expand time:
//!
//! 1. a **local** `Value::Macro` in the env (a `DefMacro` in this file), or
//! 2. a **foreign** macro exported by an imported `wavelet:meta/macros`
//!    component (`Import {… macros: true}`).
//!
//! Local macros expand via [`Interp::expand_once`]; foreign macros expand by
//! shipping the call form across the component boundary and lifting the result
//! back. Because the component runtime is native-only, the foreign path is
//! reached through the wasm-safe [`ForeignExpander`] seam: native code (see
//! [`crate::macrodep`]) supplies an implementation; the wasm playground passes
//! `None` and only ever sees local macros. Both kinds recurse through
//! `expand_form` so an expansion that itself contains a macro call (local *or*
//! foreign) is expanded to fixpoint — exactly as a local macro would be.

use std::rc::Rc;

use crate::form::{Arena, Node, NodeId};
use crate::interp::Interp;
use crate::value::{Env, Value};

/// A wasm-safe seam for expanding *foreign* macros — those exported by an
/// imported `wavelet:meta/macros` component rather than defined locally.
///
/// `expand.rs` is compiled for both native and wasm32, but the component
/// runtime that actually runs a foreign macro is native-only. Rather than gate
/// the expander itself, the foreign-expand capability is injected through this
/// trait: the native compiler ([`crate::macrodep`]) implements it over a
/// resolver + the file's imports; the wasm playground passes `None`, so only
/// local macros exist there.
///
/// The trait deliberately speaks only `Arena`/`NodeId` (never the native-only
/// `meta::Tree`), so the marshalling stays on the native side and this seam
/// imposes no native-only types on `expand.rs`.
pub trait ForeignExpander {
    /// Expand a macro call whose head is `name` (the head symbol **without** the
    /// `-MACRO` suffix) and whose call form is `call_id` in `arena`. `alias` is
    /// `Some` for a qualified `Alias/Name` head — routing the call to the import
    /// bound to that alias, even when the bare name is ambiguous — and `None` for
    /// a bare head (resolved by scanning the imports).
    ///
    /// Returns:
    /// - `None` if no foreign macro owns `name` (for a bare head) or no import
    ///   aliased `alias` owns it (for a qualified head) — the caller falls
    ///   through to local-macro lookup and ordinary descent;
    /// - `Some(Ok((arena, root)))` with the expansion in a fresh arena on
    ///   success;
    /// - `Some(Err(msg))` if a foreign macro owns `name` but expanding it
    ///   failed (the message is the macro author's, surfaced verbatim).
    fn expand_call(
        &mut self,
        alias: Option<&str>,
        name: &str,
        arena: &Arena,
        call_id: NodeId,
    ) -> Option<Result<(Arena, NodeId), String>>;
}

/// Expand a file's form tree with all macros (local and foreign) run to
/// fixpoint. `foreign` injects the foreign-macro capability ([`ForeignExpander`]);
/// pass `None` for a local-macros-only expansion (the wasm playground, and any
/// caller with no macro imports).
pub fn expand_file(
    arena: Arena,
    roots: &[NodeId],
    mut foreign: Option<&mut dyn ForeignExpander>,
) -> Result<(Arena, Vec<NodeId>), String> {
    let interp = Interp::new();
    let env = Env::root();
    crate::builtins::install(&env);

    let arena = Rc::new(arena);
    let mut out = Arena::new();
    let mut new_roots = Vec::new();
    for &root in roots {
        if is_def_macro(&arena, root) {
            interp.eval(&arena, root, &env).map_err(|e| e.to_string())?;
            continue;
        }
        new_roots.push(expand_form(
            &interp,
            &env,
            &arena,
            root,
            &mut out,
            &mut foreign,
        )?);
    }
    Ok((out, new_roots))
}

fn is_def_macro(arena: &Arena, id: NodeId) -> bool {
    if let Node::Tup(items) = arena.node(id) {
        if let Some(&head) = items.first() {
            return matches!(arena.node(head), Node::Sym(s) if s == "defmacro-MACRO");
        }
    }
    false
}

/// `foreign` is threaded as `&mut Option<&mut dyn …>` (rather than
/// `Option<&mut dyn …>`) so each recursive call reborrows it for a *fresh,
/// shorter* lifetime — passing the inner reference by value would tie every
/// reborrow to the caller's full lifetime and the borrow checker would reject
/// the sequential per-child recursion.
fn expand_form(
    interp: &Interp,
    env: &Env,
    arena: &Rc<Arena>,
    id: NodeId,
    out: &mut Arena,
    foreign: &mut Option<&mut dyn ForeignExpander>,
) -> Result<NodeId, String> {
    if let Node::Tup(items) = arena.node(id) {
        if let Some(&head) = items.first() {
            if let Node::Sym(name) = arena.node(head) {
                // forms quoted at runtime are not expanded at compile time —
                // for both local and foreign macros (the call form under a
                // quote is data, not a macro use).
                if name == "quote-MACRO" || name == "quasi-MACRO" {
                    return Ok(copy_form(arena, id, out));
                }
                // (1) Local macro: a `DefMacro` in this file. On the native path
                // a `ForeignExpander` is present and owns the file's compiled
                // local-macro component (strategy B), so the macro expands as
                // wasm — no interpreter. The wasm playground passes no expander,
                // so it falls through to `Interp::expand_once` (which also stays
                // the differential oracle).
                if let Some(Value::Macro(mac)) = env.lookup(name) {
                    let bare = name.trim_end_matches("-MACRO");
                    if let Some(fx) = foreign.as_deref_mut() {
                        if let Some(result) = fx.expand_call(None, bare, arena, id) {
                            let (expanded_arena, expanded) =
                                result.map_err(|e| format!("expanding `{bare}`: {e}"))?;
                            let expanded_arena = Rc::new(expanded_arena);
                            return expand_form(
                                interp, env, &expanded_arena, expanded, out, foreign,
                            );
                        }
                    }
                    let (expanded_arena, expanded) = interp
                        .expand_once(&mac, arena, &items[1..])
                        .map_err(|e| format!("expanding `{bare}`: {e}"))?;
                    return expand_form(interp, env, &expanded_arena, expanded, out, foreign);
                }
                // (2) Foreign macro (bare head): exported by an imported macro
                // component. The whole call form (head + args) is shipped across
                // the boundary; the component's `expand` rewrites it. Recurse so
                // the expansion is itself expanded to fixpoint.
                if let Some(fx) = foreign.as_deref_mut() {
                    let macro_name = name.trim_end_matches("-MACRO");
                    if let Some(result) = fx.expand_call(None, macro_name, arena, id) {
                        let (expanded_arena, expanded) = result.map_err(|e| {
                            format!("expanding `{macro_name}`: {e}")
                        })?;
                        let expanded_arena = Rc::new(expanded_arena);
                        return expand_form(
                            interp,
                            env,
                            &expanded_arena,
                            expanded,
                            out,
                            foreign,
                        );
                    }
                }
            } else if let Node::Qsym(alias, name) = arena.node(head) {
                // (3) Qualified foreign macro (`Alias/Name`): route to the import
                // bound to `alias` specifically — this resolves even when the
                // bare name is ambiguous across imports (§6.3). Qualified heads
                // are never local (`DefMacro` registers a bare symbol).
                if let Some(fx) = foreign.as_deref_mut() {
                    let macro_name = name.trim_end_matches("-MACRO");
                    if let Some(result) = fx.expand_call(Some(alias), macro_name, arena, id) {
                        let (expanded_arena, expanded) = result.map_err(|e| {
                            format!("expanding `{alias}/{macro_name}`: {e}")
                        })?;
                        let expanded_arena = Rc::new(expanded_arena);
                        return expand_form(
                            interp,
                            env,
                            &expanded_arena,
                            expanded,
                            out,
                            foreign,
                        );
                    }
                }
            }
        }
    }
    descend(interp, env, arena, id, out, foreign)
}

fn descend(
    interp: &Interp,
    env: &Env,
    arena: &Rc<Arena>,
    id: NodeId,
    out: &mut Arena,
    foreign: &mut Option<&mut dyn ForeignExpander>,
) -> Result<NodeId, String> {
    let span = arena.span(id);
    let node = match arena.node(id).clone() {
        // A Tup's head is just its first element (macro heads are intercepted
        // by `expand_form` before reaching here), so expand every element.
        Node::Tup(items) => {
            let mut kids = Vec::with_capacity(items.len());
            for x in items {
                kids.push(expand_form(interp, env, arena, x, out, foreign)?);
            }
            Node::Tup(kids)
        }
        Node::Lst(items) => {
            let mut kids = Vec::with_capacity(items.len());
            for x in items {
                kids.push(expand_form(interp, env, arena, x, out, foreign)?);
            }
            Node::Lst(kids)
        }
        Node::Rec(fields) => {
            let mut nf = Vec::with_capacity(fields.len());
            for (k, v) in fields {
                nf.push((k, expand_form(interp, env, arena, v, out, foreign)?));
            }
            Node::Rec(nf)
        }
        leaf => leaf,
    };
    Ok(out.add(node, span))
}

fn copy_form(arena: &Arena, id: NodeId, out: &mut Arena) -> NodeId {
    let span = arena.span(id);
    let node = match arena.node(id).clone() {
        Node::Tup(items) => Node::Tup(items.iter().map(|&x| copy_form(arena, x, out)).collect()),
        Node::Lst(items) => Node::Lst(items.iter().map(|&x| copy_form(arena, x, out)).collect()),
        Node::Rec(fields) => Node::Rec(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), copy_form(arena, *v, out)))
                .collect(),
        ),
        leaf => leaf,
    };
    out.add(node, span)
}
