#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Wait for a regular expression in a `--serial-log` mirror.
#
# `iris-ci serial-wait` matches the CI socket stream and can return as soon as
# the line prefix arrives; the `--serial-log` mirror is a separate writer and
# can still be a few bytes behind.  A boot test that greps the file in the same
# instant races the mirror.  Poll the same assertion with a bounded deadline
# instead of adding a sleep.
#
# Usage: wait-serial-log.sh FILE PATTERN [TIMEOUT_SECS]

set -euo pipefail
file="${1:?usage: wait-serial-log.sh FILE PATTERN [TIMEOUT_SECS]}"
pattern="${2:?usage: wait-serial-log.sh FILE PATTERN [TIMEOUT_SECS]}"
timeout="${3:-30}"
deadline=$((SECONDS + timeout))
while :; do
  if [ -f "$file" ] && grep -Eq -- "$pattern" "$file"; then
    exit 0
  fi
  if [ "$SECONDS" -ge "$deadline" ]; then
    echo "wait-serial-log: $file never contained: $pattern" >&2
    exit 1
  fi
  sleep 0.2
done
