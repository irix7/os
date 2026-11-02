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
# one system call round-trip.  Finally it programs the CUDA timer and takes
# ten external interrupts through the Heathrow controller at the 0x500
# vector, then identifies the ATA disk on the first Mac I/O channel, reads the
# signature sector at LBA 4 with polled PIO, and loads the flat PowerPC module
# from the disk.  The module's SHA-256 is checked before the entry runs; a
# second boot with one corrupted image byte must refuse it.  QEMU's g3beige
# machine has no debug-exit device, so the gate stops QEMU once the final
# `ppc: halted` reaches the serial file.
#
# Usage:
#   tools/powerpc-qemu.sh [WORK_DIR]
#
# Environment:
#   CARGO          cargo binary (default: cargo)
#   QEMU           qemu-system-ppc binary (default: qemu-system-ppc)
#   QEMU_MEMORY    guest RAM (default: 64M)
#   PPC_IMAGE      boot this raw image instead of building one
#   PPC_DISK       attach this raw disk instead of creating the signature and
#                  module disk; it must carry the signature at LBA 4, the
#                  module header at LBA 6 and the image at LBA 16

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
WORK="${1:-${PPC_QEMU_WORK:-$REPO/target/powerpc-qemu}}"
CARGO="${CARGO:-cargo}"
QEMU="${QEMU:-qemu-system-ppc}"
MEMORY="${QEMU_MEMORY:-64M}"
SIGNATURE_BYTES="IRIX7-ATA-SIG001"
SIGNATURE_LBA=4
MODULE_HEADER_LBA=6
MODULE_IMAGE_LBA=16
# 4 MiB: the signature sector, the module header and a full 2 MiB image fit.
DISK_IMAGE_SECTORS=8192

HOST="$(rustc -vV | sed -n 's/^host: //p')"
LLVM_BIN="$(rustc --print sysroot)/lib/rustlib/$HOST/bin"
OBJCOPY="$LLVM_BIN/llvm-objcopy"
OBJDUMP="$LLVM_BIN/llvm-objdump"

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
  # The target keeps the hard-float ABI, but the kernel has no FPR
  # save/restore yet.  Refuse an image that uses the FPU rather than let an
  # accidental f32/f64 corrupt a thread's state later.
  if "$OBJDUMP" -d "$ELF" \
    | grep -qE $'^ *[0-9a-f]+:.*\t(f[a-z]|lfd|lfs|stfd|stfs|mffs|mtfs)'; then
    echo "powerpc-qemu: the kernel image uses an FP instruction" >&2
    exit 1
  fi

  IMAGE="$WORK/kernel-ppc.bin"
  "$OBJCOPY" -O binary "$ELF" "$IMAGE"
fi

# The same `modules/hello` source as the Indy machine, linked at the PowerPC
# slot.  The module is loaded from the disk, so the gate checks its digest and
# marker rather than trusting the build directory.
MODULE_TARGET="$WORK/module-target"
CARGO_TARGET_POWERPC_UNKNOWN_IRIX7_PPC_RUSTFLAGS="-Clink-arg=-T$REPO/modules/hello/linker-ppc.ld -Crelocation-model=static" \
CARGO_TARGET_DIR="$MODULE_TARGET" \
  "$CARGO" build --release \
    -Z json-target-spec \
    -Z build-std=core \
    -Z build-std-features=compiler-builtins-mem \
    --target "$REPO/targets/powerpc-unknown-irix7-ppc.json" \
    -p hello-module

MODULE_ELF="$MODULE_TARGET/powerpc-unknown-irix7-ppc/release/hello-module"
MODULE="$WORK/hello-ppc.module"
"$OBJCOPY" -O binary "$MODULE_ELF" "$MODULE"
MODULE_SIZE="$(stat -c%s "$MODULE")"
MODULE_SHA="$(sha256sum "$MODULE" | cut -d' ' -f1)"
if [ "$MODULE_SIZE" -gt $((2 * 1024 * 1024)) ]; then
  echo "powerpc-qemu: module image $MODULE_SIZE bytes exceeds the 2 MiB slot" >&2
  exit 1
