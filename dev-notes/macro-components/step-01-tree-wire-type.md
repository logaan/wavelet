# Step 1 — The `wavelet:meta/code` `tree` wire type + arena ↔ wire conversion

- [x] Done

> **Read first:** `dev-notes/macro-components.md` (the index — driving rules and
> the verbatim subagent rules), plus `dev-notes/design.md` §6.2 ("Code as a WIT
> type"). This is the first step, so it also **creates the `macro-components`
> integration branch off `origin/main`** that every later step builds on.

## Context you need

Homoiconicity has to survive the component boundary, so "a form" is itself a WIT
type defined in `wavelet:meta` (design.md §6.2):

```wit
package wavelet:meta@0.1.0;

interface code {
  type node-id = u32;
  variant node {
    bool-val(bool),
    int-val(s64),
    dec-val(f64),
    char-val(char),
    str-val(string),
    sym(string),
    qsym(tuple<string, string>),
    call(tuple<node-id, node-id>),     // head, payload
    tup(list<node-id>),
    lst(list<node-id>),
    rec(list<tuple<string, node-id>>),
    flg(list<string>),
  }
  record tree {
    nodes: list<node>,
    root: node-id,
    spans: list<tuple<u32, u32>>,      // source offsets, parallel to nodes
  }
}
```

**Key fact:** the in-memory `src/form.rs` `Arena`/`Node`/`NodeId` already mirror
this variant set almost exactly (`Bool`, `Int`, `Dec`, `Char`, `Str`, `Sym`,
`Qsym`, `Call`, `Tup`, `Lst`, `Rec`, `Flg`; an arena `nodes`/`spans` plus a
root). So this step is **not** about inventing a representation — it is about
pinning the *canonical* wire shape and converting between `form::Arena` and it.

## Goal

Establish the `tree` wire type as a single source of truth and implement a
lossless, well-tested conversion between an in-memory `(form::Arena, root:
NodeId)` and the canonical-ABI `tree` value. Nothing executes a component yet
(that's Step 2) — this step only produces the data the boundary will carry.

## Scope

- **The WIT text.** Add `wavelet:meta@0.1.0` with the `code` interface (the
  `node`/`tree`/`node-id` definitions above) as a checked-in `.wit` somewhere the
  later steps and the component runtime can find it (e.g. `wit/meta/code.wit`, or
  alongside the other vendored WIT). Match the variant set to `form::Node`
  exactly; if any in-memory variant has no design.md counterpart (or vice versa),
  call it out in the handoff notes rather than silently diverging.
- **A Rust mirror of the wire type** — a `meta`/`code` module (e.g.
  `src/meta.rs`, native-only, gated `#[cfg(not(target_arch = "wasm32"))]` like
  `emit`/`build`/`wit`/`tools`) with plain Rust structs/enums for `Node`/`Tree`
  matching the WIT, **independent of** any wasm-runtime crate's generated types
  (Step 3 maps these to/from the runtime's dynamic `Val`s).
- **Conversion both ways:**
  - `arena_to_tree(&form::Arena, root: NodeId) -> Tree` — flatten reachable
    nodes into the wire arena. Decide and document whether the output is the
    whole arena or just the sub-tree reachable from `root` (recommend: reachable
    sub-tree, re-indexed, so callers can pass any node).
  - `tree_to_arena(&Tree) -> (form::Arena, NodeId)` — rebuild an arena + root.
- **Round-trip tests.** `arena → tree → arena` is structurally identity for every
  node variant (including nested `Call`/`Tup`/`Lst`/`Rec`/`Qsym`/`Flg`), and
  spans survive. Use existing reader output as fixtures (`reader::read_file` on a
  handful of forms) so the test exercises real trees, not hand-built ones.

## Watch out for

- **Integer width.** `form::Node::Int` is `i64`; the wire `int-val` is `s64`.
  Confirm they agree and document any narrowing.
- **`Qsym`.** `form::Node::Qsym(String, String)` ↔ wire `qsym(tuple<string,
  string>)`.
- **Doc comments.** `Arena` carries a sparse `docs` map (`///` comments, §2.1);
  the wire `tree` in §6.2 has no doc channel. Decide whether docs are dropped at
  the boundary (recommend: yes, dropped — note it) or whether the wire type needs
  extending. Do **not** silently lose information without recording the decision.
- **Spans** are `(u32, u32)` source offsets parallel to `nodes`; preserve them
  through the round trip even though foreign components will usually ignore them.

## Done when

`cargo test` passes; `meta::arena_to_tree`/`tree_to_arena` exist with round-trip
tests over real reader output; the `wavelet:meta@0.1.0` `code` WIT is checked in
and matches both `form::Node` and design.md §6.2.

## Handoff notes

### Where things landed

- **WIT**: `wit/meta/code.wit` — a new top-level `wit/` dir at the repo root
  (there was no pre-existing vendored-WIT dir; `witdep.rs` only handles a
  per-build `wit/` for synthesized worlds, and the only other `.wit` files are
  test fixtures). It declares `package wavelet:meta@0.1.0;` with `interface
  code` (`node-id`, `node` variant, `tree` record). A unit test
  (`meta::tests::meta_code_wit_parses`) parses it with `wit-parser` so it can't
  silently rot. Steps 2/3 can point a component runtime / `Resolve` at
  `wit/meta/`.
- **Rust mirror + conversions**: `src/meta.rs`, registered in `src/lib.rs` as
  `pub mod meta;` gated `#[cfg(not(target_arch = "wasm32"))]` (like
  `emit`/`build`/`wit`/`tools`). It depends only on `crate::form` — no
  wasm-runtime crate types.

### The `Tree` / `Node` Rust shape

```rust
pub enum Node {
    BoolVal(bool), IntVal(i64), DecVal(f64), CharVal(char), StrVal(String),
    Sym(String), Qsym(String, String),          // (alias, name)
    Tup(Vec<NodeId>), Lst(Vec<NodeId>),
    Rec(Vec<(String, NodeId)>), Flg(Vec<String>),
}
pub struct Tree { pub nodes: Vec<Node>, pub root: NodeId, pub spans: Vec<(u32,u32)> }
```

`NodeId` is reused from `form::NodeId` (= `u32`). The `*Val` variant names match
the WIT labels (`bool-val`, `int-val`, …) so the canonical-ABI mapping in Step 3
is mechanical: the WIT `node` variant case order is exactly
`BoolVal, IntVal, DecVal, CharVal, StrVal, Sym, Qsym, Tup, Lst, Rec, Flg`
(discriminants 0..=10), and `tree` is `{ nodes, root, spans }`.

### Conversions

- `arena_to_tree(&form::Arena, root: NodeId) -> Tree`
- `tree_to_arena(&Tree) -> (form::Arena, NodeId)`

**Decision: reachable sub-tree, re-indexed (NOT the whole arena).** A
`reader::read_file` arena holds many top-level roots; a macro call only ships
one form. `arena_to_tree` walks just the sub-tree reachable from `root`,
allocating children before parents, so the result is dense and self-contained
and callers can pass any node. `tree_to_arena` copies the node table verbatim
(keeping the wire ids), so it round-trips any well-formed `Tree`, not just ones
this module produced.

### Variant / payload decisions (read these before Step 3)

- **No `call` node.** The header snippet in this step file (and the index
  `macro-components.md`) shows a `call(tuple<node-id, node-id>)` variant, but
  that **diverges from both `form::Node` and design.md §6.2**, neither of which
  has a `Call`. Per §6.2 "a call is just a `tup` whose first element is the
  head, so there is no separate `call` node." I matched `form::Node` and §6.2:
  **the WIT has no `call` variant; a call is a `Tup` whose head is `items[0]`.**
  If Step 3+ wants an explicit `call` node it must be added to `form::Node`
  first (interpreter is the oracle) — don't add it only on the wire.
- **Integer width.** `form::Node::Int` is `i64`; wire `int-val` is `s64`. Same
  width — **no narrowing**, direct copy.
- **Docs: dropped at the boundary (as recommended).** The current `form::Arena`
  (`src/form.rs`) is just `{ nodes, spans }` — there is **no `docs` map** in the
  code today (the step's caution predates/anticipated one). So there is nothing
  to drop, but to be explicit: the wire `tree` carries no doc channel, and if a
  `docs` map is ever added to `Arena` it must NOT be expected to survive a
  `tree` round trip without extending the WIT.
- **Spans preserved.** `(u32, u32)` source offsets are carried through both
  directions, parallel to `nodes`. Round-trip tests assert spans survive.

### Tests

`src/meta.rs` `#[cfg(test)]`: round-trips over **real `reader::read_file`
output** (atoms incl. bool/int/dec/char/str/sym, qsym, tuple/list/record/flags
incl. empty, deeply nested mix), a reachable-sub-tree density check (ship inner
`(b c)` from `(a (b c) d)` → 3 dense nodes), and the WIT-parses guard.
Structural equality compares shape+payload+spans (ids legitimately differ after
re-indexing). `cargo test` is green (whole suite).

### Note for whoever writes fixtures next

This conversion makes **no assumption about how macro heads are read/expanded**
(TitleCase or otherwise) — it just maps `form::Node` variants to/from the wire
arena, so it is unaffected by changes to macro-expansion reading rules. The only
place reader rules touched this step was fixture authoring: on the branch base
(`origin/main` @ 23e0c31) a TitleCase head triggers a macro-arity lookup that
fails unless the arity is registered, so the qsym fixture uses kebab-case names.
The `Qsym` round trip is purely on its two strings, so case is irrelevant to
coverage; pick whatever the reader accepts at the time.
