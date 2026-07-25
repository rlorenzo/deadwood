#!/usr/bin/env bash
# Run the full local quality gate: format check, lints, tests.
# Usage: scripts/check.sh [--fix]  (--fix applies rustfmt and clippy fixes instead of checking)
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${1:-}" == "--fix" ]]; then
  cargo fmt --all
  cargo clippy --all-targets --fix --allow-dirty --allow-staged -- -D warnings
else
  echo "==> cargo fmt --check"
  cargo fmt --all -- --check
  echo "==> cargo clippy"
  cargo clippy --all-targets -- -D warnings
fi

echo "==> cargo test"
cargo test --all-targets
echo "==> OK"
