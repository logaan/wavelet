// Entry point for the Wavelet VS Code extension.
//
// The TextMate grammar (declared in package.json) gives syntax highlighting on
// its own. This file additionally wires up the Wavelet language server
// (`wavelet-lsp`) for diagnostics, completion, hover, and document symbols.
//
// The server is a separate binary, distributed as its own release asset. We
// locate it via the `wavelet.lsp.serverPath` setting, falling back to
// `wavelet-lsp` on the PATH. If it can't be started, we degrade gracefully to
// highlighting-only and surface a single, dismissable warning.

const { workspace, window } = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

let client;

function serverCommand() {
  const configured = workspace
    .getConfiguration("wavelet")
    .get("lsp.serverPath");
  return configured && configured.trim() !== "" ? configured.trim() : "wavelet-lsp";
}

function activate() {
  const config = workspace.getConfiguration("wavelet");
  if (config.get("lsp.enable") === false) {
    return;
  }

  const command = serverCommand();
  const serverOptions = {
    run: { command, transport: TransportKind.stdio },
    debug: { command, transport: TransportKind.stdio },
  };
  const clientOptions = {
    documentSelector: [{ scheme: "file", language: "wavelet" }],
  };

  client = new LanguageClient(
    "wavelet-lsp",
    "Wavelet Language Server",
    serverOptions,
    clientOptions
  );

  client.start().catch((err) => {
    window.showWarningMessage(
      `Wavelet: could not start the language server ('${command}'). ` +
        `Syntax highlighting still works. Install wavelet-lsp and set ` +
        `"wavelet.lsp.serverPath", or put it on your PATH. (${err})`
    );
  });
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
