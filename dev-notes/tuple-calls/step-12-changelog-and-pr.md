# Step 12 — CHANGELOG, final verification, open the PR

**Read `dev-notes/tuple-calls.md` (the index) first.** This is the final step:
record the change in the changelog, run the whole verification gate end to end,
then open the pull request for human review. Depends on Steps 01–11 all being
committed on `tuple-calls`.

Work on `tuple-calls`. Commit, then **open a PR** (`gh pr create`). Do **not**
merge it and do **not** push to `origin/main`.

## 1. CHANGELOG

Edit `CHANGELOG.md` (Keep a Changelog format). Under `## [Unreleased]`, add
user-visible entries, e.g.:

```
### Changed
- Function calls are now WAVE **tuples** with the head first: `foo(1 "baz")`
  reads and prints as `(foo, 1, "baz")` (previously a variant case). Special
  forms and macros share the shape: `If c t e` is `(if-MACRO, c, t, e)`.
  Evaluating a parenthesized form is a call; literal tuple values come from
  `Quote` or builtins. `(foo)` is a zero-argument call (parenthesized grouping
  is gone). `form-kind` reports `tup` for quoted calls.

### Removed
- List/record call sugar `foo[a b]` and `foo{k: v}`. Write `foo([a b])` and
  `foo({k: v})` instead. Attaching `[` or `{` to a name is now a read error that
  points at the new spelling.
```

Match the file's existing heading/wording style; group under the right
`Added`/`Changed`/`Removed` subsections.

## 2. Full verification gate

Run, from the repo root, and confirm each is clean:

- `cargo build` — green, no `Node::Call`/dead-code warnings.
- `./scripts/regen-examples.sh` — rebuilds the wasm, regenerates
  `docs/examples.json`, and runs `cargo test`. Must finish green. If it changed
  any generated artifact (`docs/examples.json`, `docs/src/wasm/*`), commit it.
- `cargo test` — full suite green (`tests/examples.rs`, `tests/http.rs`).
- `cargo build --manifest-path tooling/wavelet-lsp/Cargo.toml` — LSP compiles.
- `grep -rn "Node::Call" src/ tooling/wavelet-lsp/src` — returns nothing.
- `grep -rn 'str-cat\[\|args\[\]' .` (and a scan for other `name[`/`name{` call
  sugar) — only legitimate value literals remain; no call sugar lingers in
  shipped `.wvl`, templates, docs, or examples.
- Confirm `tooling/neovim` submodule pointer is staged/committed (Step 10); note
  in the PR if its upstream push is still outstanding.

If anything fails, fix it on the branch (or hand back to the relevant step's
owner) before opening the PR — do not open a PR on red.

## 3. Open the PR

```
git push -u origin tuple-calls
gh pr create --base main --head tuple-calls \
  --title "Calls are tuples, not variants" \
  --body "<summary>"
```

PR body should cover:
- **What changed**: calls are now head-first tuples; `[`/`{` call sugar removed;
  grouping removed; special forms/macros share the tuple shape; `Quote`/`form-kind`
  semantics; pattern matching of variant vs tuple forms.
- **Why / decisions**: link or restate the model from `dev-notes/tuple-calls.md`
  (the index), noting the owner-approved choices ("everything in eval position is
  a call", "(foo) is a 0-arg call", "special forms are tuples too").
- **Scope**: reader, interpreter, expander, runner, builtins, WIT synthesis, wasm
  backend, scaffold templates, examples, docs, three grammars, LSP.
- **Verification**: the gate above is green.
- **Caveats**: any outstanding `wavelet.nvim` submodule push (Step 10), and the
  emit pattern-matching limitation noted in Step 06 (a `Tup` pattern with a
  `Sym` head is compiled as a variant-case pattern).
- End the body with:
  `🤖 Generated with [Claude Code](https://claude.com/claude-code)`

A human reviews and merges. Do not merge yourself.

## Commit

e.g. `docs(changelog): record the tuple-call change` (the PR itself is the
deliverable; the branch is already built up from Steps 01–11).
