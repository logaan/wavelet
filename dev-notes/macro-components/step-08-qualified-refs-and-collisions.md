# Step 8 — Qualified references, aliasing, and collision errors

- [x] Done

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

**Qualified spelling (no lexer change needed).** A qualified TitleCase head
`alias/Name` already lexes today: the lexer (`src/lexer.rs`) emits
`Tok::QIdent(alias, name, is_title=true)`, with the name part already lowercased
and `-MACRO`-suffixed by `title_to_macro_name` (so `dsl/Unless` →
`QIdent("dsl", "unless-MACRO", true)` → `Node::Qsym("dsl", "unless-MACRO")`).
**The alias part must be kebab-case** — the lexer rejects a TitleCase alias with
"alias part of a qualified name must be kebab-case". design.md §6.3 writes
`Dsl/Element` illustratively, but the spelling the reader actually accepts is
`dsl/Element` (kebab alias / TitleCase name), matching imports' `as: dsl` alias.
No lexer/highlighting change was made, so **Step 10 has nothing extra to mirror
into the Prism/Neovim/VS Code grammars from this step** (qualified heads were
already a token class).

**How qualified keys are stored in `MacroTable` (`src/reader.rs`).** The table
now has three maps:
- `map: HashMap<String, (usize, Origin)>` — bare name → (arity, the single
  origin owning it). Core forms and file-local `DefMacro`s use `Origin::Local`;
  each import uses `Origin::Import(alias)`.
- `qualified: HashMap<(String, String), usize>` — `(alias, suffixed-name)` →
  arity, registered for **every** import-provided macro regardless of
  collisions, so a qualified head always resolves.
- `ambiguous: HashMap<String, Vec<String>>` — bare name → contributing import
  aliases, populated once a bare name is claimed by two different origins (the
  bare name is then removed from `map`).

`register_foreign(alias, name, arity)` writes both the qualified key and the
bare key (with collision tracking); `register(name, arity)` keeps the
local/core path. `arity` / `arity_qualified` / `is_ambiguous` are the lookups
the reader's `title_form` consults: a `Node::Qsym` head goes to
`arity_qualified`, a `Node::Sym` head checks `is_ambiguous` first (actionable
error naming both qualified spellings) then `arity`.

**Collision policy: (a), lazy.** Registering both colliding macros under their
qualified keys always succeeds; the **bare** name becomes ambiguous and only
errors **when the bare name is actually used** (qualified uses always work).
This applies to import-vs-import *and* local-`DefMacro`-vs-import collisions.
The bare-use error names the macro and suggests qualifying (`dsl/unless` /
`web/unless`) or aliasing the imports with `as:`. An unknown qualified head
(`dsl/Nope`, or an alias that exists but doesn't publish the name) gives a
distinct "unknown qualified macro" error.

**Expansion dispatch (`src/expand.rs` + `src/macrodep.rs`).** `expand_form` now
handles a `Node::Qsym(alias, name)` head in addition to `Node::Sym`, routing it
to the foreign expander with `Some(alias)`. `ForeignExpander::expand_call`
gained an `alias: Option<&str>` parameter: `None` scans all imports (bare head,
unchanged Step 7 behaviour), `Some(alias)` routes strictly to the import bound
to that alias via `FileExpander::owner_for_alias`, so a qualified call expands
through the right component even when the bare name is ambiguous. The PINNED
args-tree contract is unchanged (the whole call form is shipped; the guest
ignores the head and indexes args from element 1, so a `Qsym` head marshals
fine).

**Tests.** `MacroTable`-level unit tests in `reader.rs::macro_table_tests`
(collision → ambiguous, qualified-still-resolves, local+foreign collision, core
forms unaffected). Integration tests in `macrodep.rs::tests`: two imports of the
same fixture under aliases `dsl`/`web` (distinct `pkg:` so distinct cache
entries, same `from:` `.wasm`) exercise the collision — bare use errors,
qualified `dsl/Unless` and `web/Unless` resolve; an explicit `as: dsl` alias
resolves `dsl/Unless` while the package's own last segment is *not* a valid
alias; a qualified call expands through the aliased component; and the
single-import bare-use regression still passes. **No new fixture `.wasm` was
needed** — importing the existing `tests/fixtures/macros.wasm` under two aliases
exercises the collision (the simplest robust approach).

**`todo.md` item.** The qualified-arity item (in `dev-notes/notes.md` "From
todo.md") is now ticked, annotated with the real `dsl/Element` spelling and the
collision behaviour.

**`cargo test` green; `cargo check --target wasm32-unknown-unknown --lib`
builds.** No documented example changed, so no `regen-examples.sh` run was
needed.
