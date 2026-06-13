# Wavelet editor tooling

Editor integrations for the [Wavelet](../README.md) language. Each subdirectory
is self-contained and has its own install instructions.

| Editor          | Directory            | Provides                                  |
| --------------- | -------------------- | ----------------------------------------- |
| Vim / Neovim    | [`vim/`](vim/)       | syntax highlighting + `.wvl` filetype      |
| VS Code         | [`vscode/`](vscode/) | a language extension with syntax highlighting |

## Installing

Most users should grab the prebuilt package for their editor from the
[releases page](https://github.com/logaan/wavelet/releases/latest) rather than
this directory:

- **Vim / Neovim** — `wavelet-vim.zip`
- **VS Code** — `wavelet-vscode.zip`

Each zip unpacks to a single `wavelet/` directory; the per-editor READMEs below
(and inside each zip) give the exact unzip-and-go commands. These are the same
files you see here — `tooling/` is the source the release artifacts are built
from (see [`.github/workflows/release.yml`](../.github/workflows/release.yml)),
so build-from-source and the release download are interchangeable.

All three grammars (these two plus the docs' Prism grammar in
`docs/src/prism/wavelet.js`) are derived from the same source of truth — the
lexer in `src/lexer.rs` — and recognise the same token classes:

- `//` line comments and `///` doc comments
- `"..."` strings and `'.'` chars, with `\n` / `\u{...}` escapes
- `int` / `float` / `inf` / `nan` numbers
- `true` / `false` booleans and `some` / `none` / `ok` / `err` constructors
- TitleCase macro heads (`If`, `Def`, `Fn`, `Package`, ...)
- call heads (a name attached, with no space, to `(`, `[`, or `{`)
- `alias/name` qualified references and `name:` record keys

If you change the lexer, update all three grammars so highlighting cannot drift.