fi
if "$OBJDUMP" -d "$MODULE_ELF" \
  | grep -qE $'^ *[0-9a-f]+:.*\t(f[a-z]|lfd|lfs|stfd|stfs|mffs|mtfs)'; then
  echo "powerpc-qemu: the module image uses an FP instruction" >&2
  exit 1
fi

SERIAL="$WORK/serial.log"
rm -f "$SERIAL"

DISK="${PPC_DISK:-$WORK/ata.img}"
if [ -z "${PPC_DISK:-}" ]; then
  rm -f "$DISK"
  dd if=/dev/zero of="$DISK" bs=512 count="$DISK_IMAGE_SECTORS" status=none
  printf '%s' "$SIGNATURE_BYTES" \
    | dd of="$DISK" bs=1 seek=$((SIGNATURE_LBA * 512)) conv=notrunc status=none
  # Header: `IRIX7MOD`, the big-endian image length and the SHA-256.  The image
  # follows at its own fixed LBA.
  python3 -I - "$DISK" "$MODULE" "$MODULE_HEADER_LBA" "$MODULE_IMAGE_LBA" <<'PY'
import hashlib, struct, sys
disk, module, header_lba, image_lba = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
data = open(module, "rb").read()
header = b"IRIX7MOD" + struct.pack(">I", len(data)) + hashlib.sha256(data).digest()
with open(disk, "r+b") as f:
    f.seek(header_lba * 512)
    f.write(header)
    f.seek(image_lba * 512)
    f.write(data)
PY
fi
DISK_SECTORS=$(( $(wc -c < "$DISK") / 512 ))
SIGNATURE="$(od -An -tx1 -N16 -j $((SIGNATURE_LBA * 512)) "$DISK" | tr -d ' \n')"
if [ -z "$SIGNATURE" ]; then
  echo "powerpc-qemu: could not read the signature sector from $DISK" >&2
  exit 1
fi

set +e
"$QEMU" \
  -M g3beige \
  -m "$MEMORY" \
  -kernel "$IMAGE" \
  -drive "file=$DISK,format=raw,if=ide,index=0,media=disk" \
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
  "ppc: mmu module base=0x1800000 size=2097152"
  "IRIX 7 PowerPC G3 bring-up"
  "ppc: booted"
  "ppc: of client=0x"
  "ppc: escc=0x"
  "ppc: memory base=0x"
  "ppc: timebase="
  "ppc: ata reg base=0x"
  "ppc: ata irq=0xd"
  "ppc: irq controller=0x"
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
  "ppc: irq vector=0x500 line=0x12 count=1"
  "ppc: irq pending=0x"
  "IRQ-OK count="
  "ppc: ata model=QEMU HARDDISK sectors="
  "ppc: ata lba=4 sector="
  "ATA-OK"
  "ppc: module length=${MODULE_SIZE} digest=${MODULE_SHA}"
  "PPC-MODULE-RUN"
  "module info: ppc: g3beige"
  "module tick"
  "ppc: module status=0 tick=1 length=${MODULE_SIZE}"
  "MODULE-OK"
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

# Every external interrupt reports the vector, the controller line, the
# count, and the interrupted SRR0/SRR1.  The counts must be contiguous from
# one, the saved MSR must have EE set, and the interrupted instruction must
# be in the image.
irq_lines="$(tr -d '\r' < "$SERIAL" \
  | grep -E '^ppc: irq vector=0x500 line=0x12 count=[0-9]+ srr0=0x[0-9a-f]+ srr1=0x[0-9a-f]+$')" || true
if [ -z "$irq_lines" ]; then
  echo "powerpc-qemu: no external-interrupt reports" >&2
  exit 1
fi

