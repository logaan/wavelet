# Step 06 — Wasm backend (`emit.rs`) + remove `Node::Call`

**Read `dev-notes/tuple-calls.md` (the index) first.** This step makes the wasm
emitter compile the new tuple-call forms, then removes the now-unused
`Node::Call` enum variant and any residual dead arms across the crate.

Files: `src/emit.rs` (primary), `src/form.rs` (delete the variant), and dead-arm
cleanup in `src/printer.rs`, `src/value.rs`, `src/interp.rs` (any leftovers).
Work on `tuple-calls`, commit as you go, no PR. Depends on Steps 01–05.

The emitter only ever sees **fully expanded** forms (macros/`Quote`/`Quasi` are
gone by emit time — the backend already errors on `quote-MACRO`/`quasi-MACRO`).
So every `Node::Tup` reaching the emitter is a **call**, never a literal tuple
value. Tuple *values* at runtime are produced by builtins and by the
argument-bundling below, not by literal `Tup` forms.

`src/emit.rs` is large. The relevant functions (current line numbers, may shift):
`scan` (~375), `expr` (981), `seq_box`/`list_box` (1062/1068), `fn_form` (1220),
`closure_call` (1308), `payload_items` (1334), `call` (1341), `if_form`/`do_form`/
`let_form`/`match_form` (1390–1469), `pattern` (1474), `seq_pattern` (1568),
`bind_args` (1597), `internal_call` (1621), `dep_call` (1637), `builtin` (2651).
There are 4 `Node::Call` sites total — find them with `grep -n Node::Call`.

## The argument-bundling rule (mirror of the interpreter)

A call form is `Tup[head, arg1, …]`; `args = &items[1..]`. When a callee needs a
single "payload" box (closures, builtins), build it from `args`:
- 0 args ⇒ empty **list** box (`self.list_box(fx, &[])`)
- 1 arg  ⇒ `self.expr(fx, args[0], false)`
- ≥2 args ⇒ `self.list_box(fx, args)`

Add a helper, e.g. `fn payload_box(&mut self, fx, args: &[NodeId]) -> Result<()>`,
implementing exactly that. (This matches the interpreter's bundling and the
existing `bind_params`/`bind_args` binding: a record arg binds by name, a
list/tuple by order, a scalar to the sole parameter.)

## Changes

### `expr` (981)
Replace the `Node::Call(head, payload) => return self.call(…)` and
`Node::Tup(items) => return self.seq_box(fx, &items, TAG_TUP)` arms with one:
```
Node::Tup(items) => {
    let items = items.clone();
    if items.is_empty() {
        return Err("cannot evaluate empty form ()".into());
    }
    return self.call(fx, items[0], &items[1..], tail);
}
```
(`seq_box(TAG_TUP)` is still used elsewhere to build tuple value boxes — keep the
function; just don't use it for literal `Tup` forms here.)

### `call` (1341) — take `args: &[NodeId]`
Change the signature to `fn call(&mut self, fx, head: NodeId, args: &[NodeId],
tail: bool)`. Keep the head dispatch, passing `args` to each handler:
- `if-MACRO` ⇒ `if_form(fx, args, tail)`
- `do-MACRO` ⇒ `do_form(fx, args, tail)`
- `let-MACRO` ⇒ `let_form(fx, args, tail)`
- `the-MACRO` ⇒ `self.expr(fx, args[1], tail)` (args = `[ty, expr]`)
- `match-MACRO` ⇒ `match_form(fx, args, tail)`
- `fn-MACRO` ⇒ `fn_form(fx, args)`
- `quote/quasi/def/def-macro-MACRO` ⇒ same "not supported by backend" error
- closure (`fx.lookup`) ⇒ `closure_call(fx, head, args, tail)`
- builtin ⇒ `builtin(fx, &name, args)`
- internal func ⇒ `internal_call(fx, &name, args, tail)`
- value global ⇒ `closure_call(fx, head, args, tail)`
- Qsym ⇒ `http_call`/`dep_call` (pass `args`)
- other head ⇒ `closure_call(fx, head, args, tail)`

### Special-form helpers — read from `args`
- `if_form(fx, args, tail)`: `(c, t, e) = (args[0], args[1], args[2])`.
- `do_form(fx, args, tail)`: arity 1; the body list is `args[0]`. Read
  `Node::Lst(items)` from `args[0]` (empty ⇒ unit), then sequence as today.
- `let_form(fx, args, tail)`: bindings `Rec` = `args[0]`, body = `args[1]`.
- `match_form(fx, args, tail)`: scrutinee = `args[0]`, clause `Lst` = `args[1]`;
  each clause is still a `Node::Tup(pair)` of length 2 — unchanged.
- `fn_form(fx, args)`: params = `args[0]`, body = `args[1]` (it currently reads a
  `Tup` payload `[params, body]`; now those are `args[0]`/`args[1]`). Keep the
  closure-capture logic intact.

### `pattern` (1474) — variant vs tuple patterns
Remove the `Node::Call(head, vpayload)` arm and extend the handling of
`Node::Tup`. Disambiguate **by the pattern's first element** (matches all current
examples; note the limitation in a comment):
- `Node::Tup(pats)` where `pats[0]` is `Node::Sym(case)` ⇒ **variant-case
  pattern** (the old `Node::Call` logic): check the box tag is `TAG_VAR`, the case
  name matches (`eq_raw`), then match the rest against the payload:
  - `pats.len() == 1` ⇒ payload must be absent (load offset 8, `BrIf(fail)`),
    as the `none` arm does.
  - `pats.len() == 2` ⇒ load the payload box (offset 8; mismatch if 0) and
    recurse `self.pattern(fx, pats[1], inner, fail)`.
  - `pats.len() > 2` ⇒ the payload box is a tuple; `seq_pattern(fx, &pats[1..],
    inner, fail, TAG_TUP)`.
