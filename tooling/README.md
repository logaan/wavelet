# Wavelet editor tooling

Editor integrations for the [Wavelet](../README.md) language.

| Editor          | Where                          | Provides                                  |
| --------------- | ------------------------------ | ----------------------------------------- |
| Neovim          | [`neovim/`](neovim/) submodule (repo [`logaan/wavelet.nvim`](https://github.com/logaan/wavelet.nvim)) | a runtime-path package: syntax highlighting + `.wvl` filetype + `wavelet-lsp` autostart |
| VS Code         | [`vscode/`](vscode/)           | a language extension: syntax highlighting + bundled language server |
| Any LSP client  | [`wavelet-lsp/`](wavelet-lsp/) | the `wavelet-lsp` language server (diagnostics, completion, hover, symbols) |

## Installing

- **Neovim** — the plugin is its own repo,
  [`logaan/wavelet.nvim`](https://github.com/logaan/wavelet.nvim), vendored here
  as the [`neovim/`](neovim/) submodule (run `./scripts/init-submodules.sh` after
  cloning to populate it). Install with LazyVim by pointing a plugin spec at
  `logaan/wavelet.nvim`; see [the README](../README.md#neovim-lazyvim--lazynvim).
  It expects the `wavelet-lsp` binary on your `PATH`
  (`cargo install --path wavelet-lsp`, or a prebuilt binary from the releases
  page).
- **VS Code** — `wavelet-vscode.zip` from the
  [releases page](https://github.com/logaan/wavelet/releases/latest)
  (self-contained: highlighting + the bundled `wavelet-lsp` server, no extra
  download), or build from [`vscode/`](vscode/).
- **Language server** — `wavelet-lsp-<platform>` (e.g.
  `wavelet-lsp-aarch64-apple-darwin`), a standalone binary published per platform
  on the releases page, for Neovim or any other LSP-capable editor. It is
  compiled from [`wavelet-lsp/`](wavelet-lsp/) by
  [`.github/workflows/release.yml`](../.github/workflows/release.yml) and bundled
  into the VS Code zip.

All three grammars (the Neovim `neovim/syntax/wavelet.vim`, the VS Code TextMate
grammar, and the docs' Prism grammar in `docs/src/prism/wavelet.js`) are derived
from the same source of truth — the lexer in `src/lexer.rs` — and recognise the
same token classes:

- `//` line comments
- `"..."` strings and `'.'` chars, with `\n` / `\u{...}` escapes
- `int` / `float` / `inf` / `nan` numbers
- `true` / `false` booleans and `some` / `none` / `ok` / `err` constructors
- TitleCase macro heads (`If`, `Def`, `Fn`, `Package`, ...)
- call heads (a name attached, with no space, to `(`, `[`, or `{`)
- `alias/name` qualified references and `name:` record keys

If you change the lexer, update all three grammars so highlighting cannot drift.
The Neovim grammar is in the `neovim/` submodule, so editing it means committing
and pushing in [`wavelet.nvim`](https://github.com/logaan/wavelet.nvim), then
bumping the submodule pointer here (`git add tooling/neovim`).
