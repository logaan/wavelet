# Step 8 — Qualified references, aliasing, and collision errors

- [ ] Done

> **Read first:** `dev-notes/macro-components.md`, `dev-notes/design.md` §6.3
> (collisions/aliasing/qualified forms) and §2.4 (TitleCase reading), plus the
> open item in `dev-notes/todo.md` about qualified-macro arity lookup ignoring
> the alias. Base your worktree on the latest `origin/macro-components` (after
> Steps 1–7).

## Context you need

design.md §6.3, final paragraph:

> Within a single namespace, macro name collisions are errors, resolved by
> aliasing the import; a qualified TitleCase form `Dsl/Element` disambiguates at
> use sites.

And `dev-notes/todo.md`:

> Qualified TitleCase macros `Dsl/Element` arity reading (parses, but arity
> lookup ignores the alias; revisit with macro imports in Phase 2)

So qualified TitleCase heads (`Dsl/Element`) already **parse**, but the reader's
arity lookup doesn't consult the alias, and there is no collision detection
across imported macro manifests. Steps 6–7 registered foreign macros under their
bare names; this step makes aliased/qualified resolution correct and turns
ambiguous bare names into errors.

## Goal

- A bare TitleCase macro name that is provided by **two** imported manifests (or
  by an import and a local `DefMacro`) is a **compile-time error** with an
  actionable message telling the author to alias or qualify.
- A **qualified** TitleCase form `Alias/Name` resolves arity and expansion to the
  macro from the import bound to `Alias` (the import's `as:` alias from
  `ImportInfo`, Step 4), even when the bare `Name` is ambiguous or shadowed.

## Scope

- **Arity lookup for qualified heads** in `src/reader.rs` (`MacroTable` and the
  TitleCase read path, `src/reader.rs:196`–`217`): when the head is qualified
  (`Alias/Name`, i.e. a `Qsym`-style TitleCase), look up the arity registered for
  that alias's manifest, not the global bare name. This likely means the table
  registers foreign macros under an alias-qualified key in addition to (or
  instead of) the bare key.
- **Collision detection at registration** (extends Step 6): if registering a
  foreign macro's bare name would clash with an already-registered macro of a
  different origin, either (a) register both only under their qualified keys and
  make the bare name *ambiguous* (error on bare use), or (b) error eagerly at
  import time if the design prefers. Recommend (a): collisions are only an error
  if the ambiguous bare name is actually *used* — qualified uses always work.
  Document the choice.
- **Expansion dispatch for qualified heads** (extends Step 7): a qualified head
  routes to the specific `MacroComponent` bound to that alias.
- **Tests:**
  - two fixture imports exporting a same-named macro → bare use errors with a
    helpful message; qualified use of each works.
  - an aliased import (`Import {pkg: "…" as: dsl macros: true}`) → `Dsl/Element`
    (or the project's qualified spelling — confirm against the reader/§2.4)
    resolves and expands.
  - regression: a single unambiguous foreign macro still works by bare name
    (Steps 6–7 behaviour preserved).

## Watch out for

- **Confirm the qualified spelling.** design.md writes `Dsl/Element`; check how
  the lexer/reader actually tokenises a qualified TitleCase head (it parses
  today, per `todo.md`) and match that exactly. If the qualified macro head is a
  distinct token class, no new lexer work should be needed — but **verify**, and
  if the lexer *does* change, the three syntax grammars must be updated (see
  `CLAUDE.md` "syntax highlighting" — this is then also Step 10's concern).
- **Local + foreign collisions.** A local `DefMacro` and an imported macro can
  share a name too; the same rule should apply.
- **Don't break special forms.** Core forms (`If`, `Match`, `DefMacro`, …) in
  `MacroTable::core()` are not namespaced and must stay unambiguous.

## Done when

`cargo test` passes; ambiguous bare foreign-macro names error actionably;
qualified `Alias/Name` heads resolve arity and expand to the right component;
single-import bare use is unchanged; the `dev-notes/todo.md` qualified-arity item
can be ticked.

## Handoff notes

_(fill in: how qualified keys are stored in `MacroTable`, the exact qualified
spelling the reader accepts, the collision policy chosen, whether any lexer/
highlighting change was needed, and the resulting state of the `todo.md` item.)_
