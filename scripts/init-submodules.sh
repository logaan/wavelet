#!/usr/bin/env bash
# Fetch and check out this repo's git submodules.
#
# The only submodule today is `tooling/neovim` — the `logaan/wavelet.nvim`
# Neovim plugin, vendored here so `tooling/` holds all the editor integrations.
# A plain `git clone` leaves submodule directories empty; run this once after
# cloning (or after a branch switch that changes the submodule pointer) to
# populate them at the pinned commit.
#
# Equivalent to cloning with `git clone --recursive`.
#
# Run from anywhere.
set -euo pipefail
cd "$(dirname "$0")/.."

git submodule update --init --recursive
