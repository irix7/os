#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the PowerPC G3 Beige machine image and boot it under QEMU.
#
# The gate is marker-based: OpenBIOS enters the raw image at 0x01000000, the
# kernel reads the memory map and the ESCC address from the device tree,
# takes ten decrementer ticks through the 0x900 vector, and halts.  QEMU's
# g3beige machine has no debug-exit device, so the gate stops QEMU once
# BOOT-OK reaches the serial file.
#
# Usage:
#   tools/powerpc-qemu.sh [WORK_DIR]
#
# Environment:
#   CARGO          cargo binary (default: cargo)
#   QEMU           qemu-system-ppc binary (default: qemu-system-ppc)
#   QEMU_MEMORY    guest RAM (default: 64M)
#   PPC_IMAGE      boot this raw image instead of building one

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
WORK="${1:-${PPC_QEMU_WORK:-$REPO/target/powerpc-qemu}}"
CARGO="${CARGO:-cargo}"
QEMU="${QEMU:-qemu-system-ppc}"
MEMORY="${QEMU_MEMORY:-64M}"

mkdir -p "$WORK"

if [ -n "${PPC_IMAGE:-}" ]; then
  IMAGE="$PPC_IMAGE"
else
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$WORK/target}"
  export CARGO_TARGET_POWERPC_UNKNOWN_IRIX7_PPC_RUSTFLAGS="-Clink-arg=-T$REPO/kernel-ppc/linker.ld -Crelocation-model=static"
  export CARGO_PROFILE_RELEASE_OPT_LEVEL="${CARGO_PROFILE_RELEASE_OPT_LEVEL:-2}"

  "$CARGO" build --release \
    -Z json-target-spec \
    -Z build-std=core \
    -Z build-std-features=compiler-builtins-mem \
    --target "$REPO/targets/powerpc-unknown-irix7-ppc.json" \
    -p kernel-ppc --features cross

  ELF="$CARGO_TARGET_DIR/powerpc-unknown-irix7-ppc/release/kernel-ppc"
  OBJCOPY="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin/llvm-objcopy"
  # The target keeps the hard-float ABI, but the kernel has no FPR
  # save/restore yet.  Refuse an image that uses the FPU rather than let an
  # accidental f32/f64 corrupt a thread's state later.
  OBJDUMP="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin/llvm-objdump"
  if "$OBJDUMP" -d "$ELF" \
    | grep -qE $'^ *[0-9a-f]+:.*\t(f[a-z]|lfd|lfs|stfd|stfs|mffs|mtfs)'; then
    echo "powerpc-qemu: the kernel image uses an FP instruction" >&2
    exit 1
  fi

  IMAGE="$WORK/kernel-ppc.bin"
  "$OBJCOPY" -O binary "$ELF" "$IMAGE"
fi

SERIAL="$WORK/serial.log"
rm -f "$SERIAL"

set +e
"$QEMU" \
  -M g3beige \
  -m "$MEMORY" \
  -kernel "$IMAGE" \
  -display none \
  -serial "file:$SERIAL" \
  -monitor none \
  -no-reboot &
QEMU_PID=$!

deadline=$((SECONDS + 60))
while kill -0 "$QEMU_PID" 2>/dev/null; do
  if grep -qF "BOOT-OK" "$SERIAL" 2>/dev/null; then
    break
  fi
  if [ "$SECONDS" -ge "$deadline" ]; then
    break
  fi
  sleep 0.1
done
kill "$QEMU_PID" 2>/dev/null
wait "$QEMU_PID" 2>/dev/null
set -e

echo "--- serial ---"
cat "$SERIAL" 2>/dev/null || true
echo "--------------"

markers=(
  "IRIX 7 PowerPC G3 bring-up"
  "ppc: booted"
  "ppc: of client=0x"
  "ppc: escc=0x"
  "ppc: memory base=0x"
  "ppc: timebase="
  "ppc: decrementer ticks="
  "ppc: halted"
  "BOOT-OK"
)
for marker in "${markers[@]}"; do
  if ! grep -qF -- "$marker" "$SERIAL"; then
    echo "powerpc-qemu: missing marker: $marker" >&2
    exit 1
  fi
done

ticks="$(sed -n 's/^ppc: decrementer ticks=\([0-9][0-9]*\).*$/\1/p' "$SERIAL" | tr -d '\r')"
if [ -z "$ticks" ] || [ "$ticks" -lt 10 ]; then
  echo "powerpc-qemu: implausible decrementer tick count: '$ticks'" >&2
  exit 1
fi

ram_mib="$(sed -n 's/^ppc: memory base=0x[0-9a-f]* size=\([0-9][0-9]*\) MiB.*$/\1/p' "$SERIAL" | tr -d '\r')"
expected_mib="${MEMORY%[KkMmGg]}"
case "$MEMORY" in
  *[Kk]) expected_mib=$((expected_mib / 1024)) ;;
  *[Gg]) expected_mib=$((expected_mib * 1024)) ;;
esac
if [ -z "$ram_mib" ] || [ "$ram_mib" -ne "$expected_mib" ]; then
  echo "powerpc-qemu: memory size '$ram_mib' MiB does not match $MEMORY" >&2
  exit 1
fi

echo "powerpc-qemu: ok (${ram_mib} MiB RAM, ${ticks} decrementer ticks)"
