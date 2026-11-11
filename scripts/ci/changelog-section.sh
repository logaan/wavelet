#!/usr/bin/env bash

set -euo pipefail
cd "$(dirname "$0")/../.."

version="${1:-}"
if [ -z "$version" ]; then
  echo "usage: scripts/ci/changelog-section.sh VERSION" >&2
  exit 2
fi
version="${version#v}"

section="$(awk -v ver="$version" '
  $0 ~ "^## \\[" ver "\\]" { grab = 1; next }
  grab && /^## / { exit }
  grab && /^\[[^]]+\]: / { exit }
  grab { print }
' CHANGELOG.md)"

section="$(printf '%s\n' "$section" | sed -e '/./,$!d' | sed -e ':a' -e '/^\n*$/{$d;N;ba' -e '}')"

if [ -z "$section" ]; then
  echo "no CHANGELOG.md section found for version $version" >&2
  exit 1
fi

printf '%s\n' "$section"
