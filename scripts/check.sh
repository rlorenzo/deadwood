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

# The module docs of `src/baseline.rs` are the specification, and a rename that
# leaves a `[\`Type\`]` link behind turns part of that specification into text
# nothing checks. Only the broken-link lint is denied: those docs deliberately
# link to crate-private items, which rustdoc also warns about and which is not a
# defect.
echo "==> cargo doc"
RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links" cargo doc --no-deps --document-private-items --quiet

echo "==> cargo test"
cargo test --all-targets
echo "==> OK"
