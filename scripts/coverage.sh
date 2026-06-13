#!/usr/bin/env bash
# Measure native test coverage for the Rust crate with cargo-llvm-cov.
#
# cargo-llvm-cov drives LLVM's source-based coverage: it instruments the build,
# runs the full test suite (unit tests + the docs-example suite in
# tests/examples.rs), and reports which lines/regions the tests exercised.
#
# Coverage is measured on the NATIVE target only. The wasm `cdylib` build is the
# playground binding, not a test target, so it is irrelevant here.
#
# Usage:
#   scripts/coverage.sh            print a per-file summary table to the terminal
#   scripts/coverage.sh --html     also write an HTML report and open it
#   scripts/coverage.sh --lcov     also write target/coverage/lcov.info (for CI
#                                   / editor gutters)
#
# Run from anywhere. Bootstraps cargo-llvm-cov and the llvm-tools component on
# first run if they are missing.
set -euo pipefail
cd "$(dirname "$0")/.."

if ! cargo llvm-cov --version >/dev/null 2>&1; then
  echo "==> cargo-llvm-cov not found; installing"
  cargo install cargo-llvm-cov
fi

# Source-based coverage needs the LLVM tools shipped as a rustup component.
if command -v rustup >/dev/null 2>&1; then
  rustup component add llvm-tools-preview >/dev/null 2>&1 || true
fi

out_dir="target/coverage"
mkdir -p "$out_dir"

case "${1:-}" in
  --html)
    echo "==> Running tests under coverage; writing HTML report"
    cargo llvm-cov --html --output-dir "$out_dir/html"
    echo "HTML report: $out_dir/html/index.html"
    open "$out_dir/html/index.html" 2>/dev/null || true
    ;;
  --lcov)
    echo "==> Running tests under coverage; writing lcov.info"
    cargo llvm-cov --lcov --output-path "$out_dir/lcov.info"
    echo "lcov report: $out_dir/lcov.info"
    ;;
  "")
    echo "==> Running tests under coverage; printing summary"
    cargo llvm-cov --summary-only
    ;;
  *)
    echo "usage: scripts/coverage.sh [--html|--lcov]" >&2
    exit 2
    ;;
esac
