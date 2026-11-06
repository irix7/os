#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# #337 stream gate: host stream-policy tests plus a two-kernel control-during-
# bulk, cancel/reset and reuse run.
#
# CI can call this as one step after the two-node build, with `iris` and
# `iris-ci` on PATH:
#
#   tools/mesh-stream-gate.sh
#
# `--host-only` runs only the host suites, so a machine without the emulator
# still gates the stream policy.
#
# Environment:
#   IRIS, IRIS_CI      emulator binaries when --iris/--iris-ci are omitted
#   CARGO              cargo binary (default: cargo)
#   M2_STREAM_WORK     work directory (default: <repo>/target/mesh-stream-gate)
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
WORK="${M2_STREAM_WORK:-$REPO/target/mesh-stream-gate}"
BASE_PORT=10500
TIMEOUT=240
TRANSPORT=direct
HOST_ONLY=false
KEEP=false
VERBOSE=false
IRIS_BIN="${IRIS:-}"
IRIS_CI_BIN="${IRIS_CI:-}"
CARGO="${CARGO:-cargo}"

usage() {
  cat <<'EOF'
usage: tools/mesh-stream-gate.sh [options]

  --host-only          run the mesh-stream and m2-rig host tests only
  --work DIR           work directory (default: <repo>/target/mesh-stream-gate)
  --base-port N        first rig port block (default: 10500)
  --timeout SECS       boot deadline (default: 240)
  --transport MODE     direct (default) or relay
  --iris PATH          iris binary
  --iris-ci PATH       iris-ci binary
  --keep               leave the two kernels running
  --verbose            pass extra rig detail
  -h, --help           this text
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --host-only) HOST_ONLY=true ;;
    --work)
      WORK="$2"
      shift
      ;;
    --base-port)
      BASE_PORT="$2"
      shift
      ;;
    --timeout)
      TIMEOUT="$2"
      shift
      ;;
    --transport)
      TRANSPORT="$2"
      shift
      ;;
    --iris)
      IRIS_BIN="$2"
      shift
      ;;
    --iris-ci)
      IRIS_CI_BIN="$2"
      shift
      ;;
    --keep) KEEP=true ;;
    --verbose) VERBOSE=true ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "mesh-stream-gate: unknown option $1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

if ! command -v "$CARGO" >/dev/null 2>&1; then
  echo "mesh-stream-gate: $CARGO is not in PATH; source the toolchain environment first" >&2
  exit 2
fi

echo "mesh-stream-gate: repository $REPO"
echo "mesh-stream-gate: work       $WORK"
mkdir -p "$WORK"

echo "mesh-stream-gate: host stream policy"
"$CARGO" test -p mesh-stream --manifest-path "$REPO/Cargo.toml"

echo "mesh-stream-gate: host rig tests"
"$CARGO" test --manifest-path "$REPO/tools/m2-rig/Cargo.toml"

if [ "$HOST_ONLY" = true ]; then
  echo "mesh-stream-gate: host-only pass"
  exit 0
fi

if [ -z "$IRIS_BIN" ]; then
  IRIS_BIN="$(command -v iris || true)"
fi
if [ -z "$IRIS_CI_BIN" ]; then
  IRIS_CI_BIN="$(command -v iris-ci || true)"
fi
if [ -z "$IRIS_BIN" ] || [ ! -x "$IRIS_BIN" ]; then
  echo "mesh-stream-gate: iris not found; pass --iris or set IRIS" >&2
  exit 2
fi
if [ -z "$IRIS_CI_BIN" ] || [ ! -x "$IRIS_CI_BIN" ]; then
  echo "mesh-stream-gate: iris-ci not found; pass --iris-ci or set IRIS_CI" >&2
  exit 2
fi

BUILD="$WORK/build"
echo "mesh-stream-gate: build the two-node artefacts"
"$REPO/tools/m2-rig/build.sh" "$BUILD"

HOST_TARGET="$WORK/host-target"
echo "mesh-stream-gate: build the rig"
"$CARGO" build --release \
  --manifest-path "$REPO/tools/m2-rig/Cargo.toml" \
  --target-dir "$HOST_TARGET"
BIN="$HOST_TARGET/release/m2-rig"

RUN_ARGS=(
  stream
  --work "$WORK/run"
  --rom "$BUILD/irix7-rom.bin"
  --iris "$IRIS_BIN"
  --iris-ci "$IRIS_CI_BIN"
  --base-port "$BASE_PORT"
  --timeout "$TIMEOUT"
  --transport "$TRANSPORT"
)
if [ "$KEEP" = true ]; then
  RUN_ARGS+=(--keep)
fi
if [ "$VERBOSE" = true ]; then
  RUN_ARGS+=(--verbose)
fi

echo "mesh-stream-gate: two-kernel stream gate"
"$BIN" "${RUN_ARGS[@]}"

echo "mesh-stream-gate: evidence $WORK/run/stream-evidence.json"
