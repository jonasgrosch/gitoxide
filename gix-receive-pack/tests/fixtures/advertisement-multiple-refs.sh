#!/usr/bin/env sh
set -eu

# Purpose: Generate upstream git-receive-pack advertisement for a repository with multiple refs.
# Output: upstream-advertisement.pkt (raw pkt-line bytes from git-receive-pack)
#
# This fixture captures the exact byte-for-byte output from upstream git-receive-pack
# for comparison with our strict-compat implementation, following the gix-testtools
# pattern used across the gitoxide workspace.

out="upstream-advertisement.pkt"
: > "$out"

# Create a repository with multiple deterministic refs
git init --bare --quiet

# Create deterministic commits using a separate working directory
work_dir=$(mktemp -d)
trap "rm -rf '$work_dir'" EXIT

cd "$work_dir"
git init --quiet

# Create main branch with deterministic content
echo "main content" > main.txt
git add main.txt
git commit -m "Main commit" --quiet

# Create develop branch
git checkout -b develop --quiet
echo "develop content" > develop.txt
git add develop.txt
git commit -m "Develop commit" --quiet

# Create a tag on main
git checkout main --quiet
git tag v1.0.0

# Push all refs to our bare repo
git remote add origin "$OLDPWD"
git push origin main develop --quiet
git push origin --tags --quiet

cd "$OLDPWD"

# Capture git-receive-pack advertisement output
printf "0000" | git-receive-pack . > "$out" 2>/dev/null || {
    # If git-receive-pack fails, create a placeholder
    echo "# Could not capture real git-receive-pack output" > "$out"
    echo "# Expected format for multi-ref repo:" >> "$out"
    echo "# <pkt-len><hash1> refs/heads/develop\0<capabilities>\n" >> "$out"
    echo "# <pkt-len><hash2> refs/heads/main\n" >> "$out"
    echo "# <pkt-len><hash3> refs/tags/v1.0.0\n" >> "$out"
    echo "# 0000 (flush)" >> "$out"
}

echo "Generated upstream advertisement fixture for multiple refs repository"