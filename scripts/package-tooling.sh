#!/usr/bin/env bash
# Package the editor tooling under tooling/ into one release zip per editor, and
# collect the standalone language-server binaries — all into dist/.
#
# Inputs: the per-platform wavelet-lsp binaries built by scripts/build-lsp.sh.
#   LSP_BIN_DIR  Directory holding wavelet-lsp-* binaries. Defaults to dist/.
#                (CI downloads the build-lsp job's artifacts into a separate dir
#                and points this at it.)
#
# Outputs in dist/:
#   wavelet-vscode.zip  VS Code extension (unzips to a wavelet/ dir for the
#                       extensions folder), with its runtime dependency installed
#                       and the per-platform servers under server/.
#   wavelet-lsp-*       The standalone language-server binaries.
#
# The Neovim plugin lives in its own repo (logaan/wavelet.nvim, vendored here as
# the tooling/neovim submodule) and is installed straight from there by
# lazy.nvim, so it needs no release artifact; it runs wavelet-lsp from the
# user's PATH (the standalone binaries collected below).
#
# Asset names are stable and unversioned so README install instructions can
# always point at .../releases/latest/download/<asset>.
#
# Used by the `Release` GitHub workflow.
#
# Run from anywhere. Requires `node`/`npm` and `zip`.
set -euo pipefail
cd "$(dirname "$0")/.."

lsp_bin_dir="${LSP_BIN_DIR:-dist}"

mkdir -p dist

# Artifact upload/download can drop the exec bit; restore it so the binaries we
# bundle into the zips stay runnable after unzip.
chmod +x "$lsp_bin_dir"/wavelet-lsp-* 2>/dev/null || true

# VS Code extension -> unzips to a `wavelet/` dir you drop into the extensions
# folder. Install the runtime dependency (the language client) and bundle the
# per-platform servers under server/ so the extension launches the matching one
# with no extra download.
npm --prefix tooling/vscode install --omit=dev --no-audit --no-fund
rm -rf stage && mkdir -p stage/wavelet
cp -R tooling/vscode/. stage/wavelet/
mkdir -p stage/wavelet/server
cp "$lsp_bin_dir"/wavelet-lsp-* stage/wavelet/server/
(cd stage && zip -r ../dist/wavelet-vscode.zip wavelet)

# Standalone language-server binaries, published alongside the editor zips.
# Skip when the inputs already live in dist/ (local default) — copying a file
# onto itself errors under `set -e`.
if [ "$(cd "$lsp_bin_dir" && pwd)" != "$(cd dist && pwd)" ]; then
  cp "$lsp_bin_dir"/wavelet-lsp-* dist/
fi

rm -rf stage
echo "Packaged editor tooling and collected LSP binaries into dist/"
