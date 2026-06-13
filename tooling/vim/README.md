# Wavelet for Vim / Neovim

Syntax highlighting and filetype detection for [Wavelet](../../README.md) source
files (`.wvl`), plus — on **Neovim** — automatic language-server support
(diagnostics, completion, hover, document symbols) via the bundled
[`wavelet-lsp`](../wavelet-lsp/) binary.

The grammar mirrors the language's lexer (`src/lexer.rs`) and the shared Prism
grammar used by the docs (`docs/src/prism/wavelet.js`). It highlights:

- `//` line comments and `///` doc comments
- `"..."` strings and `'.'` chars, with `\n` / `\u{...}` escapes
- `int` / `float` / `inf` / `nan` numbers
- `true` / `false` booleans and `some` / `none` / `ok` / `err` constructors
- TitleCase macro heads (`If`, `Def`, `Fn`, `Package`, ...)
- call heads (a name attached, with no space, to `(`, `[`, or `{`)
- `alias/name` qualified references and `name:` record keys

## Layout

```
vim/
  ftdetect/wavelet.vim       maps *.wvl to the `wavelet` filetype
  syntax/wavelet.vim         the highlighting rules
  plugin/wavelet_lsp.lua     Neovim: start wavelet-lsp for *.wvl buffers
  bin/                       (release only) bundled wavelet-lsp binaries
```

This is a standard Vim runtime-path package, so any plugin manager that adds a
directory to `runtimepath` will pick it up.

## Language server (Neovim)

The release `wavelet-vim.zip` bundles the `wavelet-lsp` server binaries under
`bin/`, and `plugin/wavelet_lsp.lua` starts the one matching your platform for
every `.wvl` buffer — no extra setup. Classic Vim has no built-in LSP client, so
there it is a no-op and you still get highlighting.

The server is located in this order: `g:wavelet_lsp_path` (if set) → the bundled
`bin/wavelet-lsp-<platform>` → `wavelet-lsp` on your `PATH`. When installing from
source (no `bin/`), build the server and either put it on your `PATH` or set:

```vim
let g:wavelet_lsp_path = '/path/to/wavelet-lsp'
```

```console
$ cargo build --release --manifest-path tooling/wavelet-lsp/Cargo.toml
```

## Install

### From a release (recommended)

Download `wavelet-vim.zip` from the
[releases page](https://github.com/logaan/wavelet/releases/latest) and unzip it
as a package on your `runtimepath`:

```console
$ curl -L -o wavelet-vim.zip \
    https://github.com/logaan/wavelet/releases/latest/download/wavelet-vim.zip
$ mkdir -p ~/.vim/pack/wavelet/start            # Neovim: ~/.config/nvim/pack/wavelet/start
$ unzip wavelet-vim.zip -d ~/.vim/pack/wavelet/start/
```

The zip unpacks to a `wavelet/` directory (`ftdetect/` + `syntax/`), so this
leaves you with `~/.vim/pack/wavelet/start/wavelet/`. Open any `.wvl` file and it
is highlighted.

### From source — manual

Copy the files into your runtime directory, preserving the subpaths (the
`plugin/` file is Neovim-only and optional):

```console
$ mkdir -p ~/.config/nvim/ftdetect ~/.config/nvim/syntax ~/.config/nvim/plugin
$ cp tooling/vim/ftdetect/wavelet.vim    ~/.config/nvim/ftdetect/
$ cp tooling/vim/syntax/wavelet.vim      ~/.config/nvim/syntax/
$ cp tooling/vim/plugin/wavelet_lsp.lua  ~/.config/nvim/plugin/   # Neovim LSP
```

### As a package (Vim 8+ / Neovim)

```console
$ mkdir -p ~/.vim/pack/wavelet/start            # Neovim: ~/.config/nvim/pack/...
$ ln -s "$PWD/tooling/vim" ~/.vim/pack/wavelet/start/wavelet
```

### With a plugin manager

Point the manager at this subdirectory of the repo. For example, with
[lazy.nvim](https://github.com/folke/lazy.nvim):

```lua
{ "logaan/wavelet", config = function() end }   -- then set the plugin's subdir to tooling/vim
```

or [vim-plug](https://github.com/junegunn/vim-plug):

```vim
Plug 'logaan/wavelet', { 'rtp': 'tooling/vim' }
```

## Customising colours

The syntax groups link to the standard Vim highlight groups (`Comment`,
`String`, `Function`, `Keyword`, ...), so your colorscheme drives the colours.
To override a specific token, add e.g. `highlight link waveletMacro Special` to
your config after the colorscheme loads.
