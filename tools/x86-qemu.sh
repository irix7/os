#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the x86_64 machine image and boot it under QEMU's PVH path.
#
# The gate is deliberately marker-based: the image must reach the serial
# console, read a deterministic memory map from `hvm_start_info`, tick the PIT,
# and report a deliberate breakpoint through the IDT instead of hanging.  QEMU
# leaves through `isa-debug-exit`, so a hang shows up as a timeout rather than a
# stalled job.
#
# Usage:
#   tools/x86-qemu.sh [WORK_DIR]
#
# Environment:
#   CARGO        cargo binary (default: cargo)
#   QEMU         qemu-system-x86_64 binary (default: qemu-system-x86_64)
#   QEMU_MEMORY  guest RAM (default: 256M)
#   X86_IMAGE    boot this image instead of building one

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
WORK="${1:-${X86_QEMU_WORK:-$REPO/target/x86-qemu}}"
CARGO="${CARGO:-cargo}"
QEMU="${QEMU:-qemu-system-x86_64}"
MEMORY="${QEMU_MEMORY:-256M}"

mkdir -p "$WORK"
# `X86_IMAGE` boots a prebuilt image instead of building one: the release
# workflow uses it to check the exact bytes it is about to publish.
if [ -n "${X86_IMAGE:-}" ]; then
  KERNEL="$X86_IMAGE"
else
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$WORK/target}"
  export CARGO_TARGET_X86_64_UNKNOWN_NONE_RUSTFLAGS="-Clink-arg=-T$REPO/kernel-x86/linker.ld -Crelocation-model=static"
  export CARGO_PROFILE_RELEASE_OPT_LEVEL="${CARGO_PROFILE_RELEASE_OPT_LEVEL:-2}"

  "$CARGO" build --release -Z build-std=core,alloc \
    -Z build-std-features=compiler-builtins-mem \
    --target x86_64-unknown-none -p kernel-x86 --features cross

  KERNEL="$CARGO_TARGET_DIR/x86_64-unknown-none/release/kernel-x86"
fi
SERIAL="$WORK/serial.log"
rm -f "$SERIAL"

set +e
timeout 60 "$QEMU" \
  -M q35 \
  -m "$MEMORY" \
  -kernel "$KERNEL" \
  -display none \
  -serial "file:$SERIAL" \
  -monitor none \
  -no-reboot \
  -device isa-debug-exit,iobase=0x501,iosize=0x04 \
  -netdev user,id=net0 \
  -device virtio-net-pci,netdev=net0,disable-modern=on
status=$?
set -e

echo "--- serial ---"
cat "$SERIAL" 2>/dev/null || true
echo "--------------"

# `qemu_exit(0)` through isa-debug-exit encodes as (0 << 1) | 1.
if [ "$status" -ne 1 ]; then
  echo "x86-qemu: QEMU exited with status $status (expected 1; 124 means the guest hung)" >&2
  exit 1
fi

markers=(
  "x86: booted"
  "x86: pvh start_info=0x"
  "x86: pvh magic=0x336ec578 version=1"
  "x86: memmap "
  "x86: ram="
  "x86: timer enabled"
  "x86: timer ticks="
  "x86: frames total="
  "x86: frame alloc=0x"
  "x86: heap base="
  "x86: heap sum=499500"
  "x86: virtio-net mac="
  "x86: net ip=10.0.2.15"
  "x86: net gateway=10.0.2.2"
  "x86: deliberate fault: int3"
  "x86: exception vector=3 (breakpoint)"
  "x86: halted"
)
for marker in "${markers[@]}"; do
  if ! grep -qF -- "$marker" "$SERIAL"; then
    echo "x86-qemu: missing marker: $marker" >&2
    exit 1
  fi
done

# The memory map must sum to the configured RAM less the reserved holes: the
# usable total is not a fixed number across QEMU versions, but it must be
# non-zero and no larger than the machine.
ram_mib="$(sed -n 's/^x86: ram=\([0-9][0-9]*\) MiB.*$/\1/p' "$SERIAL" | tr -d '\r')"
if [ -z "$ram_mib" ] || [ "$ram_mib" -lt 128 ] || [ "$ram_mib" -gt 4096 ]; then
  echo "x86-qemu: implausible usable RAM: '$ram_mib' MiB" >&2
  exit 1
fi

echo "x86-qemu: ok (${ram_mib} MiB usable, exit $status)"
