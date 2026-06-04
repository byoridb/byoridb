#!/bin/sh
#
# Setup git hooks for ByoriDB development
#

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

echo "Setting up git hooks..."

# Configure git to use .githooks directory
git config core.hooksPath .githooks

echo "Git hooks configured successfully!"
echo ""
echo "The following hooks are now active:"
echo "  - pre-commit: Checks code formatting with 'cargo fmt'"
echo ""
echo "To disable hooks temporarily, use: git commit --no-verify"
