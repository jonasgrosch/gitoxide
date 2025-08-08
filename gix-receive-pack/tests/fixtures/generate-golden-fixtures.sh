#!/usr/bin/env sh
set -eu

# Purpose: Generate all golden advertisement fixtures for comparison with upstream git-receive-pack.
# This script runs all the individual fixture generation scripts.

echo "Generating golden advertisement fixtures..."

# Check if git-receive-pack is available
if ! command -v git-receive-pack >/dev/null 2>&1; then
    echo "Warning: git-receive-pack not found in PATH. Fixtures may not be generated correctly."
    echo "Please ensure Git is installed and git-receive-pack is available."
fi

# Generate all fixtures
for script in advertisement-*.sh; do
    if [ -f "$script" ] && [ "$script" != "generate-golden-fixtures.sh" ]; then
        echo "Running $script..."
        if ./"$script"; then
            echo "✓ $script completed successfully"
        else
            echo "✗ $script failed"
        fi
    fi
done

echo "Golden fixture generation complete."
echo ""
echo "To run the golden tests:"
echo "  cargo test --features strict-compat golden_scaffolding_tests"
echo ""
echo "Note: Golden tests are marked with #[ignore] by default as they require"
echo "upstream git-receive-pack and may be environment-dependent."
echo "Run with: cargo test --features strict-compat golden_scaffolding_tests -- --ignored"