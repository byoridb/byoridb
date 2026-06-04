#!/usr/bin/env bash
# CRAP score check: cyclomatic complexity (clippy cognitive_complexity proxy) × test coverage.
# Default policy: CRAP ≤ 30 (Crap4j default).
set -euo pipefail

THRESHOLD="${CRAP_THRESHOLD:-30}"
TOP="${CRAP_TOP:-30}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="${CRAP_WORK_DIR:-/tmp}"

cd "$REPO_ROOT"

echo "[1/3] Scanning cognitive complexity (threshold = 1, all functions)..."
saved=$(grep -E "^cognitive-complexity-threshold" clippy.toml || true)
trap 'if [ -n "$saved" ]; then echo "$saved" > clippy.toml; fi' EXIT
echo "cognitive-complexity-threshold = 1" > clippy.toml
cargo clippy --all-targets --all-features > "$WORK/cc_all.txt" 2>&1
cc_count=$(grep -c "the function has a cognitive complexity" "$WORK/cc_all.txt" || true)
echo "  → $cc_count functions with comp > 1"

echo "[2/3] Running coverage..."
cargo llvm-cov --workspace --no-fail-fast --json --output-path "$WORK/cov.json" > "$WORK/cov.log" 2>&1
echo "  → coverage report: $WORK/cov.json"

echo "[3/3] Computing CRAP scores..."
python3 "$REPO_ROOT/scripts/crap_analyze.py" \
  --clippy "$WORK/cc_all.txt" \
  --cov "$WORK/cov.json" \
  --repo "$REPO_ROOT" \
  --threshold "$THRESHOLD" \
  --top "$TOP"
