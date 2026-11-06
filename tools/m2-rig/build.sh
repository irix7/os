#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the artefacts the two-node rig needs:
#   <work>/irix7-rom.bin  PROM; the kernel is downloaded at boot (M1-ADR-0002)
#   <work>/irix7-rom-n0.bin, irix7-rom-n1.bin  per-node PROMs, when M2_KERNEL_PORT is set
#   <work>/kernel-indy-latest.bin  the kernel those PROMs fetch, when M2_KERNEL_PORT is set
#   <work>/SHA256SUMS
#
# The kernel and ROM are cross-built with the pinned nightly and the repository
# target spec.  MCP is linked into the kernel, so there is no separate module
# image.  `CARGO_TARGET_DIR` is kept inside the work directory by default so the
# rig does not disturb a developer's main target directory.
#
# Usage:
#   tools/m2-rig/build.sh [WORK_DIR]
#
# Environment:
#   CARGO              cargo binary (default: cargo)
#   CC                 host C compiler used by build scripts (optional)
#   CARGO_TARGET_DIR   override the target directory
#   M2_RIG_OPT_LEVEL   kernel opt level (default: z, matching CI; the M1
#                      remediation no longer fits the PROM at opt-level s)
#   M2_KERNEL_PORT     serve this kernel from <work> on this port.  Each node's
#                      PROM fetches it through that node's NAT gateway
#                      (192.168.<n>.1), so the gate tests the kernel this build
#                      compiled.  Unset, the PROM fetches the release kernel.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
WORK="${1:-${M2_RIG_WORK:-$REPO/target/m2-rig}}"
CARGO="${CARGO:-cargo}"
OPT_LEVEL="${M2_RIG_OPT_LEVEL:-z}"

mkdir -p "$WORK"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$WORK/target}"
TARGET_JSON="$REPO/targets/mips3-sgi-irix7-n32.json"
TARGET="mips3-sgi-irix7-n32"

if ! command -v "$CARGO" >/dev/null 2>&1; then
  echo "m2-rig: $CARGO is not in PATH; source the toolchain environment first" >&2
  exit 2
fi

echo "m2-rig: repository $REPO"
echo "m2-rig: work       $WORK"
echo "m2-rig: target dir $CARGO_TARGET_DIR"

cd "$REPO"

echo "m2-rig: build kernel"
CARGO_PROFILE_RELEASE_OPT_LEVEL="$OPT_LEVEL" \
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tkernel/linker.ld" \
  "$CARGO" build --release -Z build-std=core,alloc -Zjson-target-spec \
    --features cross --target "$TARGET_JSON" -p kernel

# Build one PROM.  The caller sets IRIX7_KERNEL_URL, which the downloader
# compiles in, so the caller picks where the kernel comes from.
build_rom() {
  local out="$1"
  CARGO_PROFILE_RELEASE_OPT_LEVEL=z CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
  CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Trom/linker.ld" \
    "$CARGO" build --release -Z build-std=core,alloc -Zjson-target-spec \
      --features cross --target "$TARGET_JSON" -p rom
  python3 rom/tools/make_prom.py \
    "$CARGO_TARGET_DIR/$TARGET/release/rom" "$out"
  local rom_size
  rom_size="$(stat -c%s "$out")"
  if [ "$rom_size" -gt 524288 ]; then
    echo "m2-rig: PROM is $rom_size bytes, over the 524288-byte window" >&2
    exit 1
  fi
  echo "m2-rig: PROM $rom_size bytes ($out)"
}

if [ -n "${M2_KERNEL_PORT:-}" ]; then
  echo "m2-rig: build the kernel to serve, and a PROM per node"
  HOST="$(rustc -vV | sed -n 's/^host: //p')"
  LLVM_OBJCOPY="$(rustc --print sysroot)/lib/rustlib/$HOST/bin/llvm-objcopy"
  CARGO_PROFILE_RELEASE_OPT_LEVEL="$OPT_LEVEL" \
  CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tkernel/linker.ld" \
    "$CARGO" build --release -Z build-std=core,alloc -Zjson-target-spec \
      --features cross --target "$TARGET_JSON" -p kernel
  "$LLVM_OBJCOPY" -O binary "$CARGO_TARGET_DIR/$TARGET/release/kernel" \
    "$WORK/kernel-indy-latest.bin"
  export IRIX7_KERNEL_URL="http://192.168.0.1:$M2_KERNEL_PORT/kernel-indy-latest.bin"
  build_rom "$WORK/irix7-rom-n0.bin"
  export IRIX7_KERNEL_URL="http://192.168.1.1:$M2_KERNEL_PORT/kernel-indy-latest.bin"
  build_rom "$WORK/irix7-rom-n1.bin"
  cp "$WORK/irix7-rom-n0.bin" "$WORK/irix7-rom.bin"
else
  echo "m2-rig: build PROM"
  build_rom "$WORK/irix7-rom.bin"
fi

(
  cd "$WORK"
  sha256sum irix7-rom.bin >SHA256SUMS
  cat SHA256SUMS
)
# Record the build provenance next to the artefacts.  A feature branch or an
# uncommitted edit must be visible: a bare `HEAD` is not a reproducible claim.
commit="$(git -C "$REPO" rev-parse HEAD 2>/dev/null || echo unknown)"
describe="$(git -C "$REPO" describe --always --dirty --tags 2>/dev/null || echo unknown)"
if [ -z "$(git -C "$REPO" status --porcelain 2>/dev/null)" ]; then
  dirty=false
else
  dirty=true
fi
{
  echo "commit=$commit"
  echo "describe=$describe"
  echo "dirty=$dirty"
  echo "rustc=$(rustc -vV | sed -n '1p')"
  echo "rom=$(stat -c%s "$WORK/irix7-rom.bin") bytes"
} >"$WORK/BUILD-PROVENANCE.txt"
cat "$WORK/BUILD-PROVENANCE.txt"
