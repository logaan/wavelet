# Step 3 — The `wavelet:meta/macros` interface + a `manifest`/`expand` caller

- [ ] Done

> **Read first:** `dev-notes/macro-components.md` and `dev-notes/design.md` §6.3.
> Base your worktree on the latest `origin/macro-components` (after Steps 1–2).

## Context you need

A macro library is a component exporting `wavelet:meta/macros` (design.md §6.3):

```wit
interface macros {
  use code.{tree};
  manifest: func() -> list<tuple<string, u32>>;          // (name, arity) pairs
  expand: func(name: string, args: tree) -> result<tree, string>;
}
```

Step 1 gave us the `tree` type and `form::Arena` ↔ `Tree` conversion. Step 2 gave
us a runtime that can instantiate a component and call exports with dynamic
`Val`s. This step **joins them**: a typed caller that, given an instantiated
macro component, can call its two exports with proper marshalling.

## Goal

A `MacroComponent` abstraction over an instantiated `wavelet:meta/macros`
component, with two methods — `manifest() -> Vec<(String, u32)>` and
`expand(name, args: &Tree) -> Result<Tree, String>` — fully marshalling between
our `Tree` (Step 1) and the runtime's `Val`s (Step 2). Tested against a fixture
macro component.

## Scope

- **Add the `macros` interface** to the `wavelet:meta@0.1.0` WIT from Step 1
  (the `manifest`/`expand` signatures above, `use code.{tree}`).
- **`Tree` ⇄ `Val` marshalling.** Lower a `Tree` into the `component::Val`
  shape the runtime expects for the `tree` record (list of `node` variants +
  `root` + `spans`), and lift a `result<tree, string>` `Val` back into
  `Result<Tree, String>`. This is the fiddly part: the `node` variant has many
  cases, each mapping to a `Val::Variant`. Centralise it so Step 7 just calls
  `expand`.
- **`MacroComponent`** built on the Step 2 runtime: locate the `manifest` and
  `expand` exports of the `wavelet:meta/macros` interface on an instantiated
  component and wrap the two calls with the marshalling above.
- **Fixture macro component.** Extend or replace the Step 2 fixture with one that
  genuinely exports `wavelet:meta/macros`: a small set of macros is enough (e.g.
  an `unless`-style macro and an identity macro). Hand-written WAT or a tiny Rust
  component, checked into `tests/fixtures/`. Keep tests hermetic.
- **Tests:** `manifest()` returns the fixture's `(name, arity)` pairs; `expand`
  on a known call form returns the expected rewritten `tree` (compare by
  converting back to an arena and printing canonically with `printer`); an
  `expand` that returns `result::err` surfaces the error string.

## Watch out for

- **Variant case ordering / names.** The `Val::Variant` discriminant must match
  the WIT `node` variant exactly (case name and payload shape). A mismatch here
  produces confusing runtime trap/marshalling errors — test every node variant.
- **`result<tree, string>`** lifts to a `Val::Result`; map `ok`→`Ok(Tree)`,
  `err`→`Err(String)`.
- **Empty `args`.** A nullary macro call still passes a `tree` (an empty/leaf
  payload). Make sure the conversion handles the trivial cases.

## Done when

`cargo test` passes; `MacroComponent::manifest`/`expand` work end-to-end against
a fixture `wavelet:meta/macros` component, including the error path; every `node`
variant is covered by a marshalling round-trip test.

## Handoff notes

_(fill in: the `MacroComponent` public surface, exactly how the `node` variant
maps to `Val::Variant` cases, where the fixture macro component lives and what it
exports, and any gotchas Step 7 should know when it wires `expand` into the
expander.)_
