// dev-notes/functor/plan/00-agent-rules.typ
// MANDATORY rules for every agent executing a step in this plan. Read in full
// before doing anything. These are relayed verbatim from the repo owner's
// working preferences (which live in an un-checked-in file you will not see).

#set document(title: "Functor plan — agent rules")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm))
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#set heading(numbering: none)

= Agent rules — read before doing anything

== Worktree isolation & the shared branch

- This entire feature lives on ONE branch, `worktree-functor-build`, in the
  worktree `.claude/worktrees/functor-build`. That branch was created from PR
  #22's tip, so it already contains the run-path code and the parity-test
  harness (`tests/functor_runtime.rs`, the `set-*` builtins).
- Before your first edit, enter that worktree:
  `EnterWorktree path=.claude/worktrees/functor-build`. Do NOT create a new
  branch and do NOT create a new worktree. Do NOT edit the shared checkout
  directly.
- If `EnterWorktree path=.claude/worktrees/functor-build` fails because the
  worktree no longer exists, STOP and report it — do not improvise a new branch.

== Commit as you go

- Commit to `worktree-functor-build` incrementally — small, logical commits that
  capture each meaningful step, not one giant commit at the end.
- Follow the repo's commit-message style (see `git log`: `feat:` / `fix:` /
  `refactor:` / `docs:` / `test:` prefixes where they fit).
- End every commit message with these two trailers, exactly:

```
Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KGe633NMqR4TY2QgA1K8rm
```

== Do NOT open a PR (except step 06)

- This whole feature lands as ONE PR, opened by step 06 only. Steps 01–05 must
  NOT open a PR, must NOT push to `origin/main`, and must NOT merge anything.
- Just commit to `worktree-functor-build`. Step 06 opens the single PR.

== The interpreter is the oracle (the one hard rule)

- `interp.rs` plus the `set-*` builtins in `builtins.rs` (landed by PR #22)
  define what every `set` operation MEANS. The wasm backend must agree with them
  on every program. A backend that diverges from the interpreter is a bug —
  this is the project's one hard rule (`CLAUDE.md`: "a wasm-backend change that
  diverges from the interpreter is a bug").
- Set membership uses structural `Value` equality — the same equality the `eq`
  builtin computes (see the `set-add` comment in `builtins.rs`). The backend
  must match this with the existing `eq_raw` core helper. Do NOT require
  `Derive Eq` on the element type, and do NOT invent a different equality.

== Hand-off

- When done, write your summary as the typst file named in your step brief,
  under `dev-notes/functor/summaries/`, and commit it. The next agent trusts
  that summary and should not need to redo your investigation — make it concrete
  (exact names, signatures, `file:line` pointers, gotchas, and the build state
  you leave behind).
- If you spawn a further sub-subagent, copy these rules into its prompt verbatim
  and tell it to relay them onward.

== Scope discipline

- Stay inside the "isolated area" your step brief names. Do not refactor
  unrelated code. If you discover the previous summary was wrong, fix the
  minimum needed, note it in your own summary, and continue.
