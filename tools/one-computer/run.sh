#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the two-node artefacts if needed, then run the one-computer M2
# transcript.  The transcript is written to <work>/demo/transcript.txt.
#
# Usage:
#   tools/one-computer/run.sh [WORK]
#
# Environment:
#   IRIS            iris binary (default: iris in PATH)
#   IRIS_CI         iris-ci binary (default: iris-ci in PATH)
#   M2_RIG_BIN      prebuilt m2-rig (default: tools/m2-rig/target/release/m2-rig)
#   ROM             prebuilt PROM (default: <build>/irix7-rom.bin)
#   MODULE          prebuilt MCP module (default: <build>/mcp.module)
#   BUILD_DIR       artefact directory (default: <repo>/target/one-computer-build)
#   BASE_PORT       first monitor port block (default: 11000)
#   TIMEOUT         boot deadline in seconds (default: 240)
#   CARGO           cargo binary (default: cargo)
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
WORK="${1:-${WORK:-$REPO/target/one-computer}}"
BUILD="${BUILD_DIR:-$REPO/target/one-computer-build}"
BIN="${M2_RIG_BIN:-$REPO/tools/m2-rig/target/release/m2-rig}"
ROM="${ROM:-$BUILD/irix7-rom.bin}"
MODULE="${MODULE:-$BUILD/mcp.module}"
BASE_PORT="${BASE_PORT:-11000}"
TIMEOUT="${TIMEOUT:-240}"
CARGO="${CARGO:-cargo}"

IRIS="${IRIS:-$(command -v iris || true)}"
IRIS_CI="${IRIS_CI:-$(command -v iris-ci || true)}"
if [ -z "$IRIS" ] || [ ! -x "$IRIS" ]; then
  echo "one-computer: iris is not in PATH; set IRIS" >&2
  exit 2
fi
if [ -z "$IRIS_CI" ] || [ ! -x "$IRIS_CI" ]; then
  echo "one-computer: iris-ci is not in PATH; set IRIS_CI" >&2
  exit 2
fi

if [ ! -x "$BIN" ]; then
  echo "one-computer: building m2-rig"
  "$CARGO" build --release --manifest-path "$REPO/tools/m2-rig/Cargo.toml"
fi

if [ ! -f "$ROM" ] || [ ! -f "$MODULE" ]; then
  echo "one-computer: building the PROM and MCP module"
  "$REPO/tools/m2-rig/build.sh" "$BUILD"
fi

exec "$BIN" demo \
  --work "$WORK" \
  --rom "$ROM" \
  --module "$MODULE" \
  --iris "$IRIS" \
  --iris-ci "$IRIS_CI" \
  --base-port "$BASE_PORT" \
  --timeout "$TIMEOUT"
