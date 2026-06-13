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

### From a release (recommended)

The release `wavelet-vscode.zip` is **self-contained** — it bundles the language
client *and* the `wavelet-lsp` server binaries for every platform, so there is
nothing else to download. Unzip it into your extensions folder and reload:

```console
$ curl -L -o wavelet-vscode.zip \
    https://github.com/logaan/wavelet/releases/latest/download/wavelet-vscode.zip
$ unzip wavelet-vscode.zip -d ~/.vscode/extensions/
```

The zip unpacks to a `wavelet/` directory, leaving you with
`~/.vscode/extensions/wavelet/`. (Use `~/.vscode-insiders/extensions` for
Insiders, or `~/.vscode-server/extensions` for remote/SSH.) Open any `.wvl` file:
it is detected as Wavelet, highlighted, and the language server starts
automatically — the extension picks the bundled binary matching your platform
(from `server/`).

> Prefer your own build? Set `wavelet.lsp.serverPath` to a `wavelet-lsp` binary,
> or put one on your `PATH`; it takes precedence over the bundled copy. Standalone
> `wavelet-lsp-<platform>` binaries are also published on the releases page for
> use outside VS Code.

### From source (development)

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

### As a packaged `.vsix`

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
| `wavelet.lsp.serverPath` | `""` | Path to the `wavelet-lsp` executable. Empty means use the bundled binary if present, else `wavelet-lsp` on the PATH. |

If the server can't be started, the extension shows one warning and falls back to
highlighting only — set `wavelet.lsp.serverPath`, or install the binary, to fix it.

## Customising colours

The grammar uses standard TextMate scopes, so your active color theme drives the
colours automatically. To tweak a specific token, add a
`editor.tokenColorCustomizations` entry in your settings targeting scopes such as
`keyword.control.macro.wavelet` or `entity.name.function.wavelet`.
