#!/usr/bin/env bash

set -euo pipefail
cd "$(dirname "$0")/../.."

cd docs
npm ci
npm run build
