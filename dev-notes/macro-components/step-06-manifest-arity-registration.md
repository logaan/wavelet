# Step 6 — Register foreign macro arities from `manifest()` with the reader

- [x] Done

> **Read first:** `dev-notes/macro-components.md`, `dev-notes/design.md` §2.4 (the
> macro sugar / arity reading) and §6.3. Base your worktree on the latest
> `origin/macro-components` (after Steps 1–5).

## Context you need

The reader must know every visible macro's **arity** as it moves top-to-bottom,
because a paren-free TitleCase head consumes exactly `arity` following forms
(§2.4). Local arities live in `src/reader.rs`'s `MacroTable`:

- `MacroTable::core()` seeds the special-form arities (`src/reader.rs:15`–).
- `arity(name)` / `register(name, arity)` (`src/reader.rs:39`–`44`).
- `read_with(src, &mut macros)` reads with a caller-owned table
  (`src/reader.rs:53`–), and `register_if_def_macro` (`src/reader.rs:324`–)
  registers a local `DefMacro`'s arity mid-file.
- The TitleCase read path looks arity up at `src/reader.rs:196`–`217` and errors
  with *"unknown macro `<name>` (macros must be in scope before use)"* when it's
  missing.

Foreign arities come from a macro component's `manifest()` (Step 3:
`MacroComponent::manifest() -> Vec<(String, u32)>`). design.md §6.3: importing
with `macros: true` "registers its manifest with the reader (this is how
TitleCase arities are known across components)."

## Goal

Before the reader reads the body of a file, each `macros: true` import is
resolved (Step 5) and instantiated, `manifest()` is called, and every
`(name, arity)` pair is registered into the `MacroTable` — so foreign TitleCase
macros read with correct arity exactly like local ones. The reader's name for a
macro `foo` is the suffixed `foo-MACRO` form (mirroring `register_if_def_macro`).

## Scope

- **An ordering problem to solve carefully.** `Import` forms are themselves read
  by the reader, and arities must be registered *before* later forms that use the
  imported macros. Two viable shapes — pick one and document it:
  - **Pre-scan:** before the main `read_with` pass, scan the source for
    `Import {… macros: true}` forms, resolve + register their manifests, then read
    the body with the populated table. Simpler, but imports must precede uses
    (which §2.4/§6.1 already require — "definitions must precede macro use").
  - **Inline during read:** when the reader finishes an `Import {… macros:
    true}` form, resolve + register right then (analogous to
    `register_if_def_macro`). More faithful to top-to-bottom semantics.
  Recommend the inline approach for fidelity, falling back to a pre-scan if the
  reader can't easily call into the (native-only) resolver. Note that the reader
  also compiles to the `wasm32` playground, so the resolver call must be behind
  `#[cfg(not(target_arch = "wasm32"))]` — in the playground there are no macro
  components, so foreign registration is simply absent there.
- **Name suffixing & aliasing.** Register `<name>-MACRO`. If the import has an
  alias (`as:`), the qualified form `Alias/Name` must also resolve to the right
  arity — Step 8 owns the full qualified-reference / collision story, so here just
  register under the unaliased name and leave a clear hook for Step 8 (note the
  existing `dev-notes/todo.md` item: qualified TitleCase arity lookup currently
  ignores the alias).
- **Tests:** a file importing the fixture macro component (Step 3/5) and using one
  of its TitleCase macros **reads** with correct arity (the previously-failing
  "unknown macro" path now succeeds). Cover a 0-arity and a ≥2-arity foreign
  macro so paren-free consumption is exercised.

## Watch out for

- **Native/wasm split.** `reader.rs` is shared by the native compiler and the
  playground wasm. Don't pull the runtime/resolver into the wasm build.
- **Errors at read time.** If a macro component fails to instantiate or
  `manifest()` traps, surface an actionable error tied to the `Import` (use its
  span) rather than a generic reader failure.
- **Expansion is still Step 7.** This step makes foreign macros *read*; it does
  not yet *expand* them. A file that reads a foreign macro but doesn't expand it
  will still fail later — that's expected until Step 7.

## Done when

