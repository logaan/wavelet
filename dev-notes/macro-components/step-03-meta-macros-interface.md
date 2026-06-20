# Step 3 — The `wavelet:meta/macros` interface + a `manifest`/`expand` caller

- [x] Done

> **Read first:** `dev-notes/macro-components.md` and `dev-notes/design.md` §6.3.
> Base your worktree on the latest `origin/macro-components` (after Steps 1–2).

## Context you need

A macro library is a component exporting `wavelet:meta/macros` (design.md §6.3):

```wit
interface macros {
  use code.{tree};
  manifest: func() -> list<tuple<string, u32>>;          // (name, arity) pairs
  expand: func(name: string, args: tree) -> result<tree, string>;
}
```

Step 1 gave us the `tree` type and `form::Arena` ↔ `Tree` conversion. Step 2 gave
us a runtime that can instantiate a component and call exports with dynamic
`Val`s. This step **joins them**: a typed caller that, given an instantiated
macro component, can call its two exports with proper marshalling.

## Goal

A `MacroComponent` abstraction over an instantiated `wavelet:meta/macros`
component, with two methods — `manifest() -> Vec<(String, u32)>` and
`expand(name, args: &Tree) -> Result<Tree, String>` — fully marshalling between
our `Tree` (Step 1) and the runtime's `Val`s (Step 2). Tested against a fixture
macro component.

## Scope

- **Add the `macros` interface** to the `wavelet:meta@0.1.0` WIT from Step 1
  (the `manifest`/`expand` signatures above, `use code.{tree}`).
- **`Tree` ⇄ `Val` marshalling.** Lower a `Tree` into the `component::Val`
  shape the runtime expects for the `tree` record (list of `node` variants +
  `root` + `spans`), and lift a `result<tree, string>` `Val` back into
  `Result<Tree, String>`. This is the fiddly part: the `node` variant has many
  cases, each mapping to a `Val::Variant`. Centralise it so Step 7 just calls
  `expand`.
- **`MacroComponent`** built on the Step 2 runtime: locate the `manifest` and
  `expand` exports of the `wavelet:meta/macros` interface on an instantiated
  component and wrap the two calls with the marshalling above.
- **Fixture macro component.** Extend or replace the Step 2 fixture with one that
  genuinely exports `wavelet:meta/macros`: a small set of macros is enough (e.g.
  an `unless`-style macro and an identity macro). Hand-written WAT or a tiny Rust
  component, checked into `tests/fixtures/`. Keep tests hermetic.
- **Tests:** `manifest()` returns the fixture's `(name, arity)` pairs; `expand`
  on a known call form returns the expected rewritten `tree` (compare by
  converting back to an arena and printing canonically with `printer`); an
  `expand` that returns `result::err` surfaces the error string.

## Watch out for

- **Variant case ordering / names.** The `Val::Variant` discriminant must match
  the WIT `node` variant exactly (case name and payload shape). A mismatch here
  produces confusing runtime trap/marshalling errors — test every node variant.
- **`result<tree, string>`** lifts to a `Val::Result`; map `ok`→`Ok(Tree)`,
  `err`→`Err(String)`.
- **Empty `args`.** A nullary macro call still passes a `tree` (an empty/leaf
  payload). Make sure the conversion handles the trivial cases.

## Done when

`cargo test` passes; `MacroComponent::manifest`/`expand` work end-to-end against
a fixture `wavelet:meta/macros` component, including the error path; every `node`
variant is covered by a marshalling round-trip test.

## Handoff notes

### What landed

- **WIT.** Added `interface macros` to `wit/meta/code.wit` (same
  `wavelet:meta@0.1.0` package as `code`): `use code.{tree}; manifest: func() ->
  list<tuple<string, u32>>; expand: func(name: string, args: tree) ->
  result<tree, string>;`. `meta::tests::meta_code_wit_parses` still guards it.

- **Marshalling** lives in the new native-only `src/macros.rs`
  (`pub mod macros`, gated `#[cfg(not(target_arch = "wasm32"))]` in `lib.rs`).
  Public functions, all centralised so Step 7 only calls `MacroComponent::expand`:
  - `node_to_val(&Node) -> Val`, `tree_to_val(&Tree) -> Val` (lowering)
  - `val_to_node(&Val) -> Result<Node,String>`, `val_to_tree(&Val) ->
    Result<Tree,String>`, `val_to_result_tree(&Val) -> Result<Tree,String>`
    (lifting)

- **`HostComponent` gained nested-export lookup** (`src/host.rs`):
  `instance_func(instance, func) -> Result<Func,String>` and
  `call_instance(instance, func, args) -> Result<Vec<Val>,String>`. Interface
  exports are **not** top-level funcs — `manifest`/`expand` live inside the
  exported instance `wavelet:meta/macros@0.1.0`, reached via
  `Instance::get_export_index(&mut store, None, "wavelet:meta/macros@0.1.0")`
  then `get_export_index(.., Some(&iface_idx), "manifest")` then `get_func`.
  `ComponentExportIndex` is re-exported from `host`.

### `MacroComponent` public surface (`src/macros.rs`)

