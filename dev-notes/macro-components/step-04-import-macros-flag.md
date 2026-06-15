# Step 4 — Parse `Import {… macros: true}` and thread the flag through

- [ ] Done

> **Read first:** `dev-notes/macro-components.md` and `dev-notes/design.md` §6.3
> (the `Import {pkg: "acme:html/dsl" macros: true}` line) and §6.1/§6.2 for how
> imports are written. Base your worktree on the latest `origin/macro-components`
> (after Steps 1–3).

## Context you need

Imports are parsed in `src/wit.rs`, in `collect()`'s `"import-MACRO"` arm
(currently around `src/wit.rs:74`–`97`). An `Import` is either a bare string or a
record; the record arm reads `pkg:` and `as:` today and **ignores any other
field**:

```rust
("pkg", Node::Str(s)) => pkg = Some(s.clone()),
("as", Node::Sym(s)) => alias = Some(s.clone()),
_ => {}
```

The parsed result is an `ImportInfo { path, package, alias }` (`src/wit.rs:24`).
There is no `macros` flag anywhere. design.md §6.3 line in §6.1's example:

```
Import {pkg: "acme:html/dsl" macros: true}          // load macro manifest too
```

This step is **purely plumbing**: recognise and carry the flag. No
instantiation, no registration, no expansion behaviour — those are Steps 5–7.

## Goal

`Import {… macros: true}` parses without error and the `macros` intent is
captured on `ImportInfo` (and anywhere else the import is modelled), defaulting to
`false`. Everything downstream still behaves exactly as today.

## Scope

- **`ImportInfo`** (`src/wit.rs:24`): add a `pub macros: bool` field (default
  `false`).
- **The `import-MACRO` record arm** (`src/wit.rs:80`–`88`): read
  `("macros", Node::Bool(b))` into the flag. Keep tolerating the bare-string form
  (no record → `macros: false`).
- **Construction site** (`src/wit.rs:97`): populate the new field.
- **Audit other `ImportInfo` constructors / readers.** Grep for `ImportInfo {`
  and for any place that pattern-matches its fields, so the new field is set
  everywhere and the build still compiles.
- **A reader/`collect` test** asserting `macros: true` round-trips into
  `ImportInfo.macros == true`, and that omitting it (and the bare-string form)
  yields `false`.

## Watch out for

- **Don't change WIT synthesis.** A `macros: true` import is a *compile-time*
  dependency, not necessarily a runtime world import. Whether such an import
  should still appear in the synthesized world (`src/wit.rs` `world_wit` /
  `host_only` paths) is a **Step 5/6 question** — for *this* step, preserve
  current behaviour exactly and just carry the flag. If you find that carrying the
  flag forces a synthesis decision, document it in the handoff and make the
  minimal, behaviour-preserving choice.
- **Lexer/highlighting:** `macros: true` is an ordinary record field — no new
  token class, so no lexer or syntax-highlighting change is expected. Confirm
  this and note it.

## Done when

`cargo test` passes; `ImportInfo` carries a `macros: bool`; parsing
`Import {pkg: "…" macros: true}` sets it; all existing import behaviour is
unchanged.

## Handoff notes

_(fill in: the final `ImportInfo` shape, every site touched, and whether a
`macros: true` import currently still flows into WIT synthesis or not — Step 5
needs to know.)_
