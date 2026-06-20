# `macros.wasm` — fixture macro-library component

A tiny, standalone Rust component that exports `wavelet:meta/macros@0.1.0`. It
is the fixture `src/macros.rs` (Step 3) tests `MacroComponent` against. The
built artifact lives one directory up at `tests/fixtures/macros.wasm` and **is
checked in** so `cargo test` stays hermetic — running the suite needs no wasm
toolchain. This crate is *not* part of the `wavelet` workspace and is never
built by `cargo test`; regenerate the `.wasm` by hand when you change it.

## What it exports

`wavelet:meta/macros` with three macros (see `src/lib.rs`):

| macro      | arity | behaviour                                            |
|------------|-------|------------------------------------------------------|
| `identity` | 1     | returns its single argument form unchanged           |
| `unless`   | 2     | `unless(c body)` → `(if-MACRO c {} body)`            |
| `boom`     | 0     | always returns `result::err` (exercises the error path) |

The `args` tree the host ships is the whole *call* form — a `tup` whose element
0 is the head and elements `1..` are the argument forms — so each macro reads
`args.nodes[args.root]` as a `tup` and indexes from 1.

## How it's built (regeneration)

Built **without** `cargo-component`: plain `wit-bindgen` + `cargo build` for
`wasm32-unknown-unknown` (so the component imports nothing — it must instantiate
under the host's empty, capability-free linker; the `wasm32-wasip1` target would
drag in `wasi_snapshot_preview1` and fail), then `wasm-tools component new`.

The WIT lives in `wit/`: `wit/world.wit` declares the `macro-lib` world, and
`wit/deps/wavelet-meta/code.wit` is a **vendored copy** of the repo's
`wit/meta/code.wit` (keep them in sync if the wire type changes).

```console
cd tests/fixtures/macros

# 1. (Re)generate guest bindings into src/macro_lib.rs
wit-bindgen rust wit --world macro-lib --generate-all --out-dir src

# 2. Build the core module (unknown-unknown ⇒ no WASI imports)
cargo build --release --target wasm32-unknown-unknown

# 3. Componentize into the checked-in fixture
wasm-tools component new \
  target/wasm32-unknown-unknown/release/wavelet_macros_fixture.wasm \
  -o ../macros.wasm

# sanity check
wasm-tools component wit ../macros.wasm   # exports wavelet:meta/macros@0.1.0
```

Tool versions used: `wit-bindgen 0.43.0`, `wasm-tools 1.235.0`, Rust target
`wasm32-unknown-unknown`. `src/macro_lib.rs` is generated; it's committed so the
crate builds without re-running `wit-bindgen`, but step 1 regenerates it.
