<p align="center">
  <img src="brand/Logo.png" alt="Wavelet logo" width="180" height="180">
</p>

# Wavelet

A small, statically typed language that compiles to **WebAssembly components**.

Wavelet uses Component Model types for data and WIT interfaces for composition.
Each source file becomes a component. WAVE-based syntax makes code available as
data, and macros transform that data during compilation.

Read the [documentation](https://logaan.github.io/wavelet/), take the
[language tour](https://logaan.github.io/wavelet/learn-wavelet), or try the
[browser REPL](https://logaan.github.io/wavelet/repl).

## Install

Install the CLI and language server from the Homebrew tap:

```sh
brew install logaan/tap/wavelet
```

Prebuilt binaries and the VS Code extension are available on the
[releases page](https://github.com/logaan/wavelet/releases/latest).
To build the CLI from a source checkout, run from the repository root:

```sh
cargo build --release
cargo test
export PATH="$PWD/target/release:$PATH"
```

The `PATH` setting makes the built CLI available as `wavelet` in this shell.

## Try the language

```console
$ wavelet repl
> map(Fn {n} mul(n n) [1 2 3 4])
[1, 4, 9, 16]
```

The CLI and browser REPL compile and execute components. Compilation errors and
runtime traps are reported directly. Runtime semantics follow WebAssembly instructions and Component Model contracts.
The macro evaluator runs only during compilation.

## Compose two files

Save this as `shout.wlt`:

```wavelet
Package "demo:shout@0.1.0"

Export shout
Def shout Fn {phrase: string}
  str-cat(phrase "!")
```

Save the caller as `main.wlt`:

```wavelet
Package "demo:main@0.1.0"

Import {pkg: "demo:shout/api" as: sh}

Export {name: run result: string}
Def run Fn {} sh/shout("components")
```

Run the entry component, or retain the compiled artifacts:

```sh
wavelet run main.wlt shout.wlt
wavelet build main.wlt shout.wlt -o out
```

`run` calls the exported function and discards its return value, so this example
succeeds without printing anything. With [Wasmtime](https://docs.wasmtime.dev/cli-install.html)
installed, inspect the compiled component's return value:

```console
$ wasmtime run --invoke 'run()' out/app.wasm
"components!"
```

`build` emits the individual components and a composed `out/app.wasm` for this
project. Imports not satisfied by sibling components remain requirements for a
host or a later composition step. The embedded runner supplies no host interfaces.

## Language features

- Static checking with inference, annotations, homogeneous lists, records,
  variants, options, results, and exhaustive pattern matching.
- Local functions, statically resolved capturing callbacks, and tail recursion
  within a component.
- Cells and user-defined or imported resources with ownership and borrowing.
- Local and component macros and compile-time templates
  that generate concrete types.
- Synchronous WIT imports and exports, component composition, and an embeddable
  compiler available as a Rust API and a WebAssembly component.

See [supported features and limits](https://logaan.github.io/wavelet/roadmap)
for restrictions on runtime function values, resource state, and WIT features.

## Create an application

Install [`wkg`](https://github.com/bytecodealliance/wasm-pkg-tools) and the
[Wasmtime CLI](https://docs.wasmtime.dev/cli-install.html) on `PATH` first.
`wavelet new` uses `wkg` to fetch the application's WIT dependencies; the generated
scripts use `wasmtime` to run the built component with WASI interfaces.

```sh
wavelet new hello
cd hello
scripts/run.sh Ada
```

The CLI template prints `Hello, Ada!` after building its components.
`wavelet new my-site --type=http` creates an HTTP application with a
`scripts/serve.sh` launcher. These are examples of ordinary WIT interfaces;
Wavelet programs can use other compatible host contracts too.

## CLI

| Command | Purpose |
|---|---|
| `wavelet new <name> [--type=cli\|http]` | Scaffold an application. |
| `wavelet read [file.wlt]` | Print the parsed form tree; read stdin if no file is given. |
| `wavelet expand <file.wlt>` | Print macro-expanded forms. |
| `wavelet wit <file.wlt>` | Inspect the synthesized WIT world. |
| `wavelet repl` | Compile and run expressions interactively. |
| `wavelet run <file.wlt>...` | Compose files and call the first file's `api.run`. |
| `wavelet build <file.wlt>... [--optimize-size] [-o <dir>]` | Write component artifacts. |
| `wavelet compose <entry.wasm> <plug.wasm>... [--strip-metadata] [-o <file>]` | Connect prebuilt components. |

An executable source file can start with `#!/usr/bin/env wavelet` and provide an
exported `run` function. See the [CLI reference](https://logaan.github.io/wavelet/cli)
for details.

## Editor support

The `wavelet-lsp` server provides diagnostics, completion, hover, and document
symbols. The VS Code extension bundles it. The
[Neovim plugin](https://github.com/logaan/wavelet.nvim) starts it from `PATH`.
Installation details for both are in the
[getting started guide](https://logaan.github.io/wavelet/getting-started#editor-support)
and [tooling documentation](tooling/README.md).

## Development

See [scripts/README.md](scripts/README.md) for build, validation, and release
workflows, and [the compiler API guide](docs/docs/compiler-api.md) for embedding.

## License

[Apache-2.0](LICENSE)
