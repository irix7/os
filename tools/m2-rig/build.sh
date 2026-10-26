#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the artefacts the two-node rig needs:
#   <work>/irix7-rom.bin  PROM with the kernel embedded
#   <work>/mcp.module     fresh MCP control-plane module
#   <work>/SHA256SUMS
#
# The kernel and ROM are cross-built with the pinned nightly and the repository
# target spec.  The module is a flat image, so llvm-objcopy strips the ELF
# container.  `CARGO_TARGET_DIR` is kept inside the work directory by default so
# the rig does not disturb a developer's main target directory.
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

echo "m2-rig: build ROM"
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Trom/linker.ld" \
  "$CARGO" build --release -Z build-std=core,alloc -Zjson-target-spec \
    --features cross --target "$TARGET_JSON" -p rom

python3 rom/tools/make_prom.py \
  "$CARGO_TARGET_DIR/$TARGET/release/rom" "$WORK/irix7-rom.bin"
rom_size="$(stat -c%s "$WORK/irix7-rom.bin")"
if [ "$rom_size" -gt 524288 ]; then
  echo "m2-rig: PROM is $rom_size bytes, over the 524288-byte window" >&2
  exit 1
fi
echo "m2-rig: PROM $rom_size bytes"

echo "m2-rig: build MCP module"
HOST="$(rustc -vV | sed -n 's/^host: //p')"
LLVM_OBJCOPY="$(rustc --print sysroot)/lib/rustlib/$HOST/bin/llvm-objcopy"
if [ ! -x "$LLVM_OBJCOPY" ]; then
  echo "m2-rig: llvm-objcopy not found at $LLVM_OBJCOPY" >&2
  exit 1
fi
CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tmodules/mcp/linker.ld" \
  "$CARGO" build --release -Z build-std=core,alloc \
    -Z build-std-features=compiler-builtins-mem -Zjson-target-spec \
    --target "$TARGET_JSON" -p mcp-module
"$LLVM_OBJCOPY" -O binary \
  "$CARGO_TARGET_DIR/$TARGET/release/mcp-module" "$WORK/mcp.module"
module_size="$(stat -c%s "$WORK/mcp.module")"
if [ "$module_size" -gt 2097152 ]; then
  echo "m2-rig: module is $module_size bytes, over the 2 MiB slot" >&2
  exit 1
fi
echo "m2-rig: module $module_size bytes"

(
  cd "$WORK"
  sha256sum irix7-rom.bin mcp.module >SHA256SUMS
  cat SHA256SUMS
)
