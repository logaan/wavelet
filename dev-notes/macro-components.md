# Macro components — running macros defined in other components

This is the design overview **and** the execution checklist for the feature
described in `dev-notes/design.md` §6.2–§6.3: the ability to import a macro
library that lives in *another component* and use its macros as if they were
local. The work is split into small, self-contained steps, **one per subagent**.

Read this file **and** the relevant sections of `dev-notes/design.md` (§2.4 the
macro sugar, §6.2 code as a WIT type, §6.3 macros are components) before
starting any step.

## What we're building

Today macros are local-only. `DefMacro` registers a `Value::Macro` closure, the
reader tracks each macro's arity in a `MacroTable` as it reads top-to-bottom
(`src/reader.rs`), the ahead-of-time expander rewrites the form tree before
codegen (`src/expand.rs`), and the interpreter expands lazily at eval time
(`src/interp.rs`). There is an `expand` builtin that runs one expansion step.

The design (§6.3) says macros need not be written in Wavelet, or even live in the
same file. A component that exports `wavelet:meta/macros` is a *macro library*:

```wit
package wavelet:meta@0.1.0;

interface macros {
  use code.{tree};
  manifest: func() -> list<tuple<string, u32>>;          // (name, arity) pairs
  expand: func(name: string, args: tree) -> result<tree, string>;
}
```

`Import {pkg: "acme:html/dsl" macros: true}` should:

1. **instantiate that component at compile time**,
2. call `manifest()` and **register the `(name, arity)` pairs with the reader**
   (this is how foreign TitleCase arities become known — §2.4 requires every
   visible macro's arity to be known as the reader moves top-to-bottom), and
3. **route expansion through the component's `expand`** function: marshal the
   call's argument forms into a `tree`, call `expand(name, args)`, lift the
   returned `tree` back into the form arena, and recurse to fixpoint.

Two payoffs fall out (§6.3): you can write macros in **any language** that
compiles to a component (e.g. a `Re` macro that builds a regex DFA at build
time in Rust), and macro expansion is **sandboxed by construction** — an
untrusted macro runs inside a wasm component with no ambient capabilities.

Name collisions within a namespace are errors, resolved by aliasing the import;
a qualified TitleCase form `Dsl/Element` disambiguates at use sites (§6.3, and
see the open item at `dev-notes/todo.md` about qualified-macro arity lookup
currently ignoring the alias).

## The two foundational facts a subagent needs

1. **The in-memory form tree already matches the wire type.** `src/form.rs`
   defines `Arena`/`Node`/`NodeId`, whose variants line up almost exactly with
   the `wavelet:meta/code` `node`/`tree` arena in design.md §6.2. So the work is
   **not** inventing a representation — it is defining the *canonical-ABI* `tree`
   value and lowering/lifting between `form::Arena` and that wire value.

2. **There is no wasm runtime in the project yet.** `Cargo.toml` pulls in
   `wac-graph`, `wasm-encoder`, `wit-component`, `wit-parser` — composition and
   WIT tooling, but nothing that *executes* a component. Running a foreign
   component's `manifest`/`expand` at compile time is net-new infrastructure
   (Step 2 adds a component runtime, e.g. `wasmtime`).

## How this worklist is driven

- **One subagent per step.** The orchestrator spawns a fresh agent for the next
  unchecked step; that agent does *only* that step, then stops. The step
  boundaries are deliberate handoff points — don't run ahead.
- **All steps accumulate on one integration branch and land as a single PR.**
  Unlike a normal one-task-one-PR change, this is a multi-step feature tracked as
  **one** pull request. Each step branches from and pushes back to a shared
  integration branch `macro-components` (created off `origin/main` by Step 1).
  A subagent's worktree is created fresh, so it can only see prior steps that
  were actually pushed to `macro-components` — the last thing every step does is
  push to `origin/macro-components`. Intermediate steps **do not** open their own
  PRs. **Step 11 (the final step) opens the single PR** from `macro-components`
  to `main`; a human reviews and merges.
- **Update this file as part of the step.** Tick the step's box and fill in its
  "Handoff notes" (decisions, surprises, follow-ups), committed together with the
  step's work.
- **Every step ends green.** `cargo test` must pass at the end of *every* step.
  Run `./scripts/regen-examples.sh` for any step that touches language behaviour
  or the example set.

## Rules every subagent must follow (paste verbatim into each subagent prompt)

`CLAUDE.local.md` is untracked and absent from worktrees, so relay these to every
subagent (and tell it to relay them onward if it spawns further agents):

- Before any edit, isolate with the **EnterWorktree** tool — your own worktree,
  your own branch, based on the latest `origin/macro-components`. Never edit the
  shared checkout.
- Commit as you go: small, logical commits in the repo's style (`feat:`,
  `refactor:`, `docs:`, …), not one giant commit at the end.
