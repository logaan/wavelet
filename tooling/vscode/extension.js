// Entry point for the Wavelet VS Code extension.
//
// The TextMate grammar (declared in package.json) gives syntax highlighting on
// its own. This file additionally wires up the Wavelet language server
// (`wavelet-lsp`) for diagnostics, completion, hover, and document symbols.
//
// We locate the server, in order:
//   1. the `wavelet.lsp.serverPath` setting, if set;
//   2. a binary bundled in this extension's `server/` directory matching the
//      current platform (release builds ship one per platform here);
//   3. `wavelet-lsp` on the PATH.
// If none can be started, we degrade gracefully to highlighting-only and surface
// a single, dismissable warning.

const fs = require("fs");
const path = require("path");
const { workspace, window } = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

let client;

// Map the running platform to the Rust target triple used for asset names.
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
    // Release zips should carry the exec bit, but make sure (best effort).
    try {
      fs.chmodSync(candidate, 0o755);
    } catch (_) {
      /* ignore */
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
