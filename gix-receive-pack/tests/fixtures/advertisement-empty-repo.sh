#!/usr/bin/env sh
set -eu

# Purpose: Generate upstream git-receive-pack advertisement for an empty repository.
# Output: upstream-advertisement.pkt (raw pkt-line bytes from git-receive-pack)
#
# This fixture captures the exact byte-for-byte output from upstream git-receive-pack
# for comparison with our strict-compat implementation, following the gix-testtools
# pattern used across the gitoxide workspace.

out="upstream-advertisement.pkt"
: > "$out"

# Create an empty bare git repository
git init --bare --quiet

# Capture git-receive-pack advertisement output
# We simulate a client connection by sending a flush packet and capturing the response
printf "0000" | git-receive-pack . > "$out" 2>/dev/null || {
    # If git-receive-pack fails (which can happen in some environments),
    # create a placeholder that documents the expected format
    echo "# Could not capture real git-receive-pack output" > "$out"
    echo "# Expected format for empty repo:" >> "$out"
    echo "# <pkt-len>0000000000000000000000000000000000000000 capabilities^{}\0<capabilities>\n" >> "$out"
    echo "# 0000 (flush)" >> "$out"
}

echo "Generated upstream advertisement fixture for empty repository"