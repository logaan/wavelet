# Step 08 — Doc examples: rewrite sources, regenerate, lock tests

**Read `dev-notes/tuple-calls.md` (the index) first.** Every runnable doc
example is authored in `docs/scripts/gen-examples.mjs`, which generates
`docs/examples.json`; both the docs `<Playground>` and `tests/examples.rs`
consume it. This step rewrites every example's source to the new syntax and
regenerates the JSON (and the committed wasm artifact). After this step the
**full `cargo test` suite must pass**.

Files: `docs/scripts/gen-examples.mjs`, plus regenerated `docs/examples.json` and
`docs/src/wasm/*` (produced by the script — commit them). Work on `tuple-calls`,
commit as you go, no PR. Depends on Steps 01–07.

## Rewrite the example sources (`gen-examples.mjs`)

Apply the same conversion rules as Step 07 to every ``E['…'] = `…` `` source:
- bracket call sugar ⇒ paren call: `add[a b]` ⇒ `add(a b)`, `str-cat[…]` ⇒
  `str-cat(…)`, `map(f [1 2 3])` stays (already paren; the list is a value),
  `eq[[1 2] [1 2]]` ⇒ `eq([1 2] [1 2])`, `get[xs 1]` ⇒ `get(xs 1)`, etc.
- record call sugar ⇒ paren call: `shout{phrase: x}` ⇒ `shout({phrase: x})`,
  `byte-add{a: 100 b: 50}` ⇒ `byte-add({a: 100 b: 50})`,
  `delete-file{path: … force: …}` ⇒ `delete-file({path: … force: …})`.
- zero-arg: `args[]` ⇒ `args()`, `gensym[]` ⇒ `gensym()`, `fresh-pair[]` ⇒
  `fresh-pair()`, `count-down(100000)` already paren.
- list/record **values** keep brackets/braces unchanged.
- TitleCase forms unchanged in spelling.

Specific examples that need attention:
- `E['syntax-quote-call']`: ``Quote delete-file{path: "foo.md" force: true}`` ⇒
  ``Quote delete-file({path: "foo.md" force: true})``. The quoted result is now
  a **tuple** `(delete-file, {…})` rather than a variant — the generator
  recomputes the printed value automatically.
- `E['values-quote-days']`: ``Quote days(30)`` already paren ⇒ quotes to the
  tuple `(days, 30)`.
- `E['sf-quote']`: ``Quote add[1 mul[2 3]]`` ⇒ ``Quote add(1 mul(2 3))`` ⇒
  value `(add, 1, (mul, 2, 3))`.
- `E['std-read']`: the **string argument** also uses old syntax —
  ``read("add[1 2]")`` ⇒ ``read("add(1 2)")``. (The string is parsed by `read`,
  so it must be new syntax too.) Result becomes the tuple `(add, 1, 2)`.
- `E['std-form-kind']`: ``[form-kind(42) form-kind("hi") form-kind(Quote foo)
  form-kind(Quote foo(1)) form-kind([1 2])]``. With calls as tuples,
  `form-kind(Quote foo(1))` now yields `"tup"`, not `"call"`. The generator
  recomputes the value; if the example's prose/comment implies a `"call"` kind,
  update it (a quoted call is a tuple now). Consider adding a case that still
  shows `"call"` for a runtime variant, e.g. `form-kind(ok(1))`, if you want to
  keep the `"call"` kind documented — optional.
- `E['macro-expand']`: ``expand(Quote And(p q))`` is unchanged in spelling and
  should still expand (relies on Step 04's `expand` over tuples).
- Macro-body examples (`sf-defmacro`, `macro-swap`, `macro-and`, `macro-trylet`,
  …): convert the bracket/record sugar inside `Quasi` bodies and `Match`
  clauses too (e.g. `Quasi [Unquote(b) Unquote(a)]` keeps the list value;
  `Quasi If Unquote(a) Unquote(b) false` is unchanged; `add[n 1]` ⇒ `add(n 1)`).

Do **not** hand-edit `docs/examples.json` — the generator produces the expected
`value`/`output`/`error` by actually running each snippet through the (rebuilt)
interpreter. Just get the sources right.

## Regenerate

Run the repo's regeneration script (see `CLAUDE.md`):
```
./scripts/regen-examples.sh
```
This builds the wasm (`wasm-pack build --target web --out-dir docs/src/wasm
--out-name wavelet`), runs `node docs/scripts/gen-examples.mjs`, then `cargo
test`. Commit the regenerated `docs/examples.json` and the updated
`docs/src/wasm/*` artifacts (they are committed so CI can build the docs without
a Rust toolchain).

If the script reports an example that errors unexpectedly, fix that example's
source (it is almost always a missed bracket/brace-sugar conversion) and re-run.

## Verification

- `./scripts/regen-examples.sh` completes and `cargo test` is **fully green**
  (`tests/examples.rs` and `tests/http.rs`).
- Spot-check `docs/examples.json`: quoted-call examples now show tuple values
  like `(add, 1, (mul, 2, 3))` and `(delete-file, {path: "foo.md", force:
  true})`.

## Commit

e.g. `test(examples): rewrite doc examples to tuple-call syntax; regenerate`
