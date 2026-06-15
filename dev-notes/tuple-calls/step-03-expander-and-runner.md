# Step 03 — Ahead-of-time expander & module runner

**Read `dev-notes/tuple-calls.md` (the index) first.** This step updates the
compile-time macro expander and the multi-module runner to read the new
tuple-call shape. Both currently pattern-match `Node::Call(head, payload)`.

Files: `src/expand.rs`, `src/runner.rs`. Keep the `Node::Call` enum variant.
Work on `tuple-calls`, commit as you go, no PR. Depends on Steps 01–02 being
committed.

## Background

A call/special-form/macro-use now reads as `Node::Tup(items)` with `items[0]`
the head (a `Sym`/`Qsym`) and `items[1..]` the arguments. `expand_once` (changed
in Step 02) now takes `args: &[NodeId]` instead of a single payload node.

## `src/expand.rs`

1. **`is_def_macro`**: match `Node::Tup(items)` where `items.first()` is
   `Node::Sym(s)` with `s == "def-macro-MACRO"` (instead of `Node::Call`).

2. **`expand_form`**: a macro use is `Node::Tup(items)` whose head `items[0]` is
   `Node::Sym(name)`.
   - Keep the special-case early return for `name == "quote-MACRO" ||
     name == "quasi-MACRO"` (do not expand quoted/quasiquoted forms at compile
     time — `copy_form` them).
   - If `env.lookup(name)` is `Some(Value::Macro(mac))`, call
     `interp.expand_once(&mac, arena, &items[1..])` (pass the argument slice),
     then recursively `expand_form` the result. Keep the error context
     (`format!("expanding `{}`: …", name.trim_end_matches("-MACRO"))`).
   - Otherwise fall through to `descend`.

3. **`descend`**: replace the `Node::Call(head, payload)` arm with a
   `Node::Tup(items)` arm that expands every element:
   `Node::Tup(items.iter().map(|&x| expand_form(...)).collect()?)`. The existing
   `Node::Lst` and `Node::Rec` arms are unchanged. (There is no longer a special
   head/payload split — a Tup's head is just its first element and is expanded
   like any other element. Macro heads are intercepted in `expand_form` before
   `descend`, so ordinary heads here are fine to expand structurally.)

4. **`copy_form`**: replace the `Node::Call` arm with a `Node::Tup` arm that
   copies every element. (Used for quoted/quasiquoted subtrees.)

## `src/runner.rs`

The module loader inspects each top-level form to handle `Package`, `Target`,
`Import`, `Export`, `DefType`, and otherwise evaluates it. Top-level forms now
read as `Node::Tup`.

1. **`eval_module` loop** (`for root in roots`): replace
   `let Node::Call(head, payload) = arena.node(root) else { … }` with
   `let Node::Tup(items) = arena.node(root) else { … eval … }`. Then:
   - `head_name = items.first()` as `Node::Sym(s)` ⇒ `s`, else `""`.
   - For the special heads, the **single payload** these handlers want is
     `items[1]` (Package/Target/Import/Export are arity 1; DefType is arity 2):
     - `export-MACRO` ⇒ `export_entry(&arena, items[1])`
     - `import-MACRO` ⇒ `parse_import(&arena, items[1])`
     - `package-MACRO | target-MACRO | def-type-MACRO` ⇒ no-op (as today).
   - Default ⇒ evaluate the whole form as before.
   Guard against a malformed/empty tuple (`items` shorter than expected) by
   falling through to plain evaluation or a clear error, matching current
   robustness.

2. **`find_package`**: match `Node::Tup(items)` with `items[0]` a `Sym` equal to
   `"package-MACRO"` and `items[1]` a `Node::Str` ⇒ `strip_version(s)`.

3. **`export_entry` and `parse_import`** take the payload node and are unchanged
   internally — they already handle `Node::Sym` / `Node::Rec` / `Node::Str`.
   You are only changing *which node* you pass them (now `items[1]`).

## Verification

- `cargo build` green.
- Smoke-test module loading end to end with the interpreter once Step 07 has
  rewritten `examples/*.wvl` — that is a later step, so here just confirm a
  single-file program with an `Export`/`Def` runs. For example, run a small
  program through `cargo run -- run <file>` where `<file>` contains:
  ```
  Package "demo:x@0.1.0"
  Export greet
  Def greet Fn {name: string} str-cat(upper(name) "!")
  ```
  (Note new syntax: `str-cat(...)`, not `str-cat[...]`.) `greet` should be
  importable/callable. If running a full module is awkward before Step 07, a
  REPL/`expand`-level check that `DefMacro` + a TitleCase use expands correctly
  is sufficient, e.g.:
  ```
  DefMacro and {a b} Quasi If Unquote(a) Unquote(b) false
  And lt(5 10) gt(5 0)
  ```
  should expand and evaluate to `true`.
- Full `cargo test` is not expected to pass yet.

## Commit

e.g. `feat(expand,runner): read top-level and macro forms as tuples`
