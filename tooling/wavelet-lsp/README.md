# wavelet-lsp

A basic [Language Server](https://microsoft.github.io/language-server-protocol/)
for the Wavelet language. It reuses the compiler crate's reader
(`wavelet::read_file`) as the single source of syntax truth, so the editor and
the `wavelet` CLI never disagree about what parses.

## Features

| Capability | What you get |
|---|---|
| **Diagnostics** | Live syntax errors from the reader, on open and on every edit. |
| **Completion** | Special forms (`Def`, `Fn`, `If`, …), the standard builtins (`map`, `str-cat`, …), and names defined in the current file. |
| **Hover** | Blurbs for special forms and builtins, plus a definition's `///` doc comment when you hover the name it defines. |
| **Document symbols** | Top-level `Def`, `DefType`, and `DefMacro` forms (outline / breadcrumbs). |

Analysis stops at the *read* stage — no macro expansion, evaluation, or codegen
— so responses are fast and side-effect free. Because the reader is
all-or-nothing, hover and document symbols are empty while the buffer has a
syntax error; completion still offers special forms and builtins.

## Installing

Prebuilt binaries are published per platform on the
[releases page](https://github.com/logaan/wavelet/releases/latest) as
`wavelet-lsp-<target>` (e.g. `wavelet-lsp-aarch64-apple-darwin`,
`wavelet-lsp-x86_64-unknown-linux-gnu`,
`wavelet-lsp-x86_64-pc-windows-msvc.exe`). Download the one for your platform,
make it executable, and put it on your `PATH`:

```console
$ curl -L -o wavelet-lsp \
    https://github.com/logaan/wavelet/releases/latest/download/wavelet-lsp-aarch64-apple-darwin
$ chmod +x wavelet-lsp && sudo mv wavelet-lsp /usr/local/bin/
```

## Building

This is a standalone crate (its own `[workspace]`), kept separate from the root
`wavelet` package so building it never perturbs the docs/examples wasm pipeline.

```console
$ cd tooling/wavelet-lsp
$ cargo build --release      # binary at target/release/wavelet-lsp
```

The server speaks LSP over **stdio**.

## Editor setup

### Neovim (built-in LSP)

```lua
vim.filetype.add({ extension = { wvl = "wavelet" } })

vim.api.nvim_create_autocmd("FileType", {
  pattern = "wavelet",
  callback = function(args)
    vim.lsp.start({
      name = "wavelet-lsp",
      cmd = { "/abs/path/to/wavelet-lsp" }, -- the built binary
      root_dir = vim.fs.dirname(args.file),
    })
  end,
})
```

### VS Code

Install the [Wavelet extension](../vscode/) (from `wavelet-vscode.zip` on the
releases page). It bundles a language client that launches `wavelet-lsp`
automatically — just make sure the binary is on your `PATH`, or point the
`wavelet.lsp.serverPath` setting at it. See [`../vscode/README.md`](../vscode/README.md)
for details.

### Helix (`languages.toml`)

```toml
[[language]]
name = "wavelet"
scope = "source.wavelet"
file-types = ["wvl"]
language-servers = ["wavelet-lsp"]

[language-server.wavelet-lsp]
command = "/abs/path/to/wavelet-lsp"
```
