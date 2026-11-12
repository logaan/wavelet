#!/usr/bin/env bash

set -euo pipefail
cd "$(dirname "$0")/../.."
root="$(pwd)"

staged="${STAGED_BINS:-dist}"

mkdir -p dist

chmod +x "$staged"/wavelet-* 2>/dev/null || true

npm --prefix tooling/vscode install --omit=dev --no-audit --no-fund
rm -rf stage && mkdir -p stage/wavelet
cp -R tooling/vscode/. stage/wavelet/
cp LICENSE stage/wavelet/LICENSE
mkdir -p stage/wavelet/server
cp "$staged"/wavelet-lsp-* stage/wavelet/server/
(cd stage && zip -r ../dist/wavelet-vscode.zip wavelet)
rm -rf stage

if [ "$(cd "$staged" && pwd)" != "$(cd dist && pwd)" ]; then
  cp "$staged"/wavelet-* dist/
fi

rm -rf brewpkg
for cli in "$staged"/wavelet-*; do
  base="$(basename "$cli")"
  case "$base" in
    wavelet-lsp-* | *.exe | *.zip | *.tar.gz) continue ;;
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
  cp LICENSE "brewpkg/${target}/LICENSE"
  chmod +x "brewpkg/${target}/wavelet" "brewpkg/${target}/wavelet-lsp"
  tar -czf "${root}/dist/wavelet-${target}.tar.gz" -C "brewpkg/${target}" wavelet wavelet-lsp LICENSE
  echo "Bundled dist/wavelet-${target}.tar.gz"
done
rm -rf brewpkg

echo "Packaged editor tooling and collected binaries into dist/"
