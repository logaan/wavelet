<p align="center">
  <img src="brand/Logo.png" alt="Wavelet logo" width="180" height="180">
</p>

# Wavelet

A small homoiconic language that compiles to **WebAssembly Components**.

## Installation

The `wavelet` CLI and the `wavelet-lsp` language server are available from a
personal Homebrew [tap][]:

``` bash
brew install logaan/tap/wavelet
```

<!-- TODO Link to the Editor support section below -->

## Example

`shout.wvl` — compiles to `demo:shout.wasm`

``` rust
Package "demo:shout@0.1.0"

Export shout
Def shout Fn {phrase: string}
  str-cat[upper(phrase) "!"]
```

`main.wvl` compiles to `demo:main.wasm`

``` rust
Package "demo:main@0.1.0"

Import {pkg: "demo:shout/api" as: sh}

Export greet
Def greet Fn {phrase: string}
  str-cat("Hello, " sh/shout(phrase))
```

<!-- TODO: Switch to a term that's less jargony than REPL-->

The repl (Read Eval Print Loop) lets us run code interactively.

``` bash
$ wavelet repl examples/main.wvl
$ demo:main/greet("world")
-> "Hello, WORLD!"
```

## Features of the language

Because Wavelet has a small core language that's tightly aligned with WASM
components it runs everywhere, and communicates with web assembly code written
in any language as fluently as one Wavelet function calling another.

### Compositional sandboxing

<!--
  TODO: Explain sandboxing and dependency injection to someone who's
  unfamiliar with them.
-->

- Wavelet programs are entirely self contained by default. They have no access
  to your network or hard drive, not even to clocks or random numbers. This is a
  feature of the WebAssembly Component Model.
- For a wavelet file to access these features it must ask to be given those
  functions. They host may choose to provide any implementation they like. In
  production you'll give full access but in your test suites you may substitute
  mock implementations.
- The same restrictions apply to libraries that your program consumes. You have
  have confidence that your `left-pad` dependency isn't going to read your
  credentials files and publish them online. It doesn't have access to your disk
  or to the internet unless it explicitly asks for it and you explicitly grant
  it.

### Only Wasm Interface Types

<!--
  TODO: Explain foreign function interfaces, impedance mismatch, and the
  boundary problem to someone who's unfamiliar with them.
-->

Wavelet programs can only use data types, and definition constructs that can be
expressed with [Wasm Interface Types]. This is a significant restriction, but it
means that wavelet programs can use, and be used by, external components with no
type conversion at the boundary.

1.  Built in types:
    1.  Primitives: `bool`, `s8`, `s16`, `s32`, `s64`, `u8`, `u16`, `u32`,
        `u64`, `f32`, `f64`, `char`, `string`
    2.  Collections: `list`, `option`, `result`, `tuple`
2.  User defined: `record`, `variant`, `enum`, `resource`, `flags`
3.  Interfaces: `func`, `interface`, `world`, `package`

### Code is data + a little sugar

<!--
  TODO: Explain homoiconicity, macros, and DLSs to someone who's unfamiliar with
  them.
-->

- Wavelet has a rich syntax of literals for every data type
- It uses that same syntax, and those same data types, to express the program's
  code as well as its data.
- This lets us implement new language features in the language itself.
- Features that you might expect are fundamental like binary `And` and `Or` can
  be implemented in a library rather than needing to be baked into the language.
- Wavelet also gives users the freedom to swap out the standard library, truly
  tailoring their environment to the task at hand on a file by file basis.

### Small set of powerful features

- You can pick it up quickly.
- You're not going to be scratching your head when you come back to it after a
  while away.
- It's still an expressive language.
  - Higher order functions let you create powerful abstractions day to day.
  - Functors let you use new types with existing data structures.
  - Macros give you ultimate power remove any boilerplate, and change the
    language.

### What you don't get

And what you can use instead.

- No loops. Use recursion or combinators.
- No exceptions. Instead return errors using results.
- No generics. Instead use functors.
- No polymorphism. Functors, type inference, and a little sugar mean you might
  not even notice.
- No sets, regex, etc. Pull them in from libraries.

## The `wavelet` CLI

<!--
  TODO: Improve the ergonomics with paths
  TODO: Drop references to the interpreter
