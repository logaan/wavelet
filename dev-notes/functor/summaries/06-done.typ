// dev-notes/functor/summaries/06-done.typ — final step: downstream surfaces + PR.
// Step 05 closed the parity suite (green). Step 06 sweeps the user- and
// developer-facing surfaces now that `wavelet build` emits `set` functor
// components, confirms the surfaces that need no change (grammars/LSP, the
// regenerated examples), and finalises PR #24 (out of draft, base kept on #22's
// branch). No code changed in this step — docs/notes/PR only.

#set document(title: "Step 06 summary — downstream surfaces and the PR")
#set page(paper: "a4", margin: (x: 2.1cm, y: 2.0cm), numbering: "1")
#set par(justify: true, leading: 0.62em)
#set text(size: 10pt)
#show raw: set text(font: "DejaVu Sans Mono", size: 8.5pt)
#set heading(numbering: none)
#show heading.where(level: 1): set text(size: 13pt)

= Step 06 summary — the downstream sweep is done and the PR is out of draft

The backend (steps 01–04) and the parity suite (step 05) are complete and green.
Step 06 propagates the one factual change — `wavelet build` now *emits* `set`
functor components — across every user- and developer-facing surface, confirms
the surfaces that need nothing, runs the post-edit suite, and finalises the PR.
*No code changed in this step*; it is docs, design notes, the CHANGELOG, and the
PR.

== The PR (#24)

#link("https://github.com/logaan/wavelet/pull/24") — _feat(functor): build set
functor components in the wasm backend_. It is now *out of draft* (state OPEN).
Its base branch is *kept* at `worktree-agent-ab091bae27225f3b1` — i.e. PR #22's
branch — and was *not* retargeted to `main`. The reason: *#22 is still open* (not
merged), so per the brief the build-path work stays *stacked on #22*; the PR then
shows only the build-path diff on top of #22 rather than re-presenting #22's
changes. The body was rewritten to cover the exported-resource emission
(cell→boxed-list rep, the ctor/add/contains/size/dtor bodies,
`resource.new`/`rep`/`drop` intrinsics, the `own<set>` handle lift/lower, the
qualified `alias/op` routing, and structural `eq_raw` membership), the
`tests/backend_functor.rs` parity proof, the one limitation, the six-step
structure under `dev-notes/functor/`, and the dependency on #22. *Not merged.*

== Surfaces touched

- *`docs/docs/language/type-system.mdx`.* The `:::note Building functors` block
  rewritten: `wavelet build` now emits the synthesized per-element interface as a
  real exported WIT `set` resource at interpreter parity (any element type,
  multiple instantiations per world), with the single shaping limitation —
  returning the handle over a local-record element is a WIT interface cycle and is
  rejected. The worked-example `wit` block reconciled to the *verified* `wavelet
  wit` output: the missing `use api.{point};` line inside `interface point-set`
  added (confirmed by running `cargo run -- wit` on the example source). The prose
  now states honestly that this example's `nearest-set` returns the handle over
  the local record `point` — exactly the shape `build` rejects — so `wavelet
  wit`/`run` still *synthesize/execute* it to illustrate synthesis, but it does
  not *build*; to build a `point` set, derive an ordinary result.

- *`CHANGELOG.md` (`## [Unreleased]` → `### Added`).* The functor bullet's tail —
  which falsely said `build` does not yet emit functor components — rewritten to
  say the wasm backend now builds `set` functor components at interpreter parity,
  with the one local-record handle-return WIT-cycle limitation rejected cleanly.

- *`dev-notes/dd-type-system.typ`.* The "binary functor" specialization pass (the
  emit/binary-path functor question, previously framed as the one genuinely new,
  not-yet-done piece of core functor support) marked *resolved*: the synthesized
  `set` is emitted as a guest-implemented exported WIT resource — rep is a 1-word
  cell holding a boxed-list pointer (mirrors `Value::Cell(Rc<RefCell<Value::Lst>>)`),
  membership uses structural `eq_raw` (matches the interpreter's `Value`
  equality), intrinsics `resource.new`/`rep`/`drop`, the OWN handle carried as an
  int box with methods recovering the rep via `resource.rep` — at full parity, with
  the local-record handle-return WIT-cycle limitation noted. The *other* open
  questions (functor spelling, derive surface/extent, overload declaration, name
  mangling, arenas) were left open — only the emit/binary-path question is
  resolved by this work.

== Surfaces confirmed _not_ to need changes

- *Grammars.* `docs/src/prism/wavelet.js` and
  `tooling/vscode/syntaxes/wavelet.tmLanguage.json` both tokenize `Import` via the
  generic TitleCase `macro` rule (`\b[A-Z][A-Za-z0-9]*[a-z][A-Za-z0-9]*\b`), and
  the functor record literal (`{pkg: … elem: … as: …}`) uses only existing
  punctuation, `name:` property, namespace, and string token classes. There is no
  functor-specific token to add. `tooling/neovim` is the `wavelet.nvim` *submodule*
  (not touched, per the brief) and follows the same lexer token classes. *No
  change made.*

- *LSP.* `tooling/wavelet-lsp` reuses `wavelet::lexer` (e.g.
  `wavelet::lexer::title_to_macro_name`) and already lists `Import` as a builtin
  macro completion; functor instantiation introduces no new token class and the
  hover text is not factually wrong about functors. *No change needed.*

- *Examples / regen.* `./scripts/regen-examples.sh` was run (full wasm-pack +
  cargo build + the suite). It produced *no committable diff at all* — neither
  `docs/examples.json` nor a rebuilt `docs/src/wasm` blob changed. This is
  expected: `emit`/`build`/`host` are `#[cfg(not(target_arch = "wasm32"))]`, so the
  backend change does not reach the playground wasm and the run-path behaviour is
  unchanged. Nothing to commit or revert for this surface. Regen's `cargo test`
  passed.

== Suite state

The post-edits `cargo test` is fully green: *219 passed, 0 failed, 0 ignored*
across all binaries (the docs/`.typ`/CHANGELOG edits do not touch test inputs;
the run is the confirmation). This matches step 05's count.

== Deferred follow-ups

- *Functors beyond `set`.* Only `wavelet:coll/set` is exercised; if the functor
  package set grows (`map`, `vec`, …), each new generic structure will want the
  same emit treatment — out of scope here.
- *The local-record handle-return WIT cycle.* Returning the `set` handle over a
  local-record element stays rejected. Lifting it needs hoisting the element
  record into a shared types interface so the dependency is one-directional — a
  follow-up, not part of this build.
- *Deeper LSP type-awareness.* The LSP is token/completion-level; richer
  functor-aware analysis (e.g. resolving `alias/op` signatures from the
  instantiation) is out of scope.
