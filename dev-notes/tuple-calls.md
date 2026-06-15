# Plan: switch the canonical call form from variants to tuples

This is the index for a multi-step change. Each step lives in
`dev-notes/tuple-calls/` and is written to be implemented by a **separate
sub-agent starting from a fresh context**. Read this index first, then your
assigned step file. Implement the steps **in order** — later steps assume the
earlier ones are already committed on the branch.

## What we are changing, and why

Today a Wavelet function call is, as data, a **variant case**: `print("hi")` is
the WAVE variant `print` carrying a payload, represented in the form tree as
`Node::Call(head, payload)`. We are switching calls to **tuples**: a call is a
WAVE tuple whose first element is the head.

```
// before (variant-shaped)            // after (tuple-shaped)
foo(1 "baz")   => foo((1, "baz"))     foo(1 "baz")  => (foo, 1, "baz")
foo()          => foo([])             foo()         => (foo)
foo({bar: x})  => foo({bar: x})       foo({bar: x}) => (foo, {bar: x})
foo([1 2])     => foo([1, 2])         foo([1 2])    => (foo, [1, 2])
```

The list/record **call sugar is removed**. `foo[bar baz]` and `foo{bar: baz}`
are no longer calls; users must write `foo([bar baz])` and `foo({bar: baz})`.

## The model (decided — do not re-litigate)

These decisions were made by the project owner. Implement them exactly.

1. **One node kind.** A parenthesized form `(a b c …)` is the single call/
   application/tuple node. We reuse `Node::Tup(Vec<NodeId>)` for it and **remove
   `Node::Call`**. (To keep the build green, the `Node::Call` enum variant is
   *kept* through the early Rust steps and deleted in the cleanup step — see
   "Build-green strategy" below.)

2. **Everything in evaluation position is a call.** Evaluating a `Node::Tup`
   means: take `items[0]` as the head (it must be a name — `Sym`/`Qsym`),
   resolve it, and apply. There is no "tuple literal evaluates to itself"
   anymore — a literal tuple **value** is obtained only via `Quote`, a builtin,
   or pattern binding. So `(1 2)` in eval position is an **error** ("1 is not
   callable"); `Quote (1 2)` yields the tuple value `(1, 2)`.

3. **Grouping is gone.** `(a)` is a **0-argument call** of `a`, not transparent
   grouping. `()` is the empty tuple `Tup[]` (an error if evaluated).

4. **Special forms and macros are tuples too.** `If c t e` reads to
   `(if-MACRO c t e)`, `Def x 1` to `(def-MACRO x 1)`, etc. The TitleCase paren-
   free *spelling* is unchanged; only the canonical/printed shape changes. An
   explicit payload `If(c t e)` reads identically to `If c t e` (both flatten to
   `(if-MACRO c t e)`).

5. **Argument bundling at a call.** A function still "takes exactly one value"
   (§4.2). The call bundles its evaluated arguments before applying:
   - 0 args ⇒ `Value::Tup(vec![])` (empty)
   - 1 arg  ⇒ that arg's value, directly
   - ≥2 args ⇒ `Value::Tup(args)`

   This reproduces the *old* payload exactly, so `bind_params` is unchanged:
   a record arg binds parameters by name, a list/tuple arg by order, a scalar
   binds the sole parameter. `(foo {a: 1 b: 2})` = named args; `(foo [1 2])` and
   `(foo 1 2)` = positional.

6. **Quote / data.** `form_to_value(Tup)` ⇒ `Value::Tup` (already true). A bare
   `Sym` ⇒ `Value::Variant(name, None)` (a symbol — unchanged). Therefore
   `Quote foo(1 2)` ⇒ `Value::Tup([Variant("foo",None), Int 1, Int 2])`, which
   prints `(foo, 1, 2)`. Quoted calls are **tuples**: `form-kind` returns
   `"tup"` for them (there is no `"call"` kind for quoted code anymore).

7. **Runtime variants still exist.** `ok` / `err` / `some` / `none` and enum
   cases are still `Value::Variant`. The inverse conversion is:
   - `value_to_form(Variant(name, None))` ⇒ `Node::Sym(name)`
   - `value_to_form(Variant(name, Some(p)))` ⇒ `Tup[Sym(name), value_to_form(p)]`
     (a 1-argument call form, e.g. `ok(x)` ⇒ `(ok, x)`).

8. **Pattern matching** disambiguates a `Node::Tup` pattern by the **scrutinee
   value**:
   - value is `Variant(case, payload)` and pattern is `Tup[Sym(case), …rest]`
     ⇒ variant-case pattern. Match `rest` against the payload using the same
     bundling: 0 rest ⇒ matches `Variant(case, None)`; 1 rest ⇒ match it against
     the payload; ≥2 rest ⇒ payload must be a `Value::Tup` matched element-wise.
   - value is `Value::Tup(vs)` of equal length ⇒ destructure element-wise.
   - otherwise ⇒ no match.
   A bare `Sym` pattern keeps today's rule: if it is bound to a payload-less
   variant case in scope and equal, match by equality; otherwise bind.

## Surface-syntax summary (for the reader)

| You write              | Reads to (form tree)              |
|------------------------|-----------------------------------|
| `foo`                  | `Sym(foo)` (variable reference)   |
| `foo(a b)` / `(foo a b)` | `Tup[foo, a, b]`                |
| `foo(a)` / `(foo a)`   | `Tup[foo, a]`                     |
| `foo()` / `(foo)`      | `Tup[foo]` (0-arg call)           |
| `()`                   | `Tup[]` (errors if evaluated)     |
| `If c t e`             | `Tup[if-MACRO, c, t, e]`          |
| `[a b]`                | `Lst[a, b]` (unchanged)           |
| `{k: v}`               | `Rec` (unchanged)                 |
| `{read write}`         | `Flg` (unchanged)                 |
| `foo[a b]`             | **read error** — sugar removed; use `foo([a b])` |
| `foo{k: v}`            | **read error** — sugar removed; use `foo({k: v})` |

Only `(` attaches to an identifier now. A `[` or `{` immediately following an
identifier is a read error whose message points users at the new spelling.
Free-standing `[…]` and `{…}` are unaffected.

## Build-green strategy (important for Rust steps 1–6)

Removing `Node::Call` is atomic across the whole crate, which would break the
build for every intermediate step. To keep `cargo build` green at every step:

- **Do not delete the `Node::Call` enum variant until Step 06.** Keep it in
  `form.rs` through Steps 01–05. Match arms for `Node::Call` in files you have
  not yet converted may remain — they are dead (the reader stops producing
  `Node::Call` in Step 01) but still compile.
- After Step 02, nothing constructs `Node::Call`, so the compiler emits a
  `dead_code` "variant never constructed" warning. The crate does **not**
  `deny(warnings)`, so this is harmless. Step 06 removes the variant and all
  residual arms to clear it.
- Expected verification per step:
  - Steps 01–05: `cargo build` stays green. Full `cargo test` is **not** green
    yet (examples still use old syntax; runtime call semantics land across these
    steps). Use the targeted smoke checks in each step file.
  - Step 06: `cargo build` green with `Node::Call` removed.
  - Step 07: `cargo test --test http` passes (templates build through emit).
  - Step 08: regenerate examples ⇒ **full `cargo test` green**.
  - Steps 09–11: docs / grammars / LSP (build the LSP crate separately).
  - Step 12: final full verification + open the PR.

## Branch & workflow conventions (every step)

- All steps land on **one shared branch: `tuple-calls`** (already created off
  `origin/main`; this plan is committed on it). Do **not** start a new divergent
  branch per step — continue on `tuple-calls` so each step builds on the last.
- **Commit as you go** with clear messages in the repo's style (`feat:`,
  `fix:`, `docs:`, `build:`…). Small, logical commits.
