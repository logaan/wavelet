# Step 04 — Builtins (`expand`, `form-kind`)

**Read `dev-notes/tuple-calls.md` (the index) first.** Most builtins are
unaffected by this change (they operate on `Value`s, and the call path that
delivers their argument was handled in Step 02). Two meta-builtins reference the
old call/variant shape and need updating.

File: `src/builtins.rs`. Work on `tuple-calls`, commit as you go, no PR. Depends
on Steps 01–03.

## 1. The `expand` builtin

`expand(form)` takes a quoted macro use and runs the macro once. Previously a
quoted call was a `Value::Variant(head, payload)`; now it is a `Value::Tup`
whose first element is the macro-name symbol. Example:
`expand(Quote And(p q))` — `Quote And(p q)` reads to `Tup[and-MACRO, p, q]` and
quotes to `Value::Tup([Variant("and-MACRO",None), Variant("p",None),
Variant("q",None)])`.

Rewrite the `"expand"` arm to handle a `Value::Tup`:
- Destructure `arg` as `Value::Tup(items)` with a non-empty `items`.
- The head `items[0]` must be `Value::Variant(name, None)` ⇒ the macro name.
  (If it is not a symbol, or the lookup below fails, return `arg.clone()`
  unchanged — same lenient behaviour as today.)
- `env.lookup(name)` must be `Some(Value::Macro(mac))`; otherwise return
  `arg.clone()`.
- Convert the argument values `items[1..]` into form nodes in a fresh arena via
  `value_to_form`, collecting a `Vec<NodeId>` of arg node ids.
- Call `interp.expand_once(&mac, &Rc::new(arena), &arg_nodes)` (note the new
  `args: &[NodeId]` signature from Step 02), then `form_to_value(&out, root)`.
- Keep the existing guard that `expand` needs an evaluation context
  (`env`).

A non-`Tup` argument (e.g. an atom, or a bare `Value::Variant(name, None)` which
is a symbol with no args) should be returned unchanged, as today.

## 2. The `form-kind` builtin

Update the kind mapping. With calls now tuples, a quoted call is a `Value::Tup`,
so `form-kind` returns `"tup"` for it. Keep `"sym"` for payload-less variants
(symbols). Runtime variants carrying a payload (`ok(x)`, `some(x)`, …) still map
to `"call"` (they are genuine `Value::Variant(_, Some(_))` values, not quoted
code). So:

```
Value::Variant(_, None)    => "sym",
Value::Variant(_, Some(_)) => "call",   // runtime variant value (ok/err/some/…)
Value::Tup(_)              => "tup",     // includes quoted call forms now
```

i.e. **leave this arm essentially as-is** — the behaviour change is that quoted
calls now arrive as `Value::Tup` and therefore report `"tup"` instead of
`"call"`. Confirm the mapping reads as above and add a brief comment noting that
quoted calls are tuples now.

## Other builtins — no change

`gensym` (still returns `Value::Variant("g{n}-gen", None)`), `apply`,
`some/ok/err`, `rec-key/rec-val`, `read`, `to-string`, `cell-*`, etc. are
unaffected. Do not change them.

## Verification

- `cargo build` green.
- REPL/manual checks:
  - `DefMacro and {a b} Quasi If Unquote(a) Unquote(b) false` then
    `expand(Quote And(p q))` ⇒ prints the expanded `If` form, e.g.
    `(if-MACRO, p, q, false)`.
  - `form-kind(Quote foo(1))` ⇒ `"tup"`.
  - `form-kind(Quote foo)` ⇒ `"sym"`.
  - `form-kind(ok(1))` ⇒ `"call"` (runtime variant).
- Full `cargo test` still not expected to pass until Step 08.

## Commit

e.g. `feat(builtins): expand over tuple macro-uses; form-kind treats quoted calls as tup`
