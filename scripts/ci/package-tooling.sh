#!/usr/bin/env bash
# Package the editor tooling under tooling/ into one release zip per editor, and
# collect/repackage the standalone binaries — all into dist/.
#
# Inputs: the per-platform binaries staged by scripts/ci/build-cli.sh and
# scripts/ci/build-lsp.sh.
#   STAGED_BINS  Directory holding wavelet-<target> and wavelet-lsp-<target>
#                binaries. Defaults to dist/. (CI downloads the build job's
#                artifacts into a separate dir and points this at it.)
#
# Outputs in dist/:
#   wavelet-vscode.zip      VS Code extension (unzips to a wavelet/ dir for the
#                           extensions folder), with its runtime dependency
#                           installed and the per-platform servers under server/.
#   wavelet-lsp-<target>    Standalone language-server binaries.
#   wavelet-<target>        Standalone CLI binaries.
#   wavelet-<target>.tar.gz Per-platform tarball bundling the CLI + language
#                           server (as plain `wavelet` and `wavelet-lsp`), for
#                           the Homebrew formula to download and install.
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
# Run from anywhere. Requires `node`/`npm`, `zip`, and `tar`.
set -euo pipefail
cd "$(dirname "$0")/../.."
root="$(pwd)"

staged="${STAGED_BINS:-dist}"

mkdir -p dist

# Artifact upload/download can drop the exec bit; restore it so the binaries we
# bundle/repackage below stay runnable.
chmod +x "$staged"/wavelet-* 2>/dev/null || true

# VS Code extension -> unzips to a `wavelet/` dir you drop into the extensions
# folder. Install the runtime dependency (the language client) and bundle the
# per-platform servers under server/ so the extension launches the matching one
# with no extra download.
npm --prefix tooling/vscode install --omit=dev --no-audit --no-fund
rm -rf stage && mkdir -p stage/wavelet
cp -R tooling/vscode/. stage/wavelet/
mkdir -p stage/wavelet/server
cp "$staged"/wavelet-lsp-* stage/wavelet/server/
(cd stage && zip -r ../dist/wavelet-vscode.zip wavelet)
rm -rf stage

# Standalone binaries, published alongside the editor zips. Skip when the inputs
# already live in dist/ (local default) — copying a file onto itself errors
# under `set -e`.
if [ "$(cd "$staged" && pwd)" != "$(cd dist && pwd)" ]; then
  cp "$staged"/wavelet-* dist/
fi

# Per-platform tarballs for the Homebrew formula: each bundles the matching CLI
# and language server under plain names (wavelet, wavelet-lsp) so the formula
# can `bin.install "wavelet", "wavelet-lsp"`. Only built for unix targets that
# have *both* binaries; a target whose build leg failed is simply skipped, so a
# partial release still ships tarballs for whatever succeeded.
rm -rf brewpkg
for cli in "$staged"/wavelet-*; do
  base="$(basename "$cli")"
  case "$base" in
    wavelet-lsp-* | *.exe | *.zip | *.tar.gz) continue ;; # only unix CLI binaries
  esac
  target="${base#wavelet-}"
  lsp="$staged/wavelet-lsp-${target}"
  if [ ! -f "$lsp" ]; then
    echo "no wavelet-lsp-${target} alongside ${base}; skipping tarball" >&2
    continue
  fi
  mkdir -p "brewpkg/${target}"
  cp "$cli" "brewpkg/${target}/wavelet"
  cp "$lsp" "brewpkg/${target}/wavelet-lsp"
  chmod +x "brewpkg/${target}/wavelet" "brewpkg/${target}/wavelet-lsp"
  tar -czf "${root}/dist/wavelet-${target}.tar.gz" -C "brewpkg/${target}" wavelet wavelet-lsp
  echo "Bundled dist/wavelet-${target}.tar.gz"
done
rm -rf brewpkg

echo "Packaged editor tooling and collected binaries into dist/"