- When the step is complete **and verified** (`cargo test`; plus
  `./scripts/regen-examples.sh` if you touched language behaviour or examples),
  push to `origin/macro-components` (fast-forward / rebase cleanly onto it; if you
  can't resolve a conflict safely, stop and report rather than force-pushing).
  **Do not push to `origin/main` and do not open a PR** — except Step 11, whose
  job *is* to open the PR.
- Do exactly one step from this file. Tick its box and write its Handoff notes in
  the same commit. Do not start the next step.

## Verification quick-reference

- `cargo test` — always.
- `./scripts/regen-examples.sh` — after any language/example change (regenerates
  `docs/examples.json`, rebuilds the docs wasm, re-locks `tests/examples.rs`).

## Steps

| # | Step | File |
|---|------|------|
| 1 | The `wavelet:meta/code` `tree` wire type + arena ↔ wire conversion | [`step-01-tree-wire-type.md`](macro-components/step-01-tree-wire-type.md) |
| 2 | A compile-time component runtime (instantiate + call exports) | [`step-02-component-runtime.md`](macro-components/step-02-component-runtime.md) |
| 3 | The `wavelet:meta/macros` interface + a `manifest`/`expand` caller | [`step-03-meta-macros-interface.md`](macro-components/step-03-meta-macros-interface.md) |
| 4 | Parse `Import {… macros: true}` and thread the flag through | [`step-04-import-macros-flag.md`](macro-components/step-04-import-macros-flag.md) |
| 5 | Resolve a `macros: true` import to a `.wasm` and instantiate it | [`step-05-resolve-macro-component.md`](macro-components/step-05-resolve-macro-component.md) |
| 6 | Register foreign macro arities from `manifest()` with the reader | [`step-06-manifest-arity-registration.md`](macro-components/step-06-manifest-arity-registration.md) |
| 7 | Route expansion through the component's `expand` | [`step-07-route-expansion.md`](macro-components/step-07-route-expansion.md) |
| 8 | Qualified references, aliasing, and collision errors | [`step-08-qualified-refs-and-collisions.md`](macro-components/step-08-qualified-refs-and-collisions.md) |
| 9 | Produce a `wavelet:meta/macros` component from a Wavelet macro file | [`step-09-produce-macro-component.md`](macro-components/step-09-produce-macro-component.md) |
| 10 | End-to-end example + docs + highlighting + CHANGELOG | [`step-10-e2e-and-docs.md`](macro-components/step-10-e2e-and-docs.md) |
| 11 | Final verification and raise the PR | [`step-11-raise-pr.md`](macro-components/step-11-raise-pr.md) |

### Dependency notes

- Steps 1–3 are the **infrastructure spine** (wire type → runtime → the macros
  contract) and must land in order.
- Step 9 (the *producer* — compiling a Wavelet `DefMacro` file into a macro
  component) is the largest and most independent piece. Steps 2–8 are testable
  against a small **fixture** macro component (recommended: a hand-written
  WAT/`wasm-tools`-built `.wasm`, or a tiny Rust component, checked into
  `tests/fixtures/`) so the *consumer* machinery does not block on the producer.
  Step 10's end-to-end example can then prefer the Wavelet-produced one.
