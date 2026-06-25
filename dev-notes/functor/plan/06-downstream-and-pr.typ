// dev-notes/functor/plan/06-downstream-and-pr.typ — Step 06: downstream surfaces + PR.

#set document(title: "Step 06 — downstream surfaces and the single PR")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.62em)
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#set heading(numbering: none)
#show heading.where(level: 1): set text(size: 13pt)

= Step 06 — downstream surfaces and the single PR

*First read `plan/00-agent-rules.typ`*, then `summaries/05-parity.typ`. Critical
rules: branch `worktree-functor-build` via
`EnterWorktree path=.claude/worktrees/functor-build`; commit as you go with the
two trailers. THIS is the only step that opens a PR.

== Goal

Per `CLAUDE.md`, a language/behaviour change is not done until the downstream
surfaces are checked. Functors now *build*, which changes documented behaviour.
Sweep the surfaces, verify the whole suite, and open the one PR.

== Tasks

+ *Docs — `docs/docs/language/type-system.mdx`.* The Functors section and the
  "Worked example: source to WIT" section currently describe functors as
  synthesis + run only (PR #22 may have added wording that `build` is not
  supported, e.g. "`wavelet run` supports this functor; `wavelet build` does
  not yet"). Update that prose: `build` now emits the component. Re-verify the
  `wit` block still matches real `wavelet wit` output for the worked example
  (run `wavelet wit` on it and diff).
+ *Examples — `docs/scripts/gen-examples.mjs`.* If a functor example should now
  be exercised on the build path (or any documented example's behaviour
  changed), update the generator. Then run `./scripts/regen-examples.sh` — it
  rebuilds the wasm artifact under `docs/src/wasm`, regenerates
  `docs/examples.json`, and runs `cargo test`. Commit the regenerated artifacts
  (the wasm blob is committed on purpose — CI builds docs with Node only).
+ *CHANGELOG.md — `## [Unreleased]`.* Add under `Added`: the wasm backend now
  builds `set` functor components — the synthesized per-element interface is
  emitted as a real exported WIT resource, at parity with the interpreter
  (any element type, multiple instantiations per world).
+ *Design notes — `dev-notes/dd-type-system.typ`.* Functors are listed there
  under "Open questions" for the binary/emit path. Move them to resolved /
  implemented and note the approach (exported resource: cell→boxed-list rep,
  `eq_raw` membership, `resource.new/rep/drop`).
+ *Syntax grammars + LSP.* Confirm NO change is needed and SAY SO explicitly:
  functor `Import` uses existing token classes (no new lexer tokens), so the
  three grammars (`docs/src/prism/wavelet.js`, `tooling/neovim/...`,
  `tooling/vscode/...`) and the LSP are unaffected. (If you discover otherwise,
  handle per `CLAUDE.md`, including the `wavelet.nvim` submodule push + pointer
  bump.)
+ *Final sweep.* `cargo test` green once more after the regen.

== Open the single PR

+ *Choose the base.* Check PR #22's state:
  - If PR #22 is already MERGED to `main`: rebase `worktree-functor-build` onto
    `origin/main`, resolve any conflicts, and target `main`.
  - If PR #22 is still OPEN: target its branch
    (`worktree-agent-ab091bae27225f3b1`) so this PR stacks cleanly and shows
    only the build-path diff.
  State the chosen base and the dependency on #22 in the PR body.
+ *Push and create.* Push `worktree-functor-build` to `origin` and
  `gh pr create`. Do NOT merge it; do NOT push to `origin/main`.
  - Title: `feat(functor): build set functor components in the wasm backend`.
  - Body: what changed (exported-resource emission: rep, bodies, intrinsics,
    handle lift/lower, call routing), the parity proof (`backend_functor.rs`),
    the step-by-step structure, downstream surfaces touched, and the dependency
    on PR #22. End the body with exactly:

```
🤖 Generated with [Claude Code](https://claude.com/claude-code)
```

== Write `summaries/06-done.typ`

- The PR URL and the base branch chosen (and why).
- Every downstream surface touched (and the ones confirmed not to need changes).
- Any follow-ups deferred (e.g. functors other than `set`, if the package set is
  ever extended; deeper LSP type-awareness — out of scope here).
