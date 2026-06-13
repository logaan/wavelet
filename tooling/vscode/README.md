# Wavelet for VS Code

Syntax highlighting for [Wavelet](../../README.md) source files (`.wvl`).

The grammar in `syntaxes/wavelet.tmLanguage.json` mirrors the language's lexer
(`src/lexer.rs`) and the shared Prism grammar used by the docs
(`docs/src/prism/wavelet.js`). It highlights:

- `//` line comments and `///` doc comments
- `"..."` strings and `'.'` chars, with `\n` / `\u{...}` escapes
- `int` / `float` / `inf` / `nan` numbers
- `true` / `false` booleans and `some` / `none` / `ok` / `err` constructors
- TitleCase macro heads (`If`, `Def`, `Fn`, `Package`, ...)
- call heads (a name attached, with no space, to `(`, `[`, or `{`)
- `alias/name` qualified references and `name:` record keys

## Install

### From a release (recommended)

Download `wavelet-vscode.zip` from the
[releases page](https://github.com/logaan/wavelet/releases/latest), unzip it into
your extensions folder, and reload the window:

```console
$ curl -L -o wavelet-vscode.zip \
    https://github.com/logaan/wavelet/releases/latest/download/wavelet-vscode.zip
$ unzip wavelet-vscode.zip -d ~/.vscode/extensions/
```

The zip unpacks to a `wavelet/` directory, leaving you with
`~/.vscode/extensions/wavelet/`. (Use `~/.vscode-insiders/extensions` for
Insiders, or `~/.vscode-server/extensions` for remote/SSH.) Open any `.wvl` file
and it is detected as Wavelet.

### From source (development)

1. Copy or symlink this directory into your VS Code extensions folder:

   ```console
   $ ln -s "$PWD/tooling/vscode" ~/.vscode/extensions/wavelet
   ```

   (Use `~/.vscode-insiders/extensions` for Insiders, or
   `~/.vscode-server/extensions` for remote/SSH.)

2. Reload VS Code. Open any `.wvl` file and it will be detected as Wavelet.

### As a packaged `.vsix`

With [`vsce`](https://github.com/microsoft/vscode-vsce) installed:

```console
$ cd tooling/vscode
$ vsce package
$ code --install-extension wavelet-0.1.0.vsix
```

## Customising colours

The grammar uses standard TextMate scopes, so your active color theme drives the
colours automatically. To tweak a specific token, add a
`editor.tokenColorCustomizations` entry in your settings targeting scopes such as
`keyword.control.macro.wavelet` or `entity.name.function.wavelet`.
