#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the PowerPC G3 Beige machine image and boot it under QEMU.
#
# The gate is marker-based: OpenBIOS enters the raw image at 0x01000000, the
# kernel reads the memory map and the ESCC address from the device tree,
# installs its own BAT entries, points SDR1 at an empty table, takes ten
# decrementer ticks through the 0x900 vector, then proves the five exception
# paths it owns: a DSI, an ISI, an alignment fault, an illegal instruction and
# one system call round-trip.  QEMU's g3beige machine has no debug-exit device,
# so the gate stops QEMU once the final `ppc: halted` reaches the serial file.
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
  if grep -qF "ppc: halted" "$SERIAL" 2>/dev/null; then
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
  "ppc: mmu enabled sdr1=0x"
  "ppc: mmu image base=0x"
  "ppc: mmu io base=0x"
  "ppc: mmu io wimg=0x28"
  "IRIX 7 PowerPC G3 bring-up"
  "ppc: booted"
  "ppc: of client=0x"
  "ppc: escc=0x"
  "ppc: memory base=0x"
  "ppc: timebase="
  "ppc: decrementer ticks="
  "BOOT-OK"
  "ppc: trap vector=0x300"
  "FAULT-OK"
  "ppc: trap vector=0x400"
  "ISI-OK"
  "ppc: trap vector=0x600"
  "ALIGN-OK"
  "ppc: trap vector=0x700"
  "PROGRAM-OK"
  "ppc: trap vector=0xc00"
  "SYSCALL-OK"
  "TRAPS-OK"
  "ppc: halted"
)

previous=0
for marker in "${markers[@]}"; do
  line="$(grep -nF -- "$marker" "$SERIAL" | head -n 1 | cut -d: -f1)" || true
  if [ -z "$line" ]; then
    echo "powerpc-qemu: missing marker: $marker" >&2
    exit 1
  fi
  if [ "$line" -le "$previous" ]; then
    echo "powerpc-qemu: marker out of order: $marker (line $line, previous $previous)" >&2
    exit 1
  fi
  previous="$line"
done

line_of() {
  grep -nF -- "$1" "$SERIAL" | head -n 1 | cut -d: -f1
}

mmu_line="$(line_of "ppc: mmu enabled sdr1=0x")" || true
banner_line="$(line_of "IRIX 7 PowerPC G3 bring-up")" || true
if [ -z "$mmu_line" ] || [ -z "$banner_line" ] || [ "$mmu_line" -ge "$banner_line" ]; then
  echo "powerpc-qemu: the banner was not printed after the BAT switch" >&2
  exit 1
fi

# One common report line per exception: the vector, SRR0 and SRR1, plus the
# memory-fault fields for DSI and alignment.
trap_line() {
  tr -d '\r' < "$SERIAL" \
    | grep -E "$1" \
    | tail -n 1
}

dsi="$(trap_line '^ppc: trap vector=0x300 srr0=0x[0-9a-f]+ srr1=0x[0-9a-f]+ dsisr=0x[0-9a-f]+ dar=0x[0-9a-f]+$')" || true
if [ -z "$dsi" ]; then
  echo "powerpc-qemu: missing or malformed DSI report" >&2
  exit 1
fi
dsisr="$(printf '%s\n' "$dsi" | sed -E 's/^.*dsisr=(0x[0-9a-f]+).*$/\1/')"
dar="$(printf '%s\n' "$dsi" | sed -E 's/^.*dar=(0x[0-9a-f]+).*$/\1/')"
srr0="$(printf '%s\n' "$dsi" | sed -E 's/^.*srr0=(0x[0-9a-f]+).*$/\1/')"
dsisr_value=$((dsisr))
dar_value=$((dar))
srr0_value=$((srr0))

if [ $((dsisr_value & 0x40000000)) -eq 0 ] \
  || [ $((dsisr_value & 0x02000000)) -ne 0 ]; then
  echo "powerpc-qemu: DSISR $dsisr is not a translation-not-found load" >&2
  exit 1
fi
if [ "$dar_value" -ne $((0x20000000)) ]; then
  echo "powerpc-qemu: DAR $dar is not the deliberate fault address" >&2
  exit 1
fi

