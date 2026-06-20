# Step 4 — Parse `Import {… macros: true}` and thread the flag through

- [x] Done

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

Pure plumbing landed; no behaviour change beyond carrying the flag.

**Final `ImportInfo` shape** (`src/wit.rs:22`):

```rust
pub struct ImportInfo {
    pub path: String,    // interface path, version stripped, e.g. `demo:shout/api`
    pub package: String, // package part, e.g. `demo:shout`
    pub alias: String,
    pub macros: bool,    // NEW — `Import {… macros: true}`; default false
}
```

**Every site touched:**

- `src/wit.rs:22` — added `pub macros: bool` to `ImportInfo` (with doc comment).
- `src/wit.rs` `import-MACRO` arm — the record-form `spec` tuple grew a third
  element; a new match arm `("macros", Node::Bool(b)) => macros = *b` reads the
  flag. The bare-string import form (`Node::Str`) supplies `false`. The
  construction site `imports.push(ImportInfo { …, macros })` populates it.
- `src/wit.rs` `#[cfg(test)] mod tests` — new tests: `import_macros_flag_true`,
  `import_macros_flag_defaults_false` (record without `macros:`, and explicit
  `macros: false`), `import_bare_string_form_defaults_false`.

**`ImportInfo` constructors / readers audited:** the struct is constructed in
exactly one place (`src/wit.rs`, the `import-MACRO` arm). It is only ever
*field-read* elsewhere — `src/build.rs` touches `.path`, `.package`, `.alias`
(never destructures the struct), so adding a field required no `build.rs`
change. No other file constructs or pattern-matches `ImportInfo`.

**Does a `macros: true` import still flow into WIT synthesis? YES — unchanged.**
`synthesize_info` (`src/wit.rs`) iterates `info.imports` with no reference to the
`macros` flag, so a `macros: true` import is still emitted into the synthesized
`world` exactly as before (host `wasi:*` → versioned `import`; sibling → bare
`import` unless `host_only`). This was the deliberate behaviour-preserving choice
for this step. **Step 5/6 decision point:** whether a `macros: true` import is a
*compile-time-only* dependency that should be **excluded** from the runtime world
(i.e. filtered out of `synthesize_info`'s import loop and/or `has_host_deps` /
build-graph edges in `src/build.rs`) is left to those steps. Today it is treated
as an ordinary world import.

**Lexer / syntax-highlighting:** no change needed. `macros: true` is an ordinary
record field (`Sym` key `:` `Bool` value) — it introduces no new token class, so
`src/lexer.rs` and the three grammars (Prism / Neovim / VS Code) are unaffected.

**Examples / docs:** no language behaviour changed and no documented example was
touched (`git status` showed only `src/wit.rs` + this step file), so
`./scripts/regen-examples.sh` was not required. `cargo test` is green.