`cargo test` passes; a file importing the fixture macro library can use its
TitleCase macros without an "unknown macro" error, with arities driven by the
component's `manifest()`.

## Handoff notes

**Inline, not pre-scan.** Registration happens the moment the reader finishes a
top-level `Import {… macros: true}` form — exactly where the reader already
registers a local `DefMacro` via `register_if_def_macro`. This is the more
faithful top-to-bottom shape and avoids a pre-scan's awkward "read the imports
but not the body" problem (a full pre-read would itself trip the very
"unknown macro" error we're trying to fix, since a paren-free foreign TitleCase
use can't be read until its arity is known). Imports-precede-uses (§2.4/§6.1)
guarantees an inline hook always registers a foreign macro before any later form
consumes it.

**How the native/wasm32 split was handled in the reader.** `reader.rs` stays
runtime-free and compiles unchanged for `wasm32`. The split is a *callback seam*:
`reader.rs` gains `pub type FormHook<'a> = dyn FnMut(&Arena, NodeId, &mut
MacroTable) -> Result<(), ReadError> + 'a` and a new `read_with_hook(src, macros,
Option<&mut FormHook>)`. The closure type names only reader/`form`/`lexer`
types, so it builds everywhere. `read_with` now just calls `read_with_hook(…,
None)`, so the REPL and the wasm playground (which call `read_file`/`read_with`)
are byte-for-byte unchanged and supply no hook → no foreign registration in the
browser, as intended. The hook is invoked right after `register_if_def_macro`
inside the top-level read loop.

**Where the resolver lives / how the root is threaded.** All the runtime-touching
code is in the native-only `src/macrodep.rs`:
- `register_macro_imports(&mut MacroResolver) -> Box<FormHook>` builds the hook
  closure; the closure calls `MacroResolver::register_form`, which parses the
  form (via `parse_macro_import`, mirroring `wit::collect`'s `import-MACRO`
  record branch for a single form), resolves+instantiates the component (Step 5),
  calls `manifest()` (Step 3), and registers each `(name, arity)` as
  `<name>-MACRO` into the `MacroTable` (mirroring local-`DefMacro` naming).
- `read_file_with_macros(src, root)` is the native compiler's read entry point:
  it spins up a fresh `MacroResolver::new(root)` + `MacroTable::core()` and reads
  with the hook. `build.rs` (`build_files`, `populate_project_wit`) and
  `runner.rs` (`run_files`) now call it instead of `crate::read_file`, passing
  the project root (parent of `src/`; `runner.rs` grew its own `project_root`
  helper mirroring `build.rs`'s). The non-macro path is unchanged — a file with
  no `macros: true` import resolves nothing.

**Errors at read time.** A failed resolve/instantiate or a trapping `manifest()`
is surfaced as a `ReadError` tied to the `Import` form's span (`arena.span(id).0`),
so it reads as an actionable read error naming the import, not a generic reader
failure. Covered by `unresolvable_macro_import_errors_at_read_time`.

**Hook left for Step 8 (aliasing / qualified refs).** Registration is under the
**unaliased** `<name>-MACRO` only. The `as:` alias is parsed into the
`ImportInfo` but deliberately *not* used for arity keying here — qualified
`Alias/Name` arity lookup still ignores the alias (the open `dev-notes/todo.md`
item). `MacroResolver::register_form` is the single place Step 8 extends: it has
the resolved `ImportInfo` (including `alias`) and the `manifest()` pairs in hand,
so wiring an alias-qualified key (and collision detection across two macro
imports in one namespace) belongs there.

**For Step 7 (route expansion).** The resolver is reached from the read path via
`macrodep::read_file_with_macros`, but the `MacroResolver` it builds is local to
that call and dropped once reading finishes — so the instantiated components are
**not** currently carried forward to expansion. Step 7 will need the live
components at expand time; either (a) hoist the `MacroResolver` so it outlives
the read and hand it to the expander, or (b) re-resolve from the file's imports
in the expander (the components are cached per package, so re-instantiation is
cheap-ish but not free). Note `MacroComponent::expand` (Step 3) already exists;
the import→component mapping logic lives in `parse_macro_import` +
`MacroResolver::resolve` and can be reused.
