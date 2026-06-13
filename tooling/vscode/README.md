# Wavelet for VS Code

Editor support for [Wavelet](../../README.md) source files (`.wvl`):

- **Syntax highlighting** — the grammar in `syntaxes/wavelet.tmLanguage.json`
  mirrors the language's lexer (`src/lexer.rs`) and the shared Prism grammar used
  by the docs (`docs/src/prism/wavelet.js`). It highlights:
  - `//` line comments and `///` doc comments
  - `"..."` strings and `'.'` chars, with `\n` / `\u{...}` escapes
  - `int` / `float` / `inf` / `nan` numbers
  - `true` / `false` booleans and `some` / `none` / `ok` / `err` constructors
  - TitleCase macro heads (`If`, `Def`, `Fn`, `Package`, ...)
  - call heads (a name attached, with no space, to `(`, `[`, or `{`)
  - `alias/name` qualified references and `name:` record keys
- **Language features** — when the [`wavelet-lsp`](../wavelet-lsp/) server is
  available, the extension also provides live diagnostics, completion, hover, and
  document symbols. Highlighting works with or without the server.

## Install

The extension and the language server are **two separate downloads**. Install
both for the full experience; the extension alone still highlights.

### 1. Install the language server

Download the `wavelet-lsp` binary for your platform from the
[releases page](https://github.com/logaan/wavelet/releases/latest) and put it on
your `PATH` (or anywhere, then point `wavelet.lsp.serverPath` at it):

```console
# macOS (Apple Silicon) — pick the asset matching your platform:
#   wavelet-lsp-aarch64-apple-darwin     macOS (Apple Silicon)
#   wavelet-lsp-x86_64-apple-darwin      macOS (Intel)
#   wavelet-lsp-x86_64-unknown-linux-gnu Linux (x86_64)
#   wavelet-lsp-x86_64-pc-windows-msvc.exe  Windows (x86_64)
$ curl -L -o wavelet-lsp \
    https://github.com/logaan/wavelet/releases/latest/download/wavelet-lsp-aarch64-apple-darwin
$ chmod +x wavelet-lsp
$ sudo mv wavelet-lsp /usr/local/bin/        # somewhere on your PATH
```

> On macOS, Gatekeeper may quarantine a downloaded binary. If you see "cannot be
> opened", clear the flag: `xattr -d com.apple.quarantine /usr/local/bin/wavelet-lsp`.

### 2. Install the extension

#### From a release (recommended)

Download `wavelet-vscode.zip` from the
[releases page](https://github.com/logaan/wavelet/releases/latest), unzip it into
your extensions folder, and reload the window:

```console
$ curl -L -o wavelet-vscode.zip \
    https://github.com/logaan/wavelet/releases/latest/download/wavelet-vscode.zip
$ unzip wavelet-vscode.zip -d ~/.vscode/extensions/
```

The zip unpacks to a `wavelet/` directory (with its bundled language-client
dependency), leaving you with `~/.vscode/extensions/wavelet/`. (Use
`~/.vscode-insiders/extensions` for Insiders, or `~/.vscode-server/extensions`
for remote/SSH.) Open any `.wvl` file: it is detected as Wavelet, highlighted,
and — if `wavelet-lsp` is on your PATH — the language server starts.

#### From source (development)

1. Install the runtime dependency, then copy or symlink this directory into your
   VS Code extensions folder:

   ```console
   $ cd tooling/vscode
   $ npm install            # fetches vscode-languageclient into node_modules/
   $ ln -s "$PWD" ~/.vscode/extensions/wavelet
   ```

   (Use `~/.vscode-insiders/extensions` for Insiders, or
   `~/.vscode-server/extensions` for remote/SSH.)

2. Build the server and put it on your PATH (or set `wavelet.lsp.serverPath`):

   ```console
   $ cargo build --release --manifest-path ../wavelet-lsp/Cargo.toml
   $ cp ../wavelet-lsp/target/release/wavelet-lsp /usr/local/bin/
   ```

3. Reload VS Code. Open any `.wvl` file.

#### As a packaged `.vsix`

With [`vsce`](https://github.com/microsoft/vscode-vsce) installed (run
`npm install` first so the client dependency is bundled):

```console
$ cd tooling/vscode
$ npm install
$ vsce package
$ code --install-extension wavelet-0.2.0.vsix
```

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `wavelet.lsp.enable` | `true` | Start the language server. Set `false` for highlighting-only. |
| `wavelet.lsp.serverPath` | `""` | Path to the `wavelet-lsp` executable. Empty means look it up as `wavelet-lsp` on the PATH. |

If the server can't be started, the extension shows one warning and falls back to
highlighting only — set `wavelet.lsp.serverPath`, or install the binary, to fix it.

## Customising colours

The grammar uses standard TextMate scopes, so your active color theme drives the
colours automatically. To tweak a specific token, add a
`editor.tokenColorCustomizations` entry in your settings targeting scopes such as
`keyword.control.macro.wavelet` or `entity.name.function.wavelet`.
