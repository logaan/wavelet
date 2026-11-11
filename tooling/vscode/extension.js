const fs = require("fs");
const path = require("path");
const { workspace, window } = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

let client;

function bundledServerPath() {
  const triples = {
    "darwin:arm64": "aarch64-apple-darwin",
    "darwin:x64": "x86_64-apple-darwin",
    "linux:x64": "x86_64-unknown-linux-gnu",
    "win32:x64": "x86_64-pc-windows-msvc",
  };
  const triple = triples[`${process.platform}:${process.arch}`];
  if (!triple) {
    return undefined;
  }
  const exe = process.platform === "win32" ? ".exe" : "";
  const candidate = path.join(__dirname, "server", `wavelet-lsp-${triple}${exe}`);
  if (!fs.existsSync(candidate)) {
    return undefined;
  }
  if (process.platform !== "win32") {

    try {
      fs.chmodSync(candidate, 0o755);
    } catch (_) {

    }
  }
  return candidate;
}

function serverCommand() {
  const configured = workspace
    .getConfiguration("wavelet")
    .get("lsp.serverPath");
  if (configured && configured.trim() !== "") {
    return configured.trim();
  }
  return bundledServerPath() || "wavelet-lsp";
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
        `Syntax highlighting still works. Set "wavelet.lsp.serverPath" to a ` +
        `wavelet-lsp binary, or put one on your PATH. (${err})`
    );
  });
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
