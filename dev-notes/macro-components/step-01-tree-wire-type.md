# Step 1 — The `wavelet:meta/code` `tree` wire type + arena ↔ wire conversion

- [ ] Done

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

_(fill in: where the WIT landed, the exact `Tree`/`Node` Rust shape, the
reachable-vs-whole-arena decision, how docs/spans were handled, and anything
Step 2/3 must know to map this to runtime `Val`s.)_
