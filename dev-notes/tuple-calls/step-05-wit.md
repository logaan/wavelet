# Step 05 — WIT synthesis (`wit.rs`)

**Read `dev-notes/tuple-calls.md` (the index) first.** `src/wit.rs` derives a
component's WIT world from its top-level forms and type forms. It pattern-matches
`Node::Call(head, payload)` in several places; all become `Node::Tup(items)`
with `head = items[0]` and `args = &items[1..]`.

File: `src/wit.rs`. Keep the `Node::Call` enum variant. Work on `tuple-calls`,
commit as you go, no PR. Depends on Steps 01–04.

## Key shape change

A top-level form is now `Tup[head, arg1, …]`. The arity-1 special forms
(`Package`, `Target`, `Import`, `Export`) take a single payload = `items[1]`.
`DefType`/`Def` take two = `items[1], items[2]`.

## Changes in `collect`

- Replace `let Node::Call(head, payload) = arena.node(root) else { continue };`
  with `let Node::Tup(items) = arena.node(root) else { continue };` and require
  a non-empty tuple whose `items[0]` is `Node::Sym(head_name)`.
- The doc-comment association: `defined_name(arena, payload)` ⇒ pass `items[1]`
  (the form's first argument: the defined name, the export target, etc.).
  `defined_name` itself already handles `Sym`/`Tup`/`Rec` and needs no change.
- `package-MACRO`/`target-MACRO`: read `items[1]` as `Node::Str`.
- `import-MACRO`: inspect `items[1]` (`Node::Str` or `Node::Rec`) exactly as the
  current code inspects `payload`.
- `export-MACRO`: inspect `items[1]` (`Node::Sym` or `Node::Rec`).
- `def-type-MACRO`: the two arguments are now `items[1]` (name `Sym`) and
  `items[2]` (the type form). (Previously this read a `payload` that was itself a
  `Tup` of length 2; now those two are just `items[1]`, `items[2]`.) Push
  `(name, items[2])` into `types`.
- `def-MACRO`: the two arguments are `items[1]` (name `Sym`) and `items[2]` (the
  bound expression). To detect a function definition, check whether `items[2]`
  is an **`Fn` form**, which now reads as `Tup[fn-MACRO, params, body]`:
  ```
  if let Node::Tup(fn_items) = arena.node(items[2]) {
      if fn_items.len() == 3
         && matches!(arena.node(fn_items[0]), Node::Sym(s) if s == "fn-MACRO") {
          // params = fn_items[1], body = fn_items[2]
          defs.insert(name, (fn_items[1], fn_items[2]));
          is_fn = true;
      }
  }
  ```
  Otherwise push `(name, items[2])` into `value_defs`. (Compare with the old
  code that matched `Node::Call(fh, fp)` with `fp` a `Tup` of length 2 — the
  `fn-MACRO` head and the two parts are now all in one flat tuple.)

## `type_text` (type forms → WIT generics)

A type constructor application like `list(u8)`, `result(t e)`, `tuple(a b)` now
reads as `Tup[ctor, arg1, …]` (note: `tuple[a b]` bracket spelling is gone —
type args use parens now). Replace the `Node::Call(head, payload)` arm:

```
Node::Tup(items) => {
    let Node::Sym(ctor) = arena.node(items[0]) else { return Err("bad type form"); };
    let args: Vec<String> = items[1..].iter()
        .map(|&i| type_text(arena, i))
        .collect::<Result<_, _>>()?;
    Ok(format!("{ctor}<{}>", args.join(", ")))
}
```
Keep the `Node::Sym(s) => Ok(s.clone())` arm. A bare type name with no args is a
`Sym`; an applied one is a `Tup`.

## `type_decl` variant cases

`DefType` variant declarations are a `Node::Lst` of cases, where a payloaded
case like `days(30)` previously read as `Node::Call(h, p)`. Update the case loop
to match `Node::Tup(case_items)`:
- `Node::Sym(s)` ⇒ payload-less case `s`.
- `Node::Tup(case_items)` with `case_items[0]` a `Sym(case)` ⇒
  `format!("{case}({})", join(type_text of case_items[1..]))`. For a single
  payload that's `type_text(case_items[1])`; for multiple, join them
  (variant-case payloads are normally a single type, but handle ≥1 uniformly).

## `infer` (result-type inference)

`infer` walks a function body and recognises calls by head name. Replace the
`Node::Call(head, payload)` arm with `Node::Tup(items)`:
- `head = items[0]` must be `Node::Sym(name)`; else `Inferred::Unknown`.
- `args = &items[1..]`.
- The per-name logic is unchanged in spirit; just read arguments from `args`
  instead of decomposing a `payload` tuple/list:
  - arithmetic (`add`, `sub`, …): inspect `args` (the elements directly) for any
    `f64` to decide `f64` vs `s64`. (Previously it matched `payload` as
    `Lst|Tup`; now the operands are `args`.)
  - `if-MACRO`: `args = [c, t, e]`; unify `infer(args[1])`, `infer(args[2])`.
  - `do-MACRO`: `args = [list]`; infer the last element of the `Lst` at
    `args[0]` (or `Unit` if empty).
  - `let-MACRO`: `args = [bindings, body]`; build scope from the `Rec` at
    `args[0]`, then infer `args[1]`.
  - `match-MACRO`: `args = [scrut, clauses]`; the clause list is the `Lst` at
    `args[1]`, each clause a `Tup` of length 2 — unify the result (`pair[1]`) of
    each. (This part already reads `Node::Tup(pair)` for clauses — keep that.)
  - `the-MACRO`: `args = [ty, expr]`; `type_text(args[0])`.
  - default (a call to another module-level def): follow `defs.get(name)` as
    today; the callee's params come from its `params_id` `Rec` (unchanged).

## Verification

- `cargo build` green.
- Smoke-test WIT synthesis with `cargo run -- wit <file>` on a small program
  using the **new** syntax, e.g.:
  ```
  Package "demo:shout@0.1.0"
  Export shout
  Def shout Fn {phrase: string} str-cat(upper(phrase) "!")
  ```
  Expect a world with `shout: func(phrase: string) -> string;`. Also try a
  `DefType` with a parameterised type (e.g. `DefType ids list(u32)`) and a
  variant (`DefType maybe-int [none whole(s64)]`) to exercise `type_text` and
  `type_decl`.
- Full `cargo test` not expected to pass until Step 08.

## Commit

e.g. `feat(wit): synthesize WIT from tuple-shaped call and type forms`
