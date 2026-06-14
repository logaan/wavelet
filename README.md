# Wavelet

A small homoiconic language for the **WebAssembly Component Model**.

Wavelet rests on three commitments:

1. **One file is one component.** The unit you edit, compile, link, version, and
   deploy is the same thing. Nothing distinguishes a component written in
   Wavelet from one written in Rust, Go, or JavaScript — composition happens at
   the WIT level.
2. **The syntax is [WAVE](https://github.com/bytecodealliance/wasm-tools/tree/main/crates/wasm-wave).**
   Source code is WAVE text (the Component Model's human-readable value encoding)
   plus a thin layer of reader sugar. Wavelet is homoiconic the way Lisp is over
   s-expressions — except its "s-expressions" are exactly the values that cross
   component boundaries.
3. **The core is minimal.** Seventeen special forms, closures, guaranteed
   tail-call elimination, and a macro system. Everything else — including the
   standard library and macros — is delivered as components.

The consequence that ties these together: **there is no FFI.** Wavelet has no
native data types of its own. Its booleans, strings, lists, records, variants,
options, results, and flags *are* WIT types, so calling a Rust component looks
identical to calling a function defined two lines up.

See [`design.md`](design.md) for the full language design (draft 0.1).

## A taste

```rust
// shout.wvl — compiles to demo:shout.wasm
Package "demo:shout@0.1.0"

Export shout
Def shout Fn {phrase: string}
  str-cat[upper(phrase) "!"]
```

```rust
// main.wvl — compiles to demo:main.wasm
Package "demo:main@0.1.0"
Target "wasi:cli/command"

Import {pkg: "demo:shout/api" as: sh}

Export run
Def run Fn {}
  If eq[len(args[]) 0]
     println("usage: main <word>")
     println(sh/shout{phrase: head(args[])})
```

```bash
$ wavelet build examples/shout.wvl examples/main.wvl
$ wavelet compose out/demo-main.wasm out/demo-shout.wasm -o app.wasm
$ wasmtime app.wasm wasm
WASM!
```

Each file declares its own package, becomes its own component, and the composer
wires `main`'s import of `demo:shout/api` to `shout`'s export. Swapping in a Rust
implementation of `demo:shout/api` would require changing nothing in `main.wvl`.

## Installing

### Homebrew

The `wavelet` CLI and the `wavelet-lsp` language server are available from a
personal [tap](https://github.com/logaan/homebrew-tap):

```bash
brew install logaan/tap/wavelet
```

This installs both `wavelet` and `wavelet-lsp` onto your `PATH` as prebuilt
binaries — no Rust toolchain is fetched. Track the bleeding edge from `main`
(built from source) with `brew install --HEAD logaan/tap/wavelet`.

### From source

Clone the repo and run `scripts/install.sh`, which builds both binaries and
symlinks them into `~/bin` (override with `BIN_DIR`). See [Building](#building)
to compile by hand.

## Building

Wavelet is written in Rust (2024 edition).

```bash
cargo build           # debug binary at ./target/debug/wavelet
cargo build --release # optimized binary at ./target/release/wavelet
cargo test            # run the test suite
```

### Test coverage

`scripts/coverage.sh` measures native test coverage with
[`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) (LLVM
source-based coverage). It bootstraps the tool on first run.

```bash
scripts/coverage.sh          # per-file summary table in the terminal
scripts/coverage.sh --html   # write + open an HTML report (target/coverage/html)
scripts/coverage.sh --lcov   # write target/coverage/lcov.info (CI / editor gutters)
```

## The `wavelet` CLI

```
wavelet read <file.wvl>                              # parse and print the canonical WAVE form tree
wavelet expand <file.wvl>                            # run macros to fixpoint and print the result
wavelet wit <file.wvl>                               # show the synthesized WIT world
wavelet repl                                         # interactive read-eval-print loop
wavelet run <file.wvl>... [-- <args>...]             # interpret directly (no codegen)
wavelet build <file.wvl>... [-o <dir>]               # compile each file to a .wasm component (default: out/)
wavelet compose <entry.wasm> <plug.wasm>... [-o <app.wasm>]  # link components (auto-plug)
wavelet --version                                    # print the wavelet version
```

`run` interprets a set of files together — resolving `Import`s by package id,
honoring `Export`/`as:`/`open:`, and calling the exported `run`. It is the
fastest way to try a program:

```bash
$ wavelet run examples/main.wvl examples/shout.wvl -- wasm
WASM!
```

`build` emits a real wasm component per file (core wasm wrapped with
canonical-ABI lift/lower and componentized via `wasm-tools`); `compose` links
them with `wac`-style auto-plugging.

## Editor support

`.wvl` files get syntax highlighting plus a language server, `wavelet-lsp`,
adding diagnostics, completion, hover, and document symbols. The highlighting
grammars are derived from the lexer, so they match the compiler. `wavelet-lsp`
ships as a standalone binary per platform on the
[releases page](https://github.com/logaan/wavelet/releases/latest) and is used
automatically by the VS Code extension and the Neovim plugin.

### Neovim (LazyVim / lazy.nvim)

The Neovim plugin lives in its own repo,
[`logaan/wavelet.nvim`](https://github.com/logaan/wavelet.nvim) (vendored here as
the [`tooling/neovim`](tooling/neovim) submodule). Add it to LazyVim by dropping a
spec in `~/.config/nvim/lua/plugins/wavelet.lua`:

```lua
return {
  {
    "logaan/wavelet.nvim",
    ft = "wavelet",
    init = function()
      vim.filetype.add({ extension = { wvl = "wavelet" } })
    end,
  },
}
```

Open any `.wvl` file and it is highlighted. For language features, put the
`wavelet-lsp` server on your `PATH` — the plugin starts it automatically:

```bash
cargo install --path tooling/wavelet-lsp     # installs into ~/.cargo/bin
```

or download a prebuilt `wavelet-lsp-<platform>` binary from the releases page.
To point at a specific binary instead, set `vim.g.wavelet_lsp_path`. See
[`tooling/wavelet-lsp/`](tooling/wavelet-lsp/) for other editors.

### VS Code

Download `wavelet-vscode.zip`, unzip it into your extensions folder, and reload
the window:

```bash
$ curl -L -o wavelet-vscode.zip \
    https://github.com/logaan/wavelet/releases/latest/download/wavelet-vscode.zip
$ unzip wavelet-vscode.zip -d ~/.vscode/extensions/
```

The extension is self-contained: it bundles the `wavelet-lsp` language server, so
you also get diagnostics, completion, hover, and document symbols with no extra
download. (Override the server with the `wavelet.lsp.serverPath` setting, or
disable it with `wavelet.lsp.enable`.) See [`tooling/vscode/`](tooling/vscode/)
for details.

## Pipeline

The compiler is **read → expand → analyze → emit → componentize**:

| Stage | Source | Role |
|---|---|---|
| read | `lexer.rs`, `reader.rs`, `form.rs`, `printer.rs` | WAVE tokens → form-tree arena; all sugar dies here |
| expand | `expand.rs` | macros run to fixpoint over form trees |
| interpret | `interp.rs`, `value.rs`, `builtins.rs`, `runner.rs` | dynamic evaluator over the WIT value space (semantics oracle) |
| WIT synthesis | `wit.rs` | derive the component's WIT world from its forms |
| emit | `emit.rs`, `build.rs` | core wasm (linear-memory value boxes, `return_call` tail calls) → component |

The interpreter exists so language semantics can be validated independently of
codegen; the wasm backend is checked against it.

## Status

Draft 0.1, actively implemented. Working today:

- Full reader with the §2 sugar (attachment rule, arity-driven TitleCase macros,
  qualified calls, quasiquote).
- Macro expander (`DefMacro`, `Quote`/`Quasi`/`Unquote`/`Splice`, `gensym`).
- Tree-walking interpreter with tail-call elimination, pattern matching, and a
  standard-library of builtins.
- WIT world synthesis from typed `Fn` params, `Export` records, and `DefType`.
- A wasm backend covering scalars, lists, records, variants, tuples, closures,
  and option/result — including these types passed **across component
  boundaries** via the canonical ABI.
- End-to-end `build` + `compose` producing components that run on wasmtime.

Not yet done (see [`todo.md`](todo.md)): macro components (compile-time wasm
instantiation), resource handles beyond `cell`, boundary coercions / the
`safely` wrapper, richer type inference, and `compose --fuse`.

## Repository layout

```
src/        compiler and CLI
examples/   shout.wvl + main.wvl (the §1 demo)
tooling/    editor support: VS Code, wavelet-lsp, and the neovim/ submodule
out/        build artifacts (.wasm / .wat / .wit)
design.md   the language design, draft 0.1
todo.md     implementation tracking
```

## License

MIT (intended).