- **Do not open the PR** except in the final step (Step 12). Do not push to or
  merge `origin/main`.
- A language change is "done" only when downstream surfaces are updated — that
  is what Steps 07–11 cover. See `CLAUDE.md` ("A language change is not done
  until the downstream surfaces are checked").

## The reference semantics oracle

`src/interp.rs` is the reference interpreter; `src/emit.rs` (wasm backend) must
agree with it on every program (see `CLAUDE.md`). When in doubt about behaviour,
the interpreter wins; emit is validated against it.

## Steps

| # | File | Scope |
|---|------|-------|
| 01 | `tuple-calls/step-01-reader.md` | Reader produces tuple-calls; drop `[`/`{` attach and grouping |
| 02 | `tuple-calls/step-02-interpreter.md` | `interp.rs` + `value.rs`: eval Tup as call, Quasi, Match, Quote |
| 03 | `tuple-calls/step-03-expander-and-runner.md` | `expand.rs` + `runner.rs` |
| 04 | `tuple-calls/step-04-builtins.md` | `builtins.rs`: `expand`, `form-kind` |
| 05 | `tuple-calls/step-05-wit.md` | `wit.rs`: WIT synthesis over tuple-forms |
| 06 | `tuple-calls/step-06-emit-and-cleanup.md` | `emit.rs` backend; remove `Node::Call` |
| 07 | `tuple-calls/step-07-scaffold-and-wvl.md` | scaffold templates + `examples/*.wvl` |
| 08 | `tuple-calls/step-08-doc-examples.md` | `gen-examples.mjs` + regenerate + lock tests |
| 09 | `tuple-calls/step-09-docs-prose.md` | docs prose + `dev-notes/design.md` |
| 10 | `tuple-calls/step-10-grammars.md` | Prism, VS Code, Neovim grammars |
| 11 | `tuple-calls/step-11-lsp.md` | LSP server |
| 12 | `tuple-calls/step-12-changelog-and-pr.md` | CHANGELOG, final verification, open PR |