```rust
MacroComponent::from_bytes(&[u8]) -> Result<MacroComponent, String>
MacroComponent::from_file(&Path)  -> Result<MacroComponent, String>
  // both verify the wavelet:meta/macros interface is present (fail fast)
MacroComponent::manifest(&mut self) -> Result<Vec<(String, u32)>, String>
MacroComponent::expand(&mut self, name: &str, args: &Tree) -> Result<Tree, String>
```

The interface instance name is the const `MACROS_INTERFACE =
"wavelet:meta/macros@0.1.0"` — note the **`@0.1.0` version suffix is required**
in the export path; a bare `wavelet:meta/macros` won't resolve.

### Exactly how `node` ↔ `Val::Variant`

Each node lowers to `Val::Variant(case_name, Some(Box::new(payload)))` with the
**kebab-case WIT case name** (not the Rust PascalCase) and this payload shape:

| `meta::Node`   | case        | payload `Val`                                       |
|----------------|-------------|-----------------------------------------------------|
| `BoolVal(b)`   | `bool-val`  | `Val::Bool(b)`                                      |
| `IntVal(n)`    | `int-val`   | `Val::S64(n)`            (WIT `s64`)                 |
| `DecVal(d)`    | `dec-val`   | `Val::Float64(d)`        (WIT `f64`)                 |
| `CharVal(c)`   | `char-val`  | `Val::Char(c)`                                      |
| `StrVal(s)`    | `str-val`   | `Val::String(s)`                                    |
| `Sym(s)`       | `sym`       | `Val::String(s)`                                    |
| `Qsym(a,n)`    | `qsym`      | `Val::Tuple([String(a), String(n)])`                |
| `Tup(ids)`     | `tup`       | `Val::List([U32(id), …])`                           |
| `Lst(ids)`     | `lst`       | `Val::List([U32(id), …])`                           |
| `Rec(fields)`  | `rec`       | `Val::List([Tuple([String(k), U32(v)]), …])`        |
| `Flg(names)`   | `flg`       | `Val::List([String(n), …])`                         |

`tree` → `Val::Record([("nodes", list<node>), ("root", U32), ("spans",
list<Tuple([U32,U32])>)])`. `result<tree,string>` lifts from `Val::Result`:
`Ok(Some(tree_val))`→`Ok(Tree)`, `Err(Some(String))`→`Err(msg)`; missing payloads
become marshalling errors. Every variant is covered by
`every_node_variant_roundtrips_through_val` (incl. nullary `tup`/empty `flg`).

### Fixture

- `tests/fixtures/macros.wasm` — checked-in component exporting
  `wavelet:meta/macros@0.1.0`. Source crate at `tests/fixtures/macros/`
  (`src/lib.rs` + wit-bindgen-generated `src/macro_lib.rs`, WIT in `wit/` with a
  vendored copy of `wit/meta/code.wit` under `wit/deps/wavelet-meta/`).
- Exports three macros: `identity`/1 (returns its arg unchanged),
  `unless`/2 (`unless(c body)` → `(if-MACRO c {} body)`), `boom`/0 (always
  `result::err`). It treats the `args` tree as the **whole call form** — a `tup`
  whose element 0 is the head and `1..` are the args.
- **Build (not run by `cargo test`):** `wit-bindgen rust wit --world macro-lib
  --generate-all --out-dir src`, then `cargo build --release --target
  wasm32-unknown-unknown`, then `wasm-tools component new
  target/.../wavelet_macros_fixture.wasm -o ../macros.wasm`. Full command +
  rationale in `tests/fixtures/macros/README.md`. `cargo-component` was avoided
  (it wouldn't merge the vendored dep package); plain wit-bindgen + wasm-tools is
  robust.

### Gotchas for Step 7 (wiring `expand` into the expander)

1. **`args` shape is a convention, not enforced.** This step ships the *whole
   call form* as `args` and the fixture indexes `args.root` (a `tup`) from
   element 1. Step 7 must decide and document the contract — either keep
   "args = the call tup, head at [0]" or build an args-only tup. Whatever you
   pick, it must match what real macro producers (Step 9) emit.
2. **Build for `wasm32-unknown-unknown`, never `wasm32-wasip1`.** The host uses
   an empty, capability-free linker (Step 2); a WASI-importing component fails to
   instantiate. The type-only `import wavelet:meta/code@0.1.0` is fine (no funcs).
3. **Interface export path needs the `@0.1.0` version.** Use
   `instance_func`/`call_instance` with `"wavelet:meta/macros@0.1.0"`, not the
   top-level `func`/`call`.
4. **`MacroComponent` is `&mut self`** (the wasmtime `Store` is mutable);
   `manifest`/`expand` take `&mut self`. Each instantiation is single-threaded;
   reuse one `MacroComponent` across many `expand` calls (post_return cleanup is
   internal, like the `add` fixture in Step 2).
5. The `tree` lifter ignores unexpected record fields and rejects unknown variant
   cases / wrong payload shapes with actionable strings rather than panicking —
   a misbehaving guest surfaces a readable expansion error.
