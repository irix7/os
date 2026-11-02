#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Boot the pinned relibc probes on iris and assert their pass markers.
#
# Usage:
#   tools/relibc-pal/boot-probes.sh OUT_DIR PROM WORK_DIR [BASE_PORT]
#
# OUT_DIR is the tools/relibc-pal/build.sh output.  Each probe is attached as
# SCSI target 2, booted with the network off, and must print its pass marker
# and the zero exit code on both the CI socket and the serial-log mirror.  The
# probes that keep a check counter also print `failures=0`; the fork/exec
# parent prints `CHILD-OK` for the staged child and `FORK-EXEC-OK` only after
# every check passes, and takes the child image on SCSI target 3.  The boot
# stops at the first failure and leaves that probe's logs in WORK_DIR.
set -euo pipefail

OUT="${1:?usage: boot-probes.sh OUT_DIR PROM WORK_DIR [BASE_PORT]}"
ROM="${2:?usage: boot-probes.sh OUT_DIR PROM WORK_DIR [BASE_PORT]}"
WORK="${3:?usage: boot-probes.sh OUT_DIR PROM WORK_DIR [BASE_PORT]}"
BASE_PORT="${4:-18920}"

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
IRIS="${IRIS:-$(command -v iris || true)}"
IRIS_CI="${IRIS_CI:-$(command -v iris-ci || true)}"

for path in "$ROM" "$IRIS" "$IRIS_CI"; do
  if [ -z "$path" ] || [ ! -e "$path" ]; then
    echo "boot-probes: missing $path; set IRIS/IRIS_CI or install iris" >&2
    exit 2
  fi
done

mkdir -p "$WORK"
port="$BASE_PORT"

# The EXIT trap runs after the function stack has unwound, so the live probe's
# directory and process id cannot be function locals.
RUN_NAME=""
RUN_DIR=""
RUN_PID=""

run_probe() {
  local name="$1" raw="$2" marker="$3"
  local extra="${4:-}" child="${5:-}"
  local run="$WORK/$name"

  if [ ! -f "$raw" ]; then
    echo "boot-probes: missing $raw; run tools/relibc-pal/build.sh first" >&2
    exit 2
  fi
  if [ -n "$child" ] && [ ! -f "$child" ]; then
    echo "boot-probes: missing $child; run tools/relibc-pal/build.sh first" >&2
    exit 2
  fi

  rm -rf "$run"
  mkdir -p "$run"
  RUN_NAME="$name"
  RUN_DIR="$run"
  cat >"$run/machine.toml" <<EOF
banks = [64, 0, 0, 0]
no_audio = true

[machine]
profile = "indy_ip24"

[scsi]

[scsi.2]
path = "$raw"
cdrom = false
EOF
  if [ -n "$child" ]; then
    cat >>"$run/machine.toml" <<EOF

[scsi.3]
path = "$child"
cdrom = false
EOF
  fi

  python3 - "$run/nvram.bin" <<'PY'
import sys

env = b"net=off\0console=serial0\0\0"
image = bytearray(8192)
image[0x10:0x10 + len(env)] = env
image[314:320] = bytes([0x02, 0x11, 0x22, 0x33, 0x44, 0x55])
open(sys.argv[1], "wb").write(image)
PY

  echo "boot-probes: $name (monitor $port)"
  IRIS_NO_IDLE=1 "$IRIS" --config "$run/machine.toml" --ci \
    --prom "$ROM" --nvram "$run/nvram.bin" \
    --ci-socket "$run/iris.sock" --monitor-port "$port" \
    --serial-log "$run/serial.log" >"$run/iris.out" 2>&1 &
  RUN_PID=$!

  cleanup() {
    local status=$?
    trap - EXIT
    if [ -n "$RUN_PID" ]; then
      kill "$RUN_PID" 2>/dev/null || true
      wait "$RUN_PID" 2>/dev/null || true
    fi
    if [ "$status" -ne 0 ]; then
      echo "--- $RUN_NAME iris output (tail)" >&2
      tail -40 "$RUN_DIR/iris.out" >&2 || true
      echo "--- $RUN_NAME serial log (tail)" >&2
      tail -80 "$RUN_DIR/serial.log" >&2 || true
    fi
    exit "$status"
  }
  trap cleanup EXIT

  "$IRIS_CI" --socket "$run/iris.sock" status --wait-ready 30
  "$IRIS_CI" --socket "$run/iris.sock" start
  "$IRIS_CI" --socket "$run/iris.sock" serial-wait 'BOOT-READY' --timeout 120
  "$IRIS_CI" --socket "$run/iris.sock" serial-wait "$marker" --timeout 180
  # The counter probes print `checks=... failures=0` before their pass marker
  # (futex and sleep), so a socket cursor that has already passed the marker
  # cannot see it.  Read those extra assertions from the serial-log mirror,
  # which scans the whole file.
  "$IRIS_CI" --socket "$run/iris.sock" serial-wait \
    'user: exit syscall code=0x00000000' --timeout 120
  "$REPO/tools/wait-serial-log.sh" "$run/serial.log" "$marker" 30
  if [ -n "$extra" ]; then
    "$REPO/tools/wait-serial-log.sh" "$run/serial.log" "$extra" 30
  fi
  "$REPO/tools/wait-serial-log.sh" "$run/serial.log" \
    'user: exit syscall code=0x00000000' 30

  "$IRIS_CI" --socket "$run/iris.sock" quit
  wait "$RUN_PID"
  RUN_PID=""
  trap - EXIT
  port=$((port + 1))
}

run_probe probe "$OUT/probe.raw" 'IRIX7-RELIB-PROBE-4' 'failures=0'
run_probe thread "$OUT/thread-probe.raw" 'THREADS-OK' 'failures=0'
run_probe futex "$OUT/futex-probe.raw" 'FUTEX-OK' 'failures=0'
run_probe pthread "$OUT/pthread-probe.raw" 'PTHREADS-OK' 'failures=0'
run_probe sleep "$OUT/sleep-probe.raw" 'SLEEP-OK' 'failures=0'
run_probe sleep-cell "$OUT/sleep-cell-probe.raw" 'SLEEP-CELL-OK' 'failures=0'
run_probe forkexec "$OUT/forkexec-probe.raw" 'FORK-EXEC-OK' 'CHILD-OK' \
  "$OUT/forkexec-child.raw"

echo "boot-probes: all seven relibc images passed"
