# Wavelet for VS Code

Editor support for [Wavelet](../../README.md) source files (`.wlt`):

- **Syntax highlighting** — the grammar in `syntaxes/wavelet.tmLanguage.json`
  mirrors the language's lexer (`src/lexer.rs`) and the shared Prism grammar used
  by the docs (`docs/src/prism/wavelet.js`). It highlights:
  - `#!` leading shebang line and `//` line comments
  - `"..."` strings and `'.'` chars, with `\n` / `\u{...}` escapes
  - `int` / `float` / `inf` / `nan` numbers
  - `true` / `false` booleans and `some` / `none` / `ok` / `err` constructors
  - TitleCase macro heads (`If`, `Def`, `Fn`, `Package`, ...)
  - call heads (a name attached, with no space, to `(`, `[`, or `{`)
  - `alias/name` qualified references and `name:` record keys
- **Language features** — when the [`wavelet-lsp`](../wavelet-lsp/) server is
  available, the extension also provides live diagnostics, completion, hover, and
  document symbols. Highlighting works with or without the server.

## Verifying the tooling

The grammar is checked headlessly against VS Code's own tokenizer
(`vscode-textmate` + `vscode-oniguruma`), so highlighting regressions are caught
without opening the editor:

```console
$ cd tooling/vscode
$ npm install        # dev-only: vscode-textmate, vscode-oniguruma
$ npm run check
```

`scripts/check-grammar.mjs` asserts that representative snippets tokenize to the
expected scopes (comments incl. the shebang, strings/chars, numbers, booleans
and `some`/`none`/`ok`/`err`, macro heads, call heads, qualified references, and
record keys) and that `package.json`, `language-configuration.json`, and the
grammar agree on the language id, `.wlt` extension, and `source.wavelet` scope.
Keep it in step with `src/lexer.rs` when token classes change.

A few things only a real editor exercises; smoke-test them once after a grammar
or config change by opening a `.wlt` file (from source per below, or the
released extension):

1. **Highlighting** — colours match the token classes above; a leading
   `#!/usr/bin/env wavelet` line reads as a comment.
2. **Comments** — `Ctrl-/` toggles a `//` line comment.
3. **Brackets** — matching `()`/`[]`/`{}` highlight, and typing an opener
   auto-closes it; typing `"` or `'` auto-closes and wraps a selection.
4. **Word selection** — double-clicking selects a whole `alias/name` reference
   (the `wordPattern`).
5. **Language server** — with `wavelet-lsp` on the `PATH` (or
   `wavelet.lsp.serverPath` set), diagnostics/hover/completion appear; with the
   server disabled (`wavelet.lsp.enable: false`) highlighting still works and a
   single dismissable warning is shown.

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
Insiders, or `~/.vscode-server/extensions` for remote/SSH.) Open any `.wlt` file:
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

3. Reload VS Code. Open any `.wlt` file.

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