-->

    wavelet new <name> [--type=cli|http]                 # scaffold a new project (cli is the default)
    wavelet read [file.wvl]                              # parse and print the canonical WAVE form tree (reads stdin if no file)
    wavelet expand <file.wvl>                            # run macros to fixpoint and print the result
    wavelet wit <file.wvl>                               # show the synthesized WIT world
    wavelet repl                                         # interactive read-eval-print loop
    wavelet run <file.wvl>... [-- <args>...]             # interpret directly (no codegen)
    wavelet build <file.wvl>... [-o <dir>]               # compile each file to a .wasm component (default: out/)
    wavelet compose <entry.wasm> <plug.wasm>... [-o <app.wasm>]  # link components (auto-plug)
    wavelet --version                                    # print the wavelet version

`run` interprets a set of files together — resolving `Import`s by package id,
honoring `Export`/`as:`/`open:`, and calling the exported `run`. It is the
fastest way to try a program:

``` bash
$ wavelet run examples/main.wvl examples/shout.wvl -- wasm
WASM!
```

`build` emits a real wasm component per file (core wasm wrapped with
canonical-ABI lift/lower and componentized via `wasm-tools`); `compose` links
them with `wac`-style auto-plugging.

`new` scaffolds a fresh project into a directory of the given name — a
`.gitignore`, a `src/` with two `.wvl` files (an entry point and the domain
model it imports across the component boundary), build/run scripts, and a short
README. `--type` picks the template; `cli` is the default:

``` bash
$ wavelet new my-app          # cli: a wasi:cli/command program
$ cd my-app
$ scripts/run.sh Ada          # build, then run with wasmtime → "Hello, Ada!"
```

`--type=http` instead lays down a web app whose front end implements the
`wasi:http/incoming-handler` interface — a stateless page that greets via the
`greeting` domain component (across the component boundary) and echoes the
request path. `scripts/serve.sh` builds it and runs it with `wasmtime serve`:

``` bash
$ wavelet new my-site --type=http
$ cd my-site
$ scripts/serve.sh           # then open http://localhost:8080
```

## Editor support

`.wvl` files get syntax highlighting plus a language server, `wavelet-lsp`,
adding diagnostics, completion, hover, and document symbols. The highlighting
grammars are derived from the lexer, so they match the compiler. `wavelet-lsp`
ships as a standalone binary per platform on the [releases page] and is used
automatically by the VS Code extension and the Neovim plugin.

### Neovim (LazyVim / lazy.nvim)

The Neovim plugin lives in its own repo, [`logaan/wavelet.nvim`] (vendored here
as the [`tooling/neovim`] submodule). Add it to LazyVim by dropping a spec in
`~/.config/nvim/lua/plugins/wavelet.lua`:

``` lua
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

``` bash
cargo install --path tooling/wavelet-lsp     # installs into ~/.cargo/bin
```

or download a prebuilt `wavelet-lsp-<platform>` binary from the releases page.
To point at a specific binary instead, set `vim.g.wavelet_lsp_path`. See
[`tooling/wavelet-lsp/`] for other editors.

### VS Code

Download `wavelet-vscode.zip`, unzip it into your extensions folder, and reload
the window:

``` bash
$ curl -L -o wavelet-vscode.zip \
    https://github.com/logaan/wavelet/releases/latest/download/wavelet-vscode.zip
$ unzip wavelet-vscode.zip -d ~/.vscode/extensions/
```

The extension is self-contained: it bundles the `wavelet-lsp` language server,
so you also get diagnostics, completion, hover, and document symbols with no
extra download. (Override the server with the `wavelet.lsp.serverPath` setting,
or disable it with `wavelet.lsp.enable`.) See [`tooling/vscode/`] for details.

## License

[Apache-2.0]

  [tap]: https://github.com/logaan/homebrew-tap
  [Wasm Interface Types]: https://component-model.bytecodealliance.org/design/wit.html
  [releases page]: https://github.com/logaan/wavelet/releases/latest
  [`logaan/wavelet.nvim`]: https://github.com/logaan/wavelet.nvim
  [`tooling/neovim`]: tooling/neovim
  [`tooling/wavelet-lsp/`]: tooling/wavelet-lsp/
  [`tooling/vscode/`]: tooling/vscode/
  [Apache-2.0]: https://opensource.org/license/Apache-2.0