expected=1
while IFS= read -r irq; do
  count="$(printf '%s\n' "$irq" | sed -E 's/^.*count=([0-9]+) .*$/\1/')"
  irq_srr0="$(printf '%s\n' "$irq" | sed -E 's/^.*srr0=(0x[0-9a-f]+).*$/\1/')"
  irq_srr1="$(printf '%s\n' "$irq" | sed -E 's/^.*srr1=(0x[0-9a-f]+).*$/\1/')"
  if [ "$count" -ne "$expected" ]; then
    echo "powerpc-qemu: external interrupt count $count, expected $expected" >&2
    exit 1
  fi
  if [ $((irq_srr1 & 0x8000)) -eq 0 ]; then
    echo "powerpc-qemu: external interrupt SRR1 $irq_srr1 has EE clear" >&2
    exit 1
  fi
  check_image_srr0 "external interrupt" "$irq_srr0"
  expected=$((expected + 1))
done <<< "$irq_lines"
irq_count=$((expected - 1))
if [ "$irq_count" -lt 4 ]; then
  echo "powerpc-qemu: implausible external interrupt count: $irq_count" >&2
  exit 1
fi

irq_ok="$(tr -d '\r' < "$SERIAL" | sed -n 's/^IRQ-OK count=\([0-9][0-9]*\)$/\1/p')"
if [ -z "$irq_ok" ] || [ "$irq_ok" -ne "$irq_count" ]; then
  echo "powerpc-qemu: IRQ-OK count '$irq_ok' does not match the reports ($irq_count)" >&2
  exit 1
fi

irq_state="$(tr -d '\r' < "$SERIAL" \
  | grep -E '^ppc: irq pending=0x[0-9a-f]+ ifr=0x[0-9a-f]+$')" || true
if [ -z "$irq_state" ]; then
  echo "powerpc-qemu: missing external-interrupt controller state" >&2
  exit 1
fi
pending="$(printf '%s\n' "$irq_state" | sed -E 's/^ppc: irq pending=(0x[0-9a-f]+).*$/\1/')"
ifr="$(printf '%s\n' "$irq_state" | sed -E 's/^.*ifr=(0x[0-9a-f]+)$/\1/')"
if [ $((pending)) -ne 0 ] || [ $((ifr)) -ne 0 ]; then
  echo "powerpc-qemu: controller still has pending=$pending ifr=$ifr after EOI" >&2
  exit 1
fi

# The ATA register window comes from the `ata-3` node's `reg`, so it must sit
# inside the Mac I/O BAT the kernel printed.  The identify model, the sector
# count and the signature sector are checked against the disk QEMU attached.
io_range="$(tr -d '\r' < "$SERIAL" \
  | sed -n 's/^ppc: mmu io base=\(0x[0-9a-f]*\) size=\([0-9][0-9]*\)$/\1 \2/p' \
  | tail -n 1)" || true
if [ -z "$io_range" ]; then
  echo "powerpc-qemu: no Mac I/O BAT range in the serial log" >&2
  exit 1
