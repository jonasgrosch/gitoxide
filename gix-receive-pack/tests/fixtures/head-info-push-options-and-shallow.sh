#!/usr/bin/env sh
set -eu

# Purpose: Generate a head-info transcript with push-options and shallow lines.
# Output: client-to-server.head-info.pkt (text lines; first command line contains a single NUL before capabilities)
#
# Conventions:
# - Executed via gix_testtools::scripted_fixture_read_only("head-info-push-options-and-shallow.sh")
# - Writes all files into the working directory provided by the test harness.

out="client-to-server.head-info.pkt"
: > "$out"

# First command line carries capabilities after a NUL (create main)
# Use printf with a NUL in the format string to emit an actual 0x00 between ref and capabilities.
printf '%s\0%s\n' \
  "0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 refs/heads/main" \
  "report-status report-status-v2 quiet delete-refs ofs-delta agent=gix/1.0" >> "$out"

# Additional recognized lines
printf '%s\n' "push-option=ci-skip=true" >> "$out"
printf '%s\n' "push-option=notify=team" >> "$out"
printf '%s\n' "shallow 3333333333333333333333333333333333333333" >> "$out"