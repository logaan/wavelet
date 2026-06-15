# Step 09 — Documentation prose & design doc

**Read `dev-notes/tuple-calls.md` (the index) first.** Update the human-readable
documentation to describe calls-as-tuples and the removed sugar. This is prose
only — the runnable examples were handled in Step 08.

Files: `docs/docs/**/*.mdx` (the Docusaurus site) and `dev-notes/design.md` (the
language design draft). Work on `tuple-calls`, commit as you go, no PR. Depends
on Step 08 (so prose matches regenerated examples).

## `dev-notes/design.md` — the load-bearing edits

1. **§2.2 The attachment rule.** Now only `(` attaches to an identifier to form a
   call. State that `[` and `{` no longer attach (the list/record call sugar is
   removed); attaching them is a read error pointing at `name([…])` / `name({…})`.

2. **§2.3 Desugaring.** Rewrite the canonical-form table. Calls are tuples with
   the head first:
   - `f(x)` ⇒ `(f, x)`
   - `f(x y)` ⇒ `(f, x, y)`
   - `f()` ⇒ `(f)` (0-arg call)
   - `f{a: 1 b: 2}` and `f[x y]` rows: **remove** (sugar gone); show the new
     spellings `f({a: 1, b: 2})` and `f([x, y])` instead.
   - `(a)` is now a **0-arg call** of `a`, not transparent grouping — update/
     remove the grouping row.
   - The prose "Function calls are variant cases" must become "function calls are
     tuples whose first element is the head." A bare identifier is still a
     variable reference (a payload-less variant case under `Quote`, i.e. a
     symbol).

3. **§2.4 Macro sugar.** `If c t e` now desugars to the tuple
   `(if-MACRO, c, t, e)` (was `if-MACRO((c, t, e))`). Update the worked example
   and the "explicit payload override" note: `If(c t e)` reads identically to the
   arity form (both `(if-MACRO, c, t, e)`).

4. **§3 Values.** The `tuple<…>` row and surrounding prose should note that a
   parenthesized form in evaluation position is a **call**, so a literal tuple
   value is written with `Quote` (or produced by a builtin). Update the
   `delete-file{…}` / `delete-file[…]` examples to the paren-call spelling.

5. **§4.1 Evaluation rules.** Rule 3 ("a call form `head(payload)`") becomes: a
   tuple `(head arg…)` in evaluation position is a call — evaluate the args,
   bundle them (0 ⇒ empty, 1 ⇒ the value, ≥2 ⇒ a tuple), and apply `head`.
   Mention that evaluating a tuple whose head is not callable is an error, and
   that tuple values come from `Quote`/builtins.

6. **§4.2 / §5 examples.** Convert every inline call in the code samples
   (`str-cat[…]`, `add[n 1]`, `delete-file{…}`, `delete-file[…]`, `count-down(…)`
   already paren, `sum-to[…]`, `args[]`, `sh/shout{…}`, …) to the new syntax.

7. **§6.2 Code as a WIT type.** This is important: the `form`/`node` WIT type
   currently lists a `call(tuple<node-id, node-id>)` case. Since calls are now
   tuples, **remove the `call` case** from both the conceptual `form` grammar and
   the `node` `variant` in the WIT snippet. A call is just a `tup`. Update the
   surrounding prose (`Quote` "hands you a natural tree"; a call form is a tuple
   node).

8. **§7 / §8 worked examples and Appendix A grammar.** Convert call spellings.
   In Appendix A, update the `call`/`payload`/`tuple`/`group` productions: only
   `(` attaches; `payload := "(" form* ")"`; a parenthesized form is a tuple
   (which is the call form); remove the transparent `group` production; remove
   the `[`/`{` payload alternatives. Update Appendix B (design ledger) note about
   call representation if it mentions variants.

## `docs/docs/**` — the site

Update prose and inline (non-`<Playground>`) code in at least:
- `language/syntax.mdx` — attachment rule, desugaring table, removed sugar.
- `language/evaluation.mdx` — call = tuple; bundling; tuple values via `Quote`.
- `language/special-forms.mdx` — `If`/`Let`/`Match`/`Fn` canonical tuple shape.
- `language/pattern-matching.mdx` — variant-case patterns `(ok x)` vs tuple
  patterns; how they read.
- `language/macros.mdx` — `Quote`/`Quasi` produce tuples; `expand` over tuples.
- `intro.mdx`, `getting-started.mdx`, `components.mdx`, `cli.mdx` — convert any
  inline call spellings (`str-cat[…]`, `name{…}`, `args[]`, …).
Search broadly: `rg -n '\w+\[|\w+\{' docs/docs` to find call-sugar spellings, but
read each hit — list/record **values** (`[1 2 3]`, `{k: v}`) are fine and must
not be changed.

Static ```` ```wavelet ```` code blocks are highlighted by the Prism grammar
(Step 10) but their *content* is prose you edit here.

## Verification

- `rg -n 'str-cat\[|args\[\]|\w+\{[a-z]' docs/docs dev-notes/design.md` turns up
  no stale call-sugar (after distinguishing genuine value literals).
- If the docs site builds locally (`npm --prefix docs run build` or similar),
  it builds without broken-example errors. The runnable examples come from
  `examples.json` (Step 08), so this step should not change test outcomes.
- `cargo test` remains green (no code changes here).

## Commit

e.g. `docs: describe calls as tuples; drop list/record call sugar`
