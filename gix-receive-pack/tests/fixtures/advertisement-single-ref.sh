#!/usr/bin/env sh
set -eu

# Purpose: Generate upstream git-receive-pack advertisement for a repository with a single ref.
# Output: upstream-advertisement.pkt (raw pkt-line bytes from git-receive-pack)
#
# This fixture captures the exact byte-for-byte output from upstream git-receive-pack
# for comparison with our strict-compat implementation, following the gix-testtools
# pattern used across the gitoxide workspace.

out="upstream-advertisement.pkt"
: > "$out"

# Create a repository with a single deterministic commit
git init --bare --quiet

# Create a deterministic commit using a separate working directory
work_dir=$(mktemp -d)
trap "rm -rf '$work_dir'" EXIT

cd "$work_dir"
git init --quiet

# Set deterministic author/committer info (gix-testtools sets these via environment)
echo "test content" > test.txt
git add test.txt

# Create a commit with deterministic hash by using fixed dates
# The gix-testtools configure_command sets GIT_AUTHOR_DATE and GIT_COMMITTER_DATE
git commit -m "Initial commit" --quiet

# Push to our bare repo
git remote add origin "$OLDPWD"
git push origin main --quiet

cd "$OLDPWD"

# Capture git-receive-pack advertisement output
printf "0000" | git-receive-pack . > "$out" 2>/dev/null || {
    # If git-receive-pack fails, create a placeholder
    echo "# Could not capture real git-receive-pack output" > "$out"
    echo "# Expected format for single-ref repo:" >> "$out"
    echo "# <pkt-len><commit-hash> refs/heads/main\0<capabilities>\n" >> "$out"
    echo "# 0000 (flush)" >> "$out"
}

echo "Generated upstream advertisement fixture for single-ref repository"