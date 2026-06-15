# Step 01 — Reader: produce tuple-calls

**Read `dev-notes/tuple-calls.md` (the index) first** for the full model,
build-green strategy, and branch/commit conventions. This step changes only the
**reader** so that every call form reads to a flat `Node::Tup` whose first
element is the head. Do not touch `interp.rs`, `wit.rs`, or `emit.rs` here.

Work on the `tuple-calls` branch. Commit as you go. Do not open a PR.

## Files

- `src/reader.rs` — all the real work.
- `src/form.rs` — **no change** in this step. Keep the `Node::Call` variant.
- `src/printer.rs` — verify only (see below); likely no change.

## Background you need

The form tree node enum (`src/form.rs`) currently has both:

```rust
Call(NodeId, NodeId),   // head, payload   <-- we stop producing this
Tup(Vec<NodeId>),       // (a, b, c …)      <-- now also the call form
```

After this step the reader must **never** emit `Node::Call`; every parenthesized
form and every call (attached-paren or TitleCase) becomes a `Node::Tup` with the
head as `items[0]`. Keep the `Node::Call` enum variant in `form.rs` for now (the
build-green strategy in the index explains why); just stop constructing it.

## Required reader changes (`src/reader.rs`)

1. **Attachment is paren-only.** `attached_opener` currently returns an attached
   `(`, `[`, or `{`. Change it so only `(` attaches. When a `[` or `{`
   *immediately* follows an identifier (no whitespace — same adjacency test,
   `span.start == end`), return a **read error** with a migration message, e.g.:
   - attached `[`: ``"list call sugar was removed: write `name([...])` instead of `name[...]`"``
   - attached `{`: ``"record call sugar was removed: write `name({...})` instead of `name{...}`"``

   Implement this in the identifier paths (`maybe_call` for `Sym`/non-title
   `Qsym`, and `title_form` for TitleCase). The cleanest approach: have a small
   helper that peeks the next token; if it is an attached `[` or `{`, raise the
   error; if attached `(`, proceed to parse the paren payload; otherwise no
   attachment. Free-standing `[…]`/`{…}` (with whitespace, or not following an
   identifier) are still parsed normally as list/record/flags.

2. **`maybe_call` (identifier head).** When an attached `(` is present, parse the
   parenthesized items and build `Tup([head, ...items])` — i.e. **prepend the
   head** to the paren contents. Examples:
   - `foo(a b)` ⇒ `Tup[foo, a, b]`
   - `foo(a)`   ⇒ `Tup[foo, a]`
   - `foo()`    ⇒ `Tup[foo]`  (0-arg call; a single-element tuple)
   With no attachment, return the bare head node (a variable reference), as
   today.

   Note: previously the payload was wrapped (0 ⇒ empty list, 1 ⇒ the value,
   2+ ⇒ a tuple) and stored as the second field of `Call`. **Do not wrap
   anymore** — splice the paren items directly after the head.

3. **`title_form` (TitleCase head).** Same idea. For an explicit attached payload
   `If(c t e)`, prepend the head: `Tup[if-MACRO, c, t, e]`. For the arity-driven
   paren-free form `If c t e`, read `arity` following forms and build
   `Tup[if-MACRO, c, t, e]`. A 0-arity TitleCase form (if any) ⇒ `Tup[head]`.
   The macro-arity lookup logic is unchanged; only the node you build changes
   (head prepended into a `Tup`, never a `Call`).

4. **`parse_payload` is no longer needed as a wrapper.** The attached-paren case
   should just return the list of items (to be prepended to the head by the
   caller). Attached `[`/`{` no longer reach here (they error in step 1 above).
   Restructure as needed — e.g. add a `parse_paren_items()` returning
   `Vec<NodeId>` for attached `(`.

5. **Free-standing parens (`parse_parens`).** Remove the grouping collapse.
   - `()`        ⇒ `Tup[]` (empty tuple; allowed — it errors only at eval time)
   - `(a)`       ⇒ `Tup[a]`  (0-arg call of `a`; **not** transparent grouping)
   - `(a b …)`   ⇒ `Tup[a, b, …]`
   So `parse_parens` becomes: parse items until `)`, return `Tup(items)`
   unconditionally (including the empty and single-element cases).

6. **`register_if_def_macro`.** This currently expects
   `Node::Call(head, payload)` where `head` is `def-macro-MACRO` and `payload`
   is a `Tup` of 3 (name, params, body). Update it to read the new shape: a
   top-level `DefMacro name {params} body` now reads as
   `Tup[def-macro-MACRO, name, params, body]` (a 4-element tuple). Extract
   `name = items[1]`, `params = items[2]`; compute arity from the params node
   (`Flg` ⇒ `len`, `Rec` ⇒ `len`) exactly as today; register `"{name}-MACRO"`.

7. **Spans.** Preserve span bookkeeping as the existing code does (start of head
   to end of closing delimiter). Nothing special — just keep spans sensible for
   diagnostics.

## `src/printer.rs` (verify only)

`Node::Tup` already prints as `(a, b, c)` with commas, which is exactly the new
canonical call form. **Leave the existing `Node::Call` arm in place** (it is now
dead but must still compile until Step 06). Do not change the `Tup` arm. Confirm
by eye that printing `Tup[foo, 1, str]` yields `(foo, 1, "…")`.

## Verification

- `cargo build` must succeed (the `Node::Call` variant still exists; reader no
  longer constructs it).
- Add or run a quick reader smoke check. The simplest is a throwaway unit test
  or a `cargo run -- ...`/REPL check that these read without error and round-trip
  through the printer to the expected canonical text:
  - `foo(1 "baz")` ⇒ prints `(foo, 1, "baz")`
  - `foo()`        ⇒ prints `(foo)`
  - `foo({bar: baz})` ⇒ prints `(foo, {bar: baz})`
  - `foo([1 2])`   ⇒ prints `(foo, [1, 2])`
  - `If c t e`     ⇒ prints `(if-MACRO, c, t, e)`
  - `foo[1 2]`     ⇒ **read error** mentioning `foo([1 2])`
  - `foo{a: 1}`    ⇒ **read error** mentioning `foo({a: 1})`
  If you add a temporary test, you may remove it before committing or keep it if
  it fits the existing test style; do not leave dead scaffolding.
- Do **not** expect `cargo test` (examples / http) to pass — runtime call
  semantics arrive in later steps. That is expected.

## Commit

Commit the reader change, e.g.:
`feat(reader): read calls as head-prefixed tuples; drop [ ] and { } call sugar`
