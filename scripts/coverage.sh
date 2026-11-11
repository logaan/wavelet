#!/usr/bin/env bash

set -euo pipefail
cd "$(dirname "$0")/.."

if ! cargo llvm-cov --version >/dev/null 2>&1; then
  echo "==> cargo-llvm-cov not found; installing"
  cargo install cargo-llvm-cov --locked
fi

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
  --check)
    minimum=$(python3 -c 'import json; print(json.load(open("quality-coverage.json"))["minimum_line_percent"])')
    cargo llvm-cov --json --summary-only --output-path "$out_dir/summary.json" --fail-under-lines "$minimum"
    ;;
  "")
    echo "==> Running tests under coverage; printing summary"
    cargo llvm-cov --json --summary-only --output-path "$out_dir/summary.json"
    ;;
  *)
    echo "usage: scripts/coverage.sh [--html|--lcov|--check]" >&2
    exit 2
    ;;
esac
