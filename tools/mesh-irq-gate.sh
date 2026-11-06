#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# The #336 receive-interrupt gate.
#
# Builds the PROM (with the kernel embedded), the MCP module and the two-node
# rig, runs the host tests for the interrupt policy and the rig parsers, then
# boots two real kernels and measures:
#
#   hot   p50/p95/p99  at a 20 ms sample gap (inside the kernel activity window)
#   cold  p50/p95/p99  at a 250 ms sample gap (parked idle loop)
#   idle  the guest's own busy share of the idle loop, from CP0 Count
#
# and verifies the receive-interrupt counters moved.  The gate fails when the
# cold p50 is not substantially below the 10 ms tick, when the hot path
# regresses beyond the readiness-poll numbers, when the idle loop does not
# park, or when a spurious interrupt appears.
#
# The polled control arm (`--rx-poll`, net.rx=off) is the A/B baseline.  It is
# expected to miss the interrupt thresholds, so it is recorded rather than
# gated; set M2_IRQ_GATE_COMPARE=1 to run it.
#
# Usage:
#   tools/mesh-irq-gate.sh [WORK_DIR]
#
# Environment:
#   IRIS, IRIS_CI      emulator binaries (default: ../iris/target/release/...)
#   M2_BASE_PORT       first port block (default: 16600)
#   M2_IRQ_GATE_COMPARE 1 to also run the polled control arm
#   M2_IRQ_GATE_BUILD  reuse an existing build directory with irix7-rom.bin
#                      instead of rebuilding
#   M2_IRQ_GATE_BIN    reuse an existing m2-rig binary instead of rebuilding
#   CARGO              cargo binary (default: cargo)

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
WORK="${1:-${M2_IRQ_GATE_WORK:-$REPO/target/mesh-irq-gate}}"
BASE_PORT="${M2_BASE_PORT:-16600}"
CARGO="${CARGO:-cargo}"
IRIS="${IRIS:-$REPO/../iris/target/release/iris}"
IRIS_CI="${IRIS_CI:-$REPO/../iris/target/release/iris-ci}"

if ! command -v "$CARGO" >/dev/null 2>&1; then
  echo "mesh-irq-gate: $CARGO is not in PATH; source the toolchain environment first" >&2
  exit 2
fi
if [ ! -x "$IRIS" ] || [ ! -x "$IRIS_CI" ]; then
  echo "mesh-irq-gate: iris/iris-ci not found (set IRIS and IRIS_CI)" >&2
  exit 2
fi

mkdir -p "$WORK"
BUILD="${M2_IRQ_GATE_BUILD:-$WORK/build}"
HARNESS="$WORK/harness"
if [ -z "${M2_IRQ_GATE_BUILD:-}" ]; then
  echo "mesh-irq-gate: build artefacts in $BUILD"
  "$HERE/m2-rig/build.sh" "$BUILD"
fi
for artefact in irix7-rom.bin; do
  if [ ! -f "$BUILD/$artefact" ]; then
    echo "mesh-irq-gate: $BUILD/$artefact is missing" >&2
    exit 2
  fi
done

echo "mesh-irq-gate: host tests"
"$CARGO" test -p kernel --test net_irq
"$CARGO" test --manifest-path "$HERE/m2-rig/Cargo.toml"

if [ -n "${M2_IRQ_GATE_BIN:-}" ]; then
  BIN="$M2_IRQ_GATE_BIN"
else
  echo "mesh-irq-gate: build the rig"
  "$CARGO" build --release --manifest-path "$HERE/m2-rig/Cargo.toml" \
    --target-dir "$HARNESS"
  BIN="$HARNESS/release/m2-rig"
fi

run_arm() {
  name="$1"
  port="$2"
  shift 2
  "$BIN" irq-gate \
    --work "$WORK/$name" \
    --rom "$BUILD/irix7-rom.bin" \
    --iris "$IRIS" --iris-ci "$IRIS_CI" \
    --base-port "$port" --timeout 300 "$@" 2>&1 | tee "$WORK/$name.log"
  return "${PIPESTATUS[0]}"
}

echo "mesh-irq-gate: interrupt arm"
if ! run_arm irq "$BASE_PORT"; then
  echo "mesh-irq-gate: FAIL (interrupt arm missed its thresholds; see $WORK/irq.log)" >&2
  exit 1
fi

# The thresholds are printed by the rig; check the verdict line separately so a
# silently skipped measurement cannot pass.
if ! grep -q '^mesh-irq-gate: verdict=pass$' "$WORK/irq.log"; then
  echo "mesh-irq-gate: FAIL (no pass verdict in $WORK/irq.log)" >&2
  exit 1
fi
if ! grep -Eq '^mesh-irq-gate: (n0|n1) cold .*p50=[0-9]+ ' "$WORK/irq.log"; then
  echo "mesh-irq-gate: FAIL (no cold series in $WORK/irq.log)" >&2
  exit 1
fi
if ! grep -Eq '^mesh-irq-gate: irq .*rx_final=[1-9][0-9]* ' "$WORK/irq.log"; then
  echo "mesh-irq-gate: FAIL (the handler counted no receive interrupt)" >&2
  exit 1
fi

if [ "${M2_IRQ_GATE_COMPARE:-0}" = "1" ]; then
  echo "mesh-irq-gate: polled control arm (expected to miss the cold target)"
  if ! run_arm poll "$((BASE_PORT + 400))" --rx-poll; then
    echo "mesh-irq-gate: control arm recorded (non-zero, as expected)"
  fi
fi

echo "mesh-irq-gate: ok"