- `Node::Tup(pats)` where `pats[0]` is **not** a `Sym` ⇒ **tuple-destructure**:
  `self.seq_pattern(fx, &pats, v, fail, TAG_TUP)` (the old `Node::Tup` arm).
The `none` arm, bare-`Sym` bind arm, literal arms, `Lst`/`Rec` arms are
unchanged.

### `bind_args` (1597) — take `args: &[NodeId]`
Rewrite to mirror the interpreter's `bind_params`, returning the per-parameter
argument node-ids to emit in order:
- If `args.len() == 1` and `node(args[0])` is a `Rec` whose sorted keys equal the
  sorted `params` ⇒ return the field value nodes in `params` order (named).
- Else if `args.len() == params.len()` ⇒ return `args.to_vec()` (positional;
  also covers `params.len() == 1, args.len() == 1` scalar, and the 0/0 case).
- Else if `params.len() == 1` (and `args.len() != 1`) ⇒ the sole parameter
  receives the **bundle** of all args as a tuple. `bind_args` cannot return a
  node for a synthesized tuple, so signal this case to the caller (e.g. return a
  dedicated error/enum, or have the caller detect `params.len()==1 &&
  !named && args.len()!=1`). In that case the caller emits a single tuple box via
  `self.seq_box(fx, args, TAG_TUP)` instead of looping. (This case does not occur
  in the current templates/examples; implement it for fidelity but a clear
  unsupported-error is acceptable if it complicates `dep_call`.)
- Else ⇒ `Err("payload does not match parameters (…)")`.

### `internal_call` (1621) / `dep_call` (1637)
Take `args: &[NodeId]`. For the normal cases, loop over `bind_args(args,
&params)?` and `expr`/lower each as today. For the 1-param bundle case (above),
push one tuple box (`seq_box(fx, args, TAG_TUP)`) — for `dep_call` this means the
imported function's single tuple-typed param, which is rare; a clear error is
acceptable there if needed.

### `closure_call` (1308) — take `args: &[NodeId]`
Replace the `match node(payload) { Lst|Tup => list_box; _ => expr }` block with a
call to the `payload_box(fx, args)` helper (0 ⇒ empty list box, 1 ⇒ expr, ≥2 ⇒
list box). Everything else (loading the table slot, `CallIndirect`) is unchanged.

### `builtin` (2651) — take `args: &[NodeId]`
This is large but the changes are mechanical:
- Every `self.payload_items(payload)` becomes `args` (a `&[NodeId]`; clone to a
  `Vec` if a site needs ownership).
- Every place that emits the **whole payload as one value** (currently
  `self.expr(fx, payload, false)`, or `match node(payload) { Lst|Tup => …; _ => …}`)
  becomes `self.payload_box(fx, args)?`.
- The per-builtin arity logic (indexing `items[0]`, `items[1]`, …) is unchanged
  once `items = args`.
Then delete `payload_items` (1334) if nothing else uses it, or leave it returning
`args.to_vec()` for convenience.

### `scan` (375)
Replace the `Node::Call(head, payload)` arm with `Node::Tup(items)`: take
`head = items[0]`, inspect it for `print`/`println`/`args`/`Qsym` (dep calls) as
today, then recurse over **all** `items`. (The current `Node::Tup(xs) | Node::Lst`
arm can stay for `Lst`; fold `Tup` into the new call-aware arm.)

## Cleanup — remove `Node::Call`

Once `emit.rs` compiles and no code references `Node::Call`:
- Delete the `Call(NodeId, NodeId)` variant from the `Node` enum in
  `src/form.rs`.
- Remove the now-dead `Node::Call` match arms left behind in `src/printer.rs`
  (the `write_form` `Call` arm), `src/value.rs` (`form_to_value` `Call` arm), and
  anywhere else `grep -rn "Node::Call" src/` still reports.
- Update the doc comment on the `Node` enum if it mentions `Call`.

`grep -rn "Node::Call" src/` must return nothing at the end of this step.

## Verification

- `cargo build` green, **no warnings** about an unconstructed `Call` variant.
- `cargo clippy` (if configured) clean for the touched files.
- Build a small component end-to-end through the emitter, e.g.
  `cargo run -- build <file>` (or `wavelet::build::build_files`) on:
  ```
  Package "demo:shout@0.1.0"
  Export shout
  Def shout Fn {phrase: string} str-cat(upper(phrase) "!")
  ```
  It must produce a validating component (the build path runs the component
  encoder with validation). Try one with `Match`, `If`, and a record-arg call to
  exercise patterns and bundling.
- `cargo test --test http` will pass only after Step 07 rewrites the scaffold
  templates; it is fine for it to stay red here. `cargo build` green is the gate
  for this step.

## Commit

Suggested commits:
- `feat(emit): compile tuple-call forms; bundle args at the boundary`
- `refactor(form): remove the Node::Call variant; calls are tuples`
