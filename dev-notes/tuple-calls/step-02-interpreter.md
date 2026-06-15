# Step 02 — Interpreter & value conversion

**Read `dev-notes/tuple-calls.md` (the index) first.** This step makes the
reference interpreter evaluate the new tuple-call forms, and updates the
form↔value conversions. After Step 01 the reader produces `Node::Tup` for every
call; here we make eval treat a `Tup` as a call (Lisp-style).

Files: `src/interp.rs`, `src/value.rs`. Keep the `Node::Call` enum variant
(index "build-green strategy"). Work on `tuple-calls`, commit as you go, no PR.

## `src/value.rs`

1. **`form_to_value`** (Quote: form → value).
   - `Node::Tup(items)` ⇒ `Value::Tup(items mapped through form_to_value)`. This
     is already the existing `Tup` arm — keep it.
   - `Node::Sym(s)` ⇒ `Value::Variant(s, None)` (symbol) — unchanged.
   - The `Node::Call` arm is now dead (reader never emits it). You may leave it
     compiling as-is; it is removed in Step 06.
   Net effect: `Quote foo(1 2)` (which reads to `Tup[foo,1,2]`) becomes
   `Value::Tup([Variant("foo",None), Int 1, Int 2])`, printing `(foo, 1, 2)`.

2. **`value_to_form`** (macro output: value → form).
   - `Value::Tup(items)` ⇒ `Node::Tup(items …)` — unchanged.
   - `Value::Variant(name, None)` ⇒ `sym_node(name)` — unchanged.
   - `Value::Variant(name, Some(p))` ⇒ build `Node::Tup([sym_node(name),
     value_to_form(p)])` (a 1-argument call form). Replace the current arm that
     builds `Node::Call(head, payload)`.

No other `value.rs` changes (printing of `Value::Tup`/`Value::Variant` is
unchanged; runtime variants `ok/err/some/none` still print `name(payload)`).

## `src/interp.rs`

### Evaluate a `Node::Tup` as a call

In `step`, replace the `Node::Tup` arm (currently builds a `Value::Tup`) and the
`Node::Call` arm with a single call path:

```
Node::Tup(items) => self.step_call(arena, items, env),
```

`step_call(arena, items: &[NodeId], env)`:
- If `items` is empty ⇒ `err("cannot evaluate empty form ()")`.
- `head = items[0]`; `args = &items[1..]`.
- Resolve the head **name**: `Node::Sym(s)` ⇒ `s`, `Node::Qsym(a,n)` ⇒
  `"{a}/{n}"`, otherwise ⇒ `err("call head must be a name (use apply for a
  computed function)")`.
- If `special_form(name, arena, args, env)?` returns `Some(step)`, return it.
- Look up the head value. If it is `Value::Macro(c)`, expand it (see below).
- Otherwise evaluate the args, **bundle** them, and apply:
  - `0` args ⇒ `Value::Tup(vec![])`
  - `1` arg  ⇒ `self.eval(arena, args[0], env)?`
  - `≥2` args ⇒ `Value::Tup(self.eval_each(arena, args, env)?)`
  Then `self.apply_step(&f, arg, Some(env))`.

`bind_params` (the §4.2 binding logic) is **unchanged** — the bundling above
reproduces the old payload exactly, so record/list/tuple/scalar binding still
works. Leave `bind_params` as-is.

### Special forms take an `args` slice

Change `special_form`'s signature from `(name, arena, payload: NodeId, env)` to
`(name, arena, args: &[NodeId], env)`. The old code destructured the single
`payload` tuple with `tup2`/`tup3`; now the arguments are `args` directly:

- Replace `tup2(arena, payload, "X")` with a check that `args.len() == 2`,
  yielding `[args[0], args[1]]`; similarly `tup3` ⇒ `args.len() == 3`. Keep the
  same error messages ("X expects N arguments"). You can keep helper functions
  but have them take `args: &[NodeId]`.
