#!/usr/bin/env sh
set -eu

# Purpose: Generate a synthetic head-info transcript for an empty-create push.
# Output: client-to-server.head-info.pkt (text lines, includes a NUL between ref line and capabilities)
#
# Notes:
# - We replicate repository-wide fixture generation patterns used elsewhere by using scripts,
#   executed at test-time via gix_testtools::scripted_fixture_read_only().
# - We avoid compiling upstream C; this file simply writes the logical head-info content our parser expects.

out="client-to-server.head-info.pkt"
: > "$out"

# First command line carries capabilities after a NUL.
# Create: old is all-zeros; new is a concrete object; refname is refs/heads/main.
# Capabilities chosen to reflect modern defaults incl. agent.
# Use printf with a NUL in the format string to emit an actual 0x00 between ref and capabilities.
printf '%s\0%s\n' \
  "0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 refs/heads/main" \
  "report-status report-status-v2 quiet delete-refs ofs-delta agent=gix/1.0" >> "$out"

# No further lines are needed for this minimal case.