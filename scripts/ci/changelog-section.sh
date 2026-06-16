#!/usr/bin/env bash
# Print the CHANGELOG.md section for a single version, for use as a GitHub
# release body.
#
# Usage: scripts/ci/changelog-section.sh VERSION
#   VERSION  Release version, with or without a leading `v` (e.g. 0.2.4 or
#            v0.2.4). The matching `## [VERSION] - ...` section in CHANGELOG.md
#            is printed to stdout, without its own heading.
#
# Exits non-zero if the version has no section, so the release workflow fails
# loudly rather than publishing empty notes for an unrecorded version.
#
# Used by the `Release` GitHub workflow (scripts feed the body to
# softprops/action-gh-release). Run from anywhere.
set -euo pipefail
cd "$(dirname "$0")/../.."

version="${1:-}"
if [ -z "$version" ]; then
  echo "usage: scripts/ci/changelog-section.sh VERSION" >&2
  exit 2
fi
version="${version#v}"

# Print the lines after the `## [<version>]` heading, up to the next `## `
# heading or the trailing link-reference block (`[x]: url`), dropping the
# heading line itself.
section="$(awk -v ver="$version" '
  $0 ~ "^## \\[" ver "\\]" { grab = 1; next }
  grab && /^## / { exit }
  grab && /^\[[^]]+\]: / { exit }
  grab { print }
' CHANGELOG.md)"

# Trim leading/trailing blank lines.
section="$(printf '%s\n' "$section" | sed -e '/./,$!d' | sed -e ':a' -e '/^\n*$/{$d;N;ba' -e '}')"

if [ -z "$section" ]; then
  echo "no CHANGELOG.md section found for version $version" >&2
  exit 1
fi

printf '%s\n' "$section"