isi="$(trap_line '^ppc: trap vector=0x400 srr0=0x[0-9a-f]+ srr1=0x[0-9a-f]+$')" || true
if [ -z "$isi" ]; then
  echo "powerpc-qemu: missing or malformed ISI report" >&2
  exit 1
fi
isi_srr0="$(printf '%s\n' "$isi" | sed -E 's/^.*srr0=(0x[0-9a-f]+).*$/\1/')"
if [ $((isi_srr0)) -ne $((0x20000000)) ]; then
  echo "powerpc-qemu: ISI SRR0 $isi_srr0 is not the deliberate fetch address" >&2
  exit 1
fi

alignment="$(trap_line '^ppc: trap vector=0x600 srr0=0x[0-9a-f]+ srr1=0x[0-9a-f]+ dsisr=0x[0-9a-f]+ dar=0x[0-9a-f]+$')" || true
if [ -z "$alignment" ]; then
  echo "powerpc-qemu: missing or malformed alignment report" >&2
  exit 1
fi
align_dar="$(printf '%s\n' "$alignment" | sed -E 's/^.*dar=(0x[0-9a-f]+).*$/\1/')"
align_srr0="$(printf '%s\n' "$alignment" | sed -E 's/^.*srr0=(0x[0-9a-f]+).*$/\1/')"
if [ $((align_dar & 3)) -eq 0 ]; then
  echo "powerpc-qemu: alignment DAR $align_dar is word aligned" >&2
  exit 1
fi

program="$(trap_line '^ppc: trap vector=0x700 srr0=0x[0-9a-f]+ srr1=0x[0-9a-f]+$')" || true
if [ -z "$program" ]; then
  echo "powerpc-qemu: missing or malformed program report" >&2
  exit 1
fi
program_srr0="$(printf '%s\n' "$program" | sed -E 's/^.*srr0=(0x[0-9a-f]+).*$/\1/')"
program_srr1="$(printf '%s\n' "$program" | sed -E 's/^.*srr1=(0x[0-9a-f]+).*$/\1/')"
if [ $((program_srr1 & 0x00080000)) -eq 0 ]; then
  echo "powerpc-qemu: program SRR1 $program_srr1 is not an illegal-instruction cause" >&2
  exit 1
fi

syscall_trap="$(trap_line '^ppc: trap vector=0xc00 srr0=0x[0-9a-f]+ srr1=0x[0-9a-f]+$')" || true
if [ -z "$syscall_trap" ]; then
  echo "powerpc-qemu: missing or malformed system-call report" >&2
  exit 1
fi
syscall_srr0="$(printf '%s\n' "$syscall_trap" | sed -E 's/^.*srr0=(0x[0-9a-f]+).*$/\1/')"

syscall="$(trap_line '^ppc: syscall call=0x2a result=0x2b$')" || true
if [ -z "$syscall" ]; then
  echo "powerpc-qemu: the system call did not return the known value" >&2
  exit 1
fi

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

image="$(tr -d '\r' < "$SERIAL" \
  | sed -n 's/^ppc: mmu image base=\(0x[0-9a-f]*\) size=\([0-9][0-9]*\)$/\1 \2/p' \
  | tail -n 1)" || true
if [ -z "$image" ]; then
  echo "powerpc-qemu: no image BAT range in the serial log" >&2
  exit 1
fi
image_base=$(( ${image% *} ))
image_end=$(( image_base + ${image#* } ))
if [ "$srr0_value" -lt "$image_base" ] || [ "$srr0_value" -ge "$image_end" ]; then
  echo "powerpc-qemu: DSI SRR0 $srr0 is outside the image BAT range" >&2
  exit 1
fi

check_image_srr0() {
  local name="$1"
  local address="$2"
  if [ $((address)) -lt "$image_base" ] || [ $((address)) -ge "$image_end" ]; then
    echo "powerpc-qemu: $name SRR0 $address is outside the image BAT range" >&2
    exit 1
  fi
}

check_image_srr0 alignment "$align_srr0"
check_image_srr0 program "$program_srr0"
check_image_srr0 "system call" "$syscall_srr0"

echo "powerpc-qemu: ok (${ram_mib} MiB RAM, ${ticks} decrementer ticks, dsisr=$dsisr dar=$dar srr0=$srr0 isi_srr0=$isi_srr0)"
