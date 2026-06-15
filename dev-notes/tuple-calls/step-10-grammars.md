# Step 10 — Syntax-highlighting grammars

**Read `dev-notes/tuple-calls.md` (the index) first.** Three grammars highlight
Wavelet and must track the lexer/attachment rule (see `CLAUDE.md` and
`tooling/README.md`). The only change: a **call head** is now a name attached to
`(` **only** — not `[` or `{`. Restrict each grammar's call-head pattern to `(`.

Work on `tuple-calls`, commit as you go, no PR. The Neovim grammar is a git
submodule and needs the extra push/pointer-bump procedure below.

## 1. Prism — `docs/src/prism/wavelet.js`

The `'function'` (call-head) token currently is:
```js
pattern: /%?[A-Za-z][\w-]*(?:\/%?[\w-]+)?(?=[([{])/,
```
Change the lookahead from `[([{]` to just `(`:
```js
pattern: /%?[A-Za-z][\w-]*(?:\/%?[\w-]+)?(?=\()/,
```
Update the comment on lines ~47–48 ("followed by `( [ or {`") to say "followed
by `(`". A name followed by `[`/`{` is now an ordinary name/reference, not a call
head — no other change needed.

## 2. VS Code — `tooling/vscode/syntaxes/wavelet.tmLanguage.json`

The `call-head` rule currently matches:
```json
"match": "%?[A-Za-z][\\w-]*(?:/%?[\\w-]+)?(?=[(\\[{])"
```
Change the lookahead to `(` only:
```json
"match": "%?[A-Za-z][\\w-]*(?:/%?[\\w-]+)?(?=\\()"
```
Update its `comment` to drop `[`/`{`. No grammar-structure change otherwise.

## 3. Neovim — `tooling/neovim` submodule (`logaan/wavelet.nvim`)

This is a **separate git repo** vendored as a submodule; an ordinary commit here
does not move it. Procedure (from `CLAUDE.md`):
1. Ensure it is checked out: `./scripts/init-submodules.sh` (a fresh clone leaves
   it empty; `git submodule status tooling/neovim` showing a leading `-` means
   not checked out).
2. Edit `tooling/neovim/syntax/wavelet.vim`: find the call-head highlighting
   (a name immediately followed by `(`/`[`/`{`) and restrict it to `(`. Update
   any token-class note in the submodule's `README` if it lists the attachment
   rule.
3. Commit **and push** that change inside `tooling/neovim` (its `origin` is
   `github.com/logaan/wavelet.nvim`).
4. Back in this repo, stage the moved submodule pointer: `git add tooling/neovim`,
   and commit it here so the repo records the new `wavelet.nvim` commit.

If you cannot push to `logaan/wavelet.nvim` (no credentials), make the edit and
commit it locally in the submodule, stage the pointer, and **flag in the PR
description (Step 12) that the `wavelet.nvim` push is outstanding** so a human can
push it. Do not skip the edit silently.

## Verification

- Eyeball: `name(` highlights `name` as a function/call head; `name[` and
  `name{` highlight `name` as a plain name (no function face) in all three
  grammars.
- These are highlighting-only; `cargo test` is unaffected.

## Commit

e.g. `build(grammars): call heads attach only to ( in Prism/VSCode/Neovim`
(plus the submodule pointer bump for Neovim).
