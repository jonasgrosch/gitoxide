#!/usr/bin/env sh
set -eu

# Purpose: Generate test pack data for pack ingestion testing.
# Output: test-pack.pack (valid pack file with deterministic content)
#         test-pack.idx (corresponding pack index)
#         invalid-pack.data (invalid pack data for failure testing)
#
# This fixture creates deterministic pack files for testing the pack ingestion
# engine wrappers, following the gix-testtools pattern.

# Create a temporary working directory for pack generation
work_dir=$(mktemp -d)
trap "rm -rf '$work_dir'" EXIT

cd "$work_dir"

# Initialize a repository with deterministic content
git init --quiet

# Set deterministic author/committer info
export GIT_AUTHOR_NAME="Test Author"
export GIT_AUTHOR_EMAIL="test@example.com"
export GIT_COMMITTER_NAME="Test Committer"
export GIT_COMMITTER_EMAIL="test@example.com"
export GIT_AUTHOR_DATE="2023-01-01T00:00:00Z"
export GIT_COMMITTER_DATE="2023-01-01T00:00:00Z"

# Create deterministic test content
echo "test blob content" > test-file.txt
echo "another test file" > another-file.txt
mkdir subdir
echo "nested content" > subdir/nested.txt

# Add files and create commits
git add .
git commit -m "Initial commit with test files" --quiet

# Create another commit for more pack content
echo "modified content" >> test-file.txt
echo "new file" > new-file.txt
git add .
git commit -m "Second commit with modifications" --quiet

# Create a third commit to ensure we have enough objects
echo "final content" > final-file.txt
git add final-file.txt
git commit -m "Third commit" --quiet

# Generate pack file using git pack-objects
# This creates a deterministic pack with all objects
git rev-list --objects --all | git pack-objects --stdout > "$OLDPWD/test-pack.pack"

# Generate the corresponding index file
cd "$OLDPWD"
git index-pack test-pack.pack

# Verify the pack is valid
git verify-pack -v test-pack.pack > pack-contents.txt || {
    echo "Warning: Could not verify generated pack"
}

# Create invalid pack data for failure testing
echo "INVALID_PACK_DATA_FOR_TESTING" > invalid-pack.data

# Create a pack that's too large for size limit testing
dd if=/dev/zero of=large-pack.data bs=1024 count=1024 2>/dev/null || {
    # Fallback if dd is not available
    python3 -c "print('X' * (1024 * 1024))" > large-pack.data 2>/dev/null || {
        # Final fallback
        for i in $(seq 1 1024); do
            echo "LARGE_PACK_DATA_LINE_$i" >> large-pack.data
        done
    }
}

echo "Generated pack ingestion test fixtures:"
echo "  test-pack.pack: $(wc -c < test-pack.pack) bytes"
echo "  test-pack.idx: $(wc -c < test-pack.idx) bytes"
echo "  invalid-pack.data: $(wc -c < invalid-pack.data) bytes"
echo "  large-pack.data: $(wc -c < large-pack.data) bytes"

# Output pack statistics for debugging
if [ -f pack-contents.txt ]; then
    echo "Pack contents:"
    head -10 pack-contents.txt
fi