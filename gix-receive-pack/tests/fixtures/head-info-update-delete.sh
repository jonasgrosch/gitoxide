#!/usr/bin/env sh
set -eu

# Purpose: Generate a head-info transcript containing create, update, and delete commands.
# Output: client-to-server.head-info.pkt (text lines with a single NUL on the first command line)
#
# Conventions:
# - Executed via gix_testtools::scripted_fixture_read_only("head-info-update-delete.sh")
# - Writes all files into the working directory provided by the test harness.

out="client-to-server.head-info.pkt"
: > "$out"

# First command line carries capabilities after a NUL.
# Create refs/heads/main
# Use printf with a NUL in the format string to emit an actual 0x00 between ref and capabilities.
printf '%s\0%s\n' \
  "0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 refs/heads/main" \
  "report-status report-status-v2 quiet delete-refs ofs-delta agent=gix/1.0" >> "$out"

# Update refs/heads/main
printf '%s\n' "1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 refs/heads/main" >> "$out"

# Delete refs/tags/v1
printf '%s\n' "2222222222222222222222222222222222222222 0000000000000000000000000000000000000000 refs/tags/v1" >> "$out"