- `def-MACRO`: `args = [name, expr]`.
- `fn-MACRO`: `args = [params, body]`.
- `if-MACRO`: `args = [c, t, e]`.
- `let-MACRO`: `args = [bindings, body]`.
- `do-MACRO`: arity 1 — `args = [list]`. The list is `args[0]`; require it to be
  a `Node::Lst` and sequence its elements (same as today, but read the list from
  `args[0]` instead of `payload`).
- `match-MACRO`: `args = [scrut, clauses]`. See pattern matching below.
- `quote-MACRO`: arity 1 — `args = [form]`. Return `form_to_value(arena,
  args[0])`. (So `Quote (1 2)` ⇒ tuple value `(1,2)`; `Quote foo(1)` ⇒
  `(foo, 1)`.)
- `quasi-MACRO`: arity 1 — `args = [form]`. Return `self.quasi(arena, args[0],
  env, 1)?`. See Quasi below.
- `unquote-MACRO`/`splice-MACRO`: same error as today ("only valid inside
  Quasi").
- `def-macro-MACRO`: `args = [name, params, body]`.
- `the-MACRO`: `args = [ty, expr]`.
- `package/target/import/export/def-type-MACRO`: same "only at top level" error.
- Return `Ok(None)` for non-special names (so `step_call` falls through to a
  function/macro application).

### Macro expansion (`expand_once`)

Change `expand_once` to take the **argument forms** instead of a single payload
node:

```
pub fn expand_once(&self, mac: &Rc<Closure>, arena: &Rc<Arena>, args: &[NodeId])
    -> R<(Rc<Arena>, NodeId)>
```

Binding rule (mirror of the old behaviour, adapted to flat args). Let
`n = mac.params.len()`:
- if `n == args.len()` ⇒ bind each param to `form_to_value(arena, arg)`.
- else if `n == 1` ⇒ bind the single param to `Value::Tup(args mapped through
  form_to_value)` (a 1-param macro receiving several explicit args gets them as
  a tuple form — rare, but keep it well-defined).
- else ⇒ `err(format!("macro expects {n} arguments"))`.
Then evaluate the body and serialize the result with `value_to_form` exactly as
today.

Update the two callers:
- `expand_macro(mac, arena, payload, use_env)` ⇒ becomes
  `expand_macro(mac, arena, args, use_env)`; pass `args` through.
- `step_call`'s macro branch passes `args`.

### Quasi (`quasi` / `quasi_seq`)

`Quasi` now operates over `Node::Tup` everywhere (no `Node::Call`). Rewrite
`quasi` so that, for `Node::Tup(items)`:

1. If `items.first()` is a `Node::Sym(name)`, handle the special heads (the
   single argument is `items[1]`; these forms are arity 1):
   - `name == "unquote-MACRO"` and `depth == 1` ⇒ return `self.eval(arena,
     items[1], env)`.
   - `name == "splice-MACRO"` and `depth == 1` ⇒ `err("Splice must appear inside
     a sequence")`.
   - `name == "unquote-MACRO" | "splice-MACRO"` and `depth > 1` ⇒ rebuild one
     level shallower: `Value::Tup([Variant(name, None), self.quasi(arena,
     items[1], env, depth-1)?])`.
   - `name == "quasi-MACRO"` ⇒ rebuild one level deeper:
     `Value::Tup([Variant(name, None), self.quasi(arena, items[1], env,
     depth+1)?])`.
   (Only treat these as special when `items.len() == 2`, matching their arity.)
2. Otherwise (ordinary head, or non-Sym head) ⇒
   `Value::Tup(self.quasi_seq(arena, items, env, depth)?)`.

`Node::Lst` ⇒ `Value::Lst(quasi_seq(...))`; `Node::Rec` ⇒ rebuild fields with
`quasi`; leaves ⇒ `form_to_value`. (Same structure as today, minus the `Call`
arm and minus the old "rebuild as `Value::Variant`" for non-special calls — they
are now rebuilt as `Value::Tup`.)

`quasi_seq` keeps the splice handling but detects the splice on a `Node::Tup`
whose head is `splice-MACRO`:

```
if depth == 1 {
    if let Node::Tup(items) = arena.node(item) {
        if let [head, arg] = items[..] {            // arity-1 splice
            if matches!(arena.node(head), Node::Sym(s) if s == "splice-MACRO") {
                // eval arg; must be Value::Lst; extend `out`; continue
            }
        }
    }
}
out.push(self.quasi(arena, item, env, depth)?);
```

Worked check: `Quasi add(Unquote(x) 1)` with `x = 41` ⇒ head `add` is ordinary ⇒
`quasi_seq([add, (unquote x), 1])` ⇒ `Tup[Variant("add",None), Int 41, Int 1]`
⇒ prints `(add, 41, 1)`.

### Pattern matching (`match_pattern`)

A `Node::Tup` pattern is disambiguated by the **scrutinee value**. Update the
`match_pattern` `Node::Tup` arm (and remove the `Node::Call` arm — fold its
variant-matching logic into the `Tup` arm):

```
Node::Tup(pats) => match v {
    // variant-case pattern: (case …rest) against Variant(case, payload)
    Value::Variant(cval, payload)
        if matches!(arena.node(pats[0]), Node::Sym(c) if c == cval) =>
    {
        let rest = &pats[1..];
        match (rest.len(), payload) {
            (0, None) => Ok(true),
            (0, _)    => Ok(false),
            (1, Some(p)) => match_pattern(arena, rest[0], p, binds, scope),
            (1, None)    => Ok(false),
            (_, Some(p)) => match (&**p) {
                Value::Tup(vs) if vs.len() == rest.len() =>
                    match_all(arena, rest, vs, binds, scope),
                _ => Ok(false),
            },
            (_, None) => Ok(false),
        }
    }
    // tuple-destructure pattern: (a b …) against a tuple value
    Value::Tup(vs) if vs.len() == pats.len() =>
        match_all(arena, pats, vs, binds, scope),
    _ => Ok(false),
},
```

The bare `Node::Sym` pattern arm is unchanged (payload-less variant case in
scope ⇒ equality match; otherwise bind). The `Lst`/`Rec`/literal arms are
unchanged.

Worked checks (from the doc examples):
- `Match ok(42) [(ok(n) …) (err(e) …)]`: scrutinee `Variant("ok", Some(42))`;
  pattern `(ok n)` = `Tup[Sym ok, Sym n]`; head matches case `ok`, `rest=[n]`,
  payload `Some(42)` ⇒ bind `n=42`. ✓
- `Match none [(none …) (some(v) …)]`: `none` pattern is a `Sym` (payload-less
  variant), matches `Variant("none", None)` by equality. ✓
- `Match [1 2 3] [([] …) ([x] …) ([x y z] …) (other …)]`: list patterns,
  unchanged. ✓

## Verification

- `cargo build` green.
- Smoke-test the interpreter directly (REPL via `cargo run -- repl`, or a small
  throwaway driver). These should now evaluate correctly:
  - `add(1 2)` ⇒ `3`  (after later steps the std lib is unchanged; `add` is a
    builtin — see Step 04, but the call path is what you are testing here)
  - `If lt(2 3) "less" "more"` ⇒ `"less"`
  - `Quote foo(1 2)` ⇒ prints `(foo, 1, 2)`
  - `Match ok(42) [(ok(n) add(n 1)) (err(e) 0)]` ⇒ `43`
  - `Let {x: 41} Quasi add(Unquote(x) 1)` ⇒ prints `(add, 41, 1)`
  Note: builtins like `add` already work via `apply_step`/`builtins::call`; you
  are exercising the new `step_call`. Full `cargo test` is **not** expected to
  pass until Step 08.

## Commit

e.g. `feat(interp): evaluate tuple forms as calls; tuple Quote/Quasi/Match`
