# Step 11 — LSP server (`tooling/wavelet-lsp`)

**Read `dev-notes/tuple-calls.md` (the index) first.** The language server is a
separate crate (`tooling/wavelet-lsp`, path-depends on the `wavelet` crate at
`../..`). It funnels everything through `wavelet::read_file`, so most of it
tracks the reader automatically — but its form-tree helpers pattern-match
`Node::Call`, which no longer exists after Step 06. Update them to the tuple
shape so the crate compiles.

Files: `tooling/wavelet-lsp/src/analysis.rs` (and a glance at `main.rs`). Work on
`tuple-calls`, commit as you go, no PR. Depends on Step 06 (the `wavelet` crate
no longer has `Node::Call`).

## Required changes (`analysis.rs`)

In the "form-tree helpers" section (around lines 290–320):

1. **Top-level definition scan** (the function returning `(name, kind,
   call-node-id)` for each top-level `Def`/`DefType`/`DefMacro`): replace
   ```rust
   let Node::Call(head, _) = arena.node(root) else { continue };
   ```
   with
   ```rust
   let Node::Tup(items) = arena.node(root) else { continue };
   let head = items[0];   // guard items non-empty
   ```
   and read the head name from `arena.node(head)` as before
   (`Sym`/`Qsym` ⇒ `…-MACRO`). Keep returning the root/tuple node id as the
   "call-node-id" used downstream.

2. **Defined-name extraction** (the helper doing
   `let Node::Call(_, payload) = arena.node(call_id) else { return None };` then
   reading the defined name from `payload`): a top-level form is now
   `Tup[head, name, …]`, so the defined name is `items[1]`. Replace with
   ```rust
   let Node::Tup(items) = arena.node(call_id) else { return None };
   // items[0] is the head (def-MACRO / def-type-MACRO / def-macro-MACRO)
   // the defined name is items.get(1)
   ```
   then extract the name from `items[1]` (a `Node::Sym`) as before. (Previously
   `payload` was a tuple `[name, …]`; now those elements are flat in `items`.)

3. Any other `Node::Call` match in the file: convert the same way (head =
   `items[0]`, arguments = `items[1..]`). `grep -n Node::Call
   tooling/wavelet-lsp/src` must return nothing afterwards.

The `SPECIAL_FORMS` table, `builtin_doc`, hover, diagnostics, and completion
logic need **no semantic change** — they key off head names and `read_file`,
which already produce the new shape. (Optional polish: the `Do` blurb already
reads "Do (a b …)"; you may leave the other blurbs as-is. Do not invent new
completion snippets here.)

## Verification

- Build the LSP crate on its own:
  `cargo build --manifest-path tooling/wavelet-lsp/Cargo.toml`. It must compile
  cleanly against the updated `wavelet` crate.
- If the crate has tests, run them
  (`cargo test --manifest-path tooling/wavelet-lsp/Cargo.toml`).
- Sanity: opening a `.wvl` document with a `Def`/`DefMacro` still yields document
  symbols and hover (manual, optional).

## Commit

e.g. `fix(lsp): read top-level definition forms as tuples`
