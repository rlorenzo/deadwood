#!/usr/bin/env bash
# Mutation-test the code changed relative to a base ref (default main):
# cargo-mutants rewrites each changed function in turn and expects some test
# to notice. A surviving mutant is a recall gap the review's hand-picked
# inversions missed. The full crate holds ~960 mutants — hours of wall
# clock — which is why this runs on the diff, not the tree. The diff is
# between commits, so commit first: uncommitted edits are invisible here
# and produce "nothing to mutate", not a clean bill.
# Usage: scripts/mutants.sh [base-ref]
set -euo pipefail
cd "$(dirname "$0")/.."

base="${1:-main}"
diff_file=$(mktemp "${TMPDIR:-/tmp}/mutants-diff.XXXXXX")
trap 'rm -f "$diff_file"' EXIT
git diff "$base"...HEAD > "$diff_file"
if [[ ! -s "$diff_file" ]]; then
  echo "no changes against $base; nothing to mutate"
  exit 0
fi
cargo mutants --in-diff "$diff_file"