fi
io_base=$(( ${io_range% *} ))
io_size=${io_range#* }

ata_range="$(tr -d '\r' < "$SERIAL" \
  | sed -n 's/^ppc: ata reg base=\(0x[0-9a-f]*\) size=\([0-9][0-9]*\)$/\1 \2/p' \
  | tail -n 1)" || true
if [ -z "$ata_range" ]; then
  echo "powerpc-qemu: no ATA register window in the serial log" >&2
  exit 1
fi
ata_base=$(( ${ata_range% *} ))
ata_size=${ata_range#* }
if [ "$ata_base" -lt "$io_base" ] \
  || [ $((ata_base + ata_size)) -gt $((io_base + io_size)) ]; then
  echo "powerpc-qemu: ATA window $ata_range is outside the Mac I/O BAT" >&2
  exit 1
fi

ata="$(tr -d '\r' < "$SERIAL" \
  | grep -E '^ppc: ata model=QEMU HARDDISK sectors=[0-9]+ lba48=[01]$' \
  | tail -n 1)" || true
if [ -z "$ata" ]; then
  echo "powerpc-qemu: missing or malformed ATA identify report" >&2
  exit 1
fi
ata_sectors="$(printf '%s\n' "$ata" | sed -E 's/^.*sectors=([0-9]+).*$/\1/')"
if [ "$ata_sectors" -ne "$DISK_SECTORS" ]; then
  echo "powerpc-qemu: ATA capacity $ata_sectors sectors, expected $DISK_SECTORS" >&2
  exit 1
fi

ata_sector="$(tr -d '\r' < "$SERIAL" \
  | sed -n 's/^ppc: ata lba=4 sector=\([0-9a-f]*\)$/\1/p' \
  | tail -n 1)" || true
if [ -z "$ata_sector" ]; then
  echo "powerpc-qemu: missing ATA signature-sector report" >&2
  exit 1
fi
if [ "$ata_sector" != "$SIGNATURE" ]; then
  echo "powerpc-qemu: ATA sector 4 is $ata_sector, expected $SIGNATURE" >&2
  exit 1
fi

module_report="$(tr -d '\r' < "$SERIAL" \
  | sed -n 's/^ppc: module length=\([0-9][0-9]*\) digest=\([0-9a-f][0-9a-f]*\)$/\1 \2/p' \
  | tail -n 1)" || true
if [ -z "$module_report" ]; then
  echo "powerpc-qemu: missing module digest report" >&2
  exit 1
fi
if [ "${module_report% *}" != "$MODULE_SIZE" ] \
  || [ "${module_report#* }" != "$MODULE_SHA" ]; then
  echo "powerpc-qemu: module report '$module_report' does not match $MODULE_SIZE bytes $MODULE_SHA" >&2
  exit 1
fi

# The same kernel must refuse an image whose digest does not match.  Flip one
# byte after the module entry and require the digest marker, no module output
# and no MODULE-OK.
BAD_DISK="$WORK/ata-bad.img"
BAD_SERIAL="$WORK/serial-bad.log"
cp "$DISK" "$BAD_DISK"
printf '\377' | dd of="$BAD_DISK" bs=1 \
  seek=$((MODULE_IMAGE_LBA * 512 + 32)) conv=notrunc status=none
rm -f "$BAD_SERIAL"
set +e
"$QEMU" \
  -M g3beige \
  -m "$MEMORY" \
  -kernel "$IMAGE" \
  -drive "file=$BAD_DISK,format=raw,if=ide,index=0,media=disk" \
  -display none \
  -serial "file:$BAD_SERIAL" \
  -monitor none \
  -no-reboot &
BAD_PID=$!
deadline=$((SECONDS + 60))
while kill -0 "$BAD_PID" 2>/dev/null; do
  if grep -qF "ppc: halted" "$BAD_SERIAL" 2>/dev/null; then
    break
  fi
  if [ "$SECONDS" -ge "$deadline" ]; then
    break
  fi
  sleep 0.1
done
kill "$BAD_PID" 2>/dev/null
wait "$BAD_PID" 2>/dev/null
set -e
if ! grep -qF "MODULE-DIGEST-BAD" "$BAD_SERIAL" 2>/dev/null; then
  echo "powerpc-qemu: the corrupted module was not refused" >&2
  cat "$BAD_SERIAL" 2>/dev/null || true
  exit 1
fi
if grep -qF "MODULE-OK" "$BAD_SERIAL" || grep -qF "PPC-MODULE-RUN" "$BAD_SERIAL"; then
  echo "powerpc-qemu: the corrupted module ran" >&2
  exit 1
fi

echo "powerpc-qemu: ok (${ram_mib} MiB RAM, ${ticks} decrementer ticks, ${irq_count} external interrupts, dsisr=$dsisr dar=$dar srr0=$srr0 isi_srr0=$isi_srr0, ata ${ata_sectors} sectors sector4=$ata_sector, module ${MODULE_SIZE} bytes ${MODULE_SHA})"
