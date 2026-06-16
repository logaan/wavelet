# Blueberries 🫐

Low-hanging fruit picked out of `notes.md`: small, concrete, bounded wins that
each fit in a single small PR. Ordered by value/effort — best ratio first.

1. [x] **Make `wavelet read` default to stdin.** Today you must pass
   `/dev/stdin` explicitly (`src/main.rs` only matches `read <path>`). Add a
   no-path arm that reads stdin. One file, removes a real wart.
1. [x] **Stop framing `Quote` as an "Inside the playground" feature** in the
   docs — it's a real special form, not playground-only. Trivial prose fix.
1. [ ] **Soften the "argument" language in the docs.** The wording is too
   strong.
1. [ ] **Soften the "NO-FFI!!" example.** The wording is too strong.
1. [x] **Add a `README` to `scripts/`.** Ten scripts, no README — a short table
   of what each does and when to run it (several are already described in
   `CLAUDE.md`).
1. [ ] **Add a "Trivia" callout that beginners can ignore.** Use it for the
   "Lisp-1" mention so it doesn't derail newcomers.
1. [ ] **Support `#!/usr/bin/env wavelet` shebang lines** so `.wvl` files can be
   run directly as scripts.
   1. Run them as though they've imported wasi:cli, and give them a full wasi host.
   1. Possibly treat code outside of fn definitions as though it's in `run` (or whatever the cli method is called)
1. [ ] **Move the formal grammar specification to an appendix** — it currently
   lands too early in the learning flow.
1. [x] **Give the sugar cases their own numbered docs page** — enumerate each
   reader-sugar form on a dedicated, numbered page.
1. [ ] **Test the VS Code tooling** — verify the TextMate grammar + language
   config work end-to-end.
1. [ ] **Pick a nicer docs font** — the current one isn't loved (low priority,
   subjective).
