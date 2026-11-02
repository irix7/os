#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the PowerPC G3 Beige machine image and boot it under QEMU.
#
# The gate is marker-based: OpenBIOS enters the raw image at 0x01000000, the
# kernel selects the Mac I/O node that carries the ESCC console and the ATA
# disk, installs its own BAT entries, points SDR1 at an empty table, takes ten
# decrementer ticks through the 0x900 vector, then proves the five exception
# paths it owns: a DSI, an ISI, an alignment fault, an illegal instruction and
# one system call round-trip.  Finally it programs the CUDA timer and takes
# ten external interrupts through the Heathrow controller at the 0x500
# vector, then identifies the ATA disk on the selected Mac I/O channel, reads
# the signature sector at LBA 4 with polled PIO, and loads the flat PowerPC module
# from the disk.  The module's SHA-256 is checked before the entry runs; a
# second boot with one corrupted image byte must refuse it.  The kernel
# initialises both ESCC channels and mirrors every byte to both, so QEMU
# captures the same kernel log on channel A and channel B.  It also maps the
# Open Firmware display aperture cache-inhibited, mirrors the boot log into an
# 8x16 text console, draws a block and a glyph, reads them back from the
# framebuffer in the guest and prints GFX-OK.  A second boot adds
# an extra `macio-oldworld` device ahead of the built-in controller in tree
# order and requires the kernel to keep selecting the node with the ESCC and
# the disk.  That boot has no keyboard input and must reach both bounded
# timeout markers.
#
# After the module check the kernel waits one second for a key on the selected
# ESCC channel.  Silence prints `ppc: monitor timeout`.  A pipe chardev on
# channel A in a separate boot carries `info`, `help` and `halt` at the `ppc>`
# prompt, then a pasted burst with a backspace edit, an overlong line and an
# unknown command; the gate checks the echo, the prompt and every reported
# value.  A final boot stops OpenBIOS auto-boot, names channel B as the output
# device and runs the monitor on channel B with a pasted burst.
#
# The primary boot also proves the ADB keyboard.  QEMU's g3beige has a CUDA
# and an ADB keyboard at the address OpenBIOS put in the device tree.  Once
# the kernel prints `ppc: keyboard waiting`, the gate sends `sendkey a` on a
# QEMU monitor socket and requires the decoded scancode, the ASCII character
# and `KEY-OK`.
#
# QEMU's g3beige machine has no debug-exit device, so the gate stops
# QEMU once the final `ppc: halted` reaches both serial files.
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

# The PMU timer path cannot run under QEMU: g3beige has a CUDA and QEMU has no
# PMU model.  Require that the image still links the via-pmu selection so the
# real M4753 check (#16) has something to run; this is a presence check, not a
# substitute for hardware evidence.
if ! grep -aqF "via-pmu" "$IMAGE"; then
  echo "powerpc-qemu: the image does not contain the via-pmu timer path" >&2
  exit 1
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
SERIAL_B="$WORK/serial-b.log"
rm -f "$SERIAL" "$SERIAL_B"

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
MONITOR="$WORK/monitor.sock"
rm -f "$MONITOR"

# Send one key through the QEMU monitor once the kernel is waiting.  The
# monitor is a unix socket because the serial ports already own the chardevs.
send_key() {
  python3 -I - "$MONITOR" <<'PY'
import socket, sys, time
path = sys.argv[1]
deadline = time.monotonic() + 5
while True:
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.connect(path)
        break
    except OSError:
        if time.monotonic() > deadline:
            sys.exit(1)
        time.sleep(0.05)
sock.settimeout(3)

def read_prompt():
    data = b""
    while b"(qemu)" not in data:
        try:
            chunk = sock.recv(4096)
        except OSError:
            return False
        if not chunk:
            return False
        data += chunk
    return True

# The greeting ends with the first prompt.  Wait for the second prompt after
# the command so a dropped monitor line fails the gate instead of the key.
if not read_prompt():
    sys.exit(1)
sock.sendall(b"sendkey a\n")
if not read_prompt():
    sys.exit(1)
sock.close()
PY
}

"$QEMU" \
  -M g3beige \
  -m "$MEMORY" \
  -kernel "$IMAGE" \
  -drive "file=$DISK,format=raw,if=ide,index=0,media=disk" \
  -nographic \
  -serial "file:$SERIAL" \
  -serial "file:$SERIAL_B" \
  -monitor "unix:$MONITOR,server=on,wait=off" \
  -no-reboot &
QEMU_PID=$!

key_sent=0
key_error=0
deadline=$((SECONDS + 60))
while kill -0 "$QEMU_PID" 2>/dev/null; do
  if [ "$key_sent" -eq 0 ] \
    && grep -qF "ppc: keyboard waiting" "$SERIAL" 2>/dev/null; then
    if send_key; then
      key_sent=1
    else
      key_error=1
      break
    fi
  fi
  if grep -qF "ppc: halted" "$SERIAL" 2>/dev/null \
    && grep -qF "ppc: halted" "$SERIAL_B" 2>/dev/null; then
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
if [ "$key_error" -ne 0 ]; then
  echo "powerpc-qemu: could not send a key through $MONITOR" >&2
  exit 1
fi
if [ "$key_sent" -ne 1 ]; then
  echo "powerpc-qemu: the kernel never reached the keyboard wait" >&2
  exit 1
fi

echo "--- serial (channel A) ---"
cat "$SERIAL" 2>/dev/null || true
echo "--- serial (channel B) ---"
cat "$SERIAL_B" 2>/dev/null || true
echo "--------------------------"

markers=(
  "ppc: mmu enabled sdr1=0x"
  "ppc: mmu image base=0x"
  "ppc: mmu io base=0x"
  "ppc: mmu io wimg=0x28"
  "ppc: mmu module base=0x1800000 size=2097152"
  "ppc: mmu fb base=0x"
  "ppc: mmu fb wimg=0x28"
  "ppc: display path=/pci"
  "IRIX 7 PowerPC G3 bring-up"
  "ppc: booted"
  "ppc: of client=0x"
  "ppc: macio base=0x"
  "ppc: macio path="
  "ppc: macio children escc="
  "ppc: escc=0x"
  "ppc: console output-device="
  "ppc: memory base=0x"
  "ppc: timebase="
  "ppc: timer transport=cuda via=0x"
  "ppc: ata reg base=0x"
  "ppc: ata irq=0xd"
  "ppc: adb transport=cuda via=0x"
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
  "ppc: framebuffer bytes="
  "GFX-OK"
  "ppc: module length=${MODULE_SIZE} digest=${MODULE_SHA}"
  "PPC-MODULE-RUN"
  "module info: ppc: g3beige"
  "module tick"
  "ppc: module status=0 tick=1 length=${MODULE_SIZE}"
  "MODULE-OK"
  "ppc: monitor wait input=a"
  "ppc: monitor timeout"
  "ppc: keyboard transport=cuda address="
  "ppc: keyboard reset=ok"
  "ppc: keyboard waiting"
  "ppc: adb key code=0x00 down=1 ascii=0x61 char=a"
  "KEY-OK"
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

key_line="$(tr -d '\r' < "$SERIAL" \
  | grep -E '^ppc: adb key code=0x[0-9a-f][0-9a-f] down=[01] ascii=0x[0-9a-f][0-9a-f] char=.$' \
  | tail -n 1)" || true
if [ -z "$key_line" ]; then
  echo "powerpc-qemu: missing or malformed keyboard report" >&2
  exit 1
fi
if [ "$key_line" != "ppc: adb key code=0x00 down=1 ascii=0x61 char=a" ]; then
  echo "powerpc-qemu: decoded key '$key_line' is not the injected 'a'" >&2
  exit 1
fi
if grep -qF "ppc: keyboard timeout" "$SERIAL"; then
  echo "powerpc-qemu: the keyboard timed out after the key was sent" >&2
  exit 1
fi

adb_line="$(tr -d '\r' < "$SERIAL" \
  | grep -E '^ppc: adb transport=cuda via=0x[0-9a-f]+ keyboard=[0-9]+$' \
  | tail -n 1)" || true
if [ -z "$adb_line" ]; then
  echo "powerpc-qemu: missing or malformed ADB transport report" >&2
  exit 1
fi
adb_keyboard="$(printf '%s\n' "$adb_line" | sed -E 's/^.* keyboard=([0-9]+)$/\1/')"
if [ "$adb_keyboard" -ne 8 ]; then
  echo "powerpc-qemu: keyboard address $adb_keyboard, expected the tree's 8" >&2
  exit 1
fi

# The selected Mac I/O node must be a PCI mac-io node with an ESCC and at
# least one ATA child.  g3beige has one candidate; the PowerBook has two and
# the same selection code scores them.
macio_line="$(tr -d '\r' < "$SERIAL" \
  | grep -E '^ppc: macio base=0x[0-9a-f]+ size=[0-9]+$' \
  | tail -n 1)" || true
if [ -z "$macio_line" ]; then
  echo "powerpc-qemu: missing Mac I/O window report" >&2
  exit 1
fi
macio_base=$(( $(printf '%s\n' "$macio_line" | sed -E 's/^ppc: macio base=(0x[0-9a-f]+) .*$/\1/') ))
macio_size=$(printf '%s\n' "$macio_line" | sed -E 's/^.* size=([0-9]+)$/\1/')

macio_path_line="$(tr -d '\r' < "$SERIAL" \
  | grep -E '^ppc: macio path=/[^ ]+$' \
  | tail -n 1)" || true
if [ -z "$macio_path_line" ]; then
  echo "powerpc-qemu: missing Mac I/O path report" >&2
  exit 1
fi
macio_path="${macio_path_line#ppc: macio path=}"
case "$macio_path" in
  /pci*) ;;
  *)
    echo "powerpc-qemu: selected Mac I/O path '$macio_path' is not under /pci" >&2
    exit 1
    ;;
esac
case "$macio_path" in
  *mac-io*) ;;
  *)
    echo "powerpc-qemu: selected Mac I/O path '$macio_path' is not a mac-io node" >&2
    exit 1
    ;;
esac

macio_children="$(tr -d '\r' < "$SERIAL" \
  | grep -E '^ppc: macio children escc=[0-9]+ ata=[0-9]+ disks=[0-9]+$' \
  | tail -n 1)" || true
if [ -z "$macio_children" ]; then
  echo "powerpc-qemu: missing Mac I/O child report" >&2
  exit 1
fi
escc_children=$(printf '%s\n' "$macio_children" | sed -E 's/^.*escc=([0-9]+) .*$/\1/')
ata_children=$(printf '%s\n' "$macio_children" | sed -E 's/^.*ata=([0-9]+) .*$/\1/')
if [ "$escc_children" -lt 1 ] || [ "$ata_children" -lt 1 ]; then
  echo "powerpc-qemu: selected Mac I/O node has escc=$escc_children ata=$ata_children" >&2
  exit 1
fi

# The ESCC line names the register window and both channel control ports.
# Channel B's control port is the window base; channel A's is 0x20 above it.
escc_line="$(tr -d '\r' < "$SERIAL" \
  | grep -E '^ppc: escc=0x[0-9a-f]+ ch-a=0x[0-9a-f]+ ch-b=0x[0-9a-f]+$' \
  | tail -n 1)" || true
if [ -z "$escc_line" ]; then
  echo "powerpc-qemu: missing or malformed ESCC channel report" >&2
  exit 1
fi
escc_base=$(( $(printf '%s\n' "$escc_line" | sed -E 's/^ppc: escc=(0x[0-9a-f]+) .*$/\1/') ))
escc_a=$(( $(printf '%s\n' "$escc_line" | sed -E 's/^.* ch-a=(0x[0-9a-f]+) .*$/\1/') ))
escc_b=$(( $(printf '%s\n' "$escc_line" | sed -E 's/^.* ch-b=(0x[0-9a-f]+)$/\1/') ))
if [ "$escc_b" -ne "$escc_base" ]; then
  echo "powerpc-qemu: channel B control 0x$(printf '%x' "$escc_b") is not the ESCC window" >&2
  exit 1
fi
if [ "$escc_a" -ne $((escc_base + 0x20)) ]; then
  echo "powerpc-qemu: channel A control 0x$(printf '%x' "$escc_a") is not window+0x20" >&2
  exit 1
fi
if [ "$escc_base" -lt "$macio_base" ] \
  || [ $((escc_base + 0x40)) -gt $((macio_base + macio_size)) ]; then
  echo "powerpc-qemu: ESCC window 0x$(printf '%x' "$escc_base") is outside the selected Mac I/O window" >&2
  exit 1
fi

# The firmware console property must name one of the two channels, and the
# kernel's selection must agree with it.  -nographic makes OpenBIOS publish
# the serial path instead of the screen.
console_line="$(tr -d '\r' < "$SERIAL" \
  | grep -E '^ppc: console output-device=.* stdout-path=.* selected=(a|b) mirror=a\+b$' \
  | tail -n 1)" || true
if [ -z "$console_line" ]; then
  echo "powerpc-qemu: the firmware console property did not select an ESCC channel" >&2
  exit 1
fi
console_path="$(printf '%s\n' "$console_line" | sed -E 's/^ppc: console output-device=([^ ]*) .*$/\1/')"
selected="$(printf '%s\n' "$console_line" | sed -E 's/^.* selected=([ab]) mirror=a\+b$/\1/')"
case "$console_path" in
  *ch-a*|scca|scca:*|ttya|ttya:*) expected=a ;;
  *ch-b*|sccb|sccb:*|ttyb|ttyb:*) expected=b ;;
  *)
    echo "powerpc-qemu: output-device '$console_path' names no ESCC channel" >&2
    exit 1
    ;;
esac
if [ "$selected" != "$expected" ]; then
  echo "powerpc-qemu: selected channel $selected does not match output-device '$console_path'" >&2
  exit 1
fi
case "$console_path" in
  "$macio_path"/*) ;;
  *)
    echo "powerpc-qemu: console '$console_path' is not below the selected Mac I/O node '$macio_path'" >&2
    exit 1
    ;;
esac

# The framebuffer console reads the display mode from the tree and maps the
# aperture with its own BAT.  The gate checks that the mode is one of the
# supported depths, that the stride matches the mode, and that the BAT covers
# the whole visible frame.  The guest's own readback of the drawn block and
# glyph is what proves the pixels; GFX-OK is printed only if that passed.
display_line="$(tr -d '\r' < "$SERIAL" \
  | grep -E '^ppc: display path=/[^ ]+ base=0x[0-9a-f]+ size=[0-9]+ width=[0-9]+ height=[0-9]+ depth=[0-9]+ linebytes=[0-9]+$' \
  | tail -n 1)" || true
if [ -z "$display_line" ]; then
  echo "powerpc-qemu: missing framebuffer display report" >&2
  exit 1
fi
display_base=$(( $(printf '%s\n' "$display_line" | sed -E 's/^.* base=(0x[0-9a-f]+) .*$/\1/') ))
display_size=$(printf '%s\n' "$display_line" | sed -E 's/^.* size=([0-9]+) .*$/\1/')
display_width=$(printf '%s\n' "$display_line" | sed -E 's/^.* width=([0-9]+) .*$/\1/')
display_height=$(printf '%s\n' "$display_line" | sed -E 's/^.* height=([0-9]+) .*$/\1/')
display_depth=$(printf '%s\n' "$display_line" | sed -E 's/^.* depth=([0-9]+) .*$/\1/')
display_linebytes=$(printf '%s\n' "$display_line" | sed -E 's/^.* linebytes=([0-9]+)$/\1/')
case "$display_depth" in
  8|16|32) ;;
  *)
    echo "powerpc-qemu: unsupported framebuffer depth $display_depth" >&2
    exit 1
    ;;
esac
expected_linebytes=$(( display_width * display_depth / 8 ))
if [ "$display_linebytes" -ne "$expected_linebytes" ]; then
  echo "powerpc-qemu: framebuffer stride $display_linebytes, expected $expected_linebytes for ${display_width}x${display_height}x${display_depth}" >&2
  exit 1
fi
frame_size=$(( display_linebytes * display_height ))
if [ "$display_size" -lt "$frame_size" ]; then
  echo "powerpc-qemu: framebuffer aperture $display_size is smaller than the ${frame_size}-byte frame" >&2
  exit 1
fi

fb_range="$(tr -d '\r' < "$SERIAL" \
  | sed -n 's/^ppc: mmu fb base=\(0x[0-9a-f]*\) size=\([0-9][0-9]*\)$/\1 \2/p' \
  | tail -n 1)" || true
if [ -z "$fb_range" ]; then
  echo "powerpc-qemu: no framebuffer BAT range in the serial log" >&2
  exit 1
fi
fb_base=$(( ${fb_range% *} ))
fb_size=${fb_range#* }
if [ "$fb_base" -gt "$display_base" ] \
  || [ $((fb_base + fb_size)) -lt $((display_base + frame_size)) ]; then
  echo "powerpc-qemu: framebuffer BAT $fb_range does not cover 0x$(printf '%x' "$display_base") + $frame_size" >&2
  exit 1
fi

# Both channels must carry exactly the same kernel output.  OpenBIOS writes
# its own banner to channel A only, so compare from the BAT switch onward.
kernel_a="$WORK/kernel-a.log"
kernel_b="$WORK/kernel-b.log"
sed -n '/^ppc: mmu enabled sdr1=0x/,$p' "$SERIAL" | tr -d '\r' > "$kernel_a"
tr -d '\r' < "$SERIAL_B" > "$kernel_b"
case "$(head -n 1 "$kernel_b")" in
  "ppc: mmu enabled sdr1=0x"*) ;;
  *)
    echo "powerpc-qemu: channel B did not start with the kernel banner" >&2
    exit 1
    ;;
esac
if ! cmp -s "$kernel_a" "$kernel_b"; then
  echo "powerpc-qemu: channel B is not a mirror of channel A" >&2
  diff -u "$kernel_a" "$kernel_b" 2>/dev/null | head -n 20 >&2 || true
  exit 1
fi

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
if [ "$ata_base" -lt "$macio_base" ] \
  || [ $((ata_base + ata_size)) -gt $((macio_base + macio_size)) ]; then
  echo "powerpc-qemu: ATA window $ata_range is outside the selected Mac I/O window" >&2
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

# Monitor boot.  Channel A is a pipe chardev so the gate can type at the
# console; the read side of the pipe is copied to the log.  The kernel prints
# `ppc: monitor wait input=a` and polls for one second.  Commands are sent one
# byte at a time because QEMU's ESCC model holds a single received character:
# a burst would lose every byte until the monitor drains the next one.
MONITOR_SERIAL="$WORK/serial-monitor.log"
MONITOR_SERIAL_B="$WORK/serial-monitor-b.log"
MONITOR_PIPE="$WORK/serial-monitor"
MONITOR_IN="$MONITOR_PIPE.in"
MONITOR_OUT="$MONITOR_PIPE.out"
rm -f "$MONITOR_SERIAL" "$MONITOR_SERIAL_B" "$MONITOR_IN" "$MONITOR_OUT"
mkfifo "$MONITOR_IN" "$MONITOR_OUT"
cat "$MONITOR_OUT" > "$MONITOR_SERIAL" &
MONITOR_DRAIN=$!
set +e
"$QEMU" \
  -M g3beige \
  -m "$MEMORY" \
  -kernel "$IMAGE" \
  -drive "file=$DISK,format=raw,if=ide,index=0,media=disk" \
  -nographic \
  -chardev "pipe,id=mon0,path=$MONITOR_PIPE" \
  -serial chardev:mon0 \
  -serial "file:$MONITOR_SERIAL_B" \
  -monitor none \
  -no-reboot &
MONITOR_PID=$!
set -e

stop_monitor() {
  kill "$MONITOR_PID" 2>/dev/null || true
  wait "$MONITOR_PID" 2>/dev/null || true
  kill "$MONITOR_DRAIN" 2>/dev/null || true
  wait "$MONITOR_DRAIN" 2>/dev/null || true
}

monitor_fail() {
  echo "powerpc-qemu: $*" >&2
  tr -d '\r' < "$MONITOR_SERIAL" 2>/dev/null | tail -n 40 >&2 || true
  stop_monitor
  exit 1
}

monitor_wait() {
  local pattern="$1"
  local seconds="${2:-30}"
  local deadline=$((SECONDS + seconds))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if grep -qF -- "$pattern" "$MONITOR_SERIAL" 2>/dev/null; then
      return 0
    fi
    sleep 0.05
  done
  return 1
}

monitor_send_line() {
  # One write for the whole line: the receive ring must retain a burst that
  # arrives while the previous command is still executing.
  printf '%s\r' "$1" > "$MONITOR_IN"
}

monitor_send_raw() {
  printf '%s' "$1" > "$MONITOR_IN"
}

monitor_wait "ppc: monitor wait" || monitor_fail "the monitor did not wait for a key"
monitor_send_line "info"
monitor_wait "monitor: module status=0 tick=1 length=$MODULE_SIZE" \
  || monitor_fail "the info command did not report the module"
monitor_send_line "help"
monitor_wait "monitor: halt    stop the machine" \
  || monitor_fail "the help command did not list halt"

# Paste a help command, a backspace-edited help command, an overlong line and
# an unknown command in one write.  The edited line is `help` only if the
# monitor erases the `x`; the overlong line must be refused, not truncated
# into a command.
overlong="$(printf 'a%.0s' $(seq 1 40))"
monitor_send_raw "help"$'\r'"helx"$'\x08'"p"$'\r'"$overlong"$'\r'"bogus"$'\r'
monitor_wait "monitor: command too long" \
  || monitor_fail "the overlong command was not refused"
monitor_wait "monitor: unknown command: bogus" \
  || monitor_fail "the unknown command was not reported"
monitor_send_line "halt"
monitor_wait "ppc: halted" || monitor_fail "the halt command did not stop the monitor"
stop_monitor

echo "--- monitor session ---"
tr -d '\r' < "$MONITOR_SERIAL" | sed -n '/^ppc: monitor wait/,$p'
echo "------------------------"

monitor_text="$(tr -d '\r' < "$MONITOR_SERIAL")"
prompts="$(printf '%s\n' "$monitor_text" | grep -c '^ppc> ')" || true
if [ -z "$prompts" ] || [ "$prompts" -lt 6 ]; then
  echo "powerpc-qemu: the monitor printed $prompts prompts, expected at least six" >&2
  exit 1
fi
for echo_line in "ppc> info" "ppc> help" "ppc> halt" "ppc> helx"; do
  if ! printf '%s\n' "$monitor_text" | grep -qF -- "$echo_line"; then
    echo "powerpc-qemu: the monitor did not echo '$echo_line'" >&2
    exit 1
  fi
done
help_count="$(printf '%s\n' "$monitor_text" | grep -cF "monitor: help    list commands")" || true
if [ -z "$help_count" ] || [ "$help_count" -lt 2 ]; then
  echo "powerpc-qemu: the backspace-edited command did not run help" >&2
  exit 1
fi
for help_line in \
  "monitor: help    list commands" \
  "monitor: info    machine selection and status" \
  "monitor: halt    stop the machine"; do
  if ! printf '%s\n' "$monitor_text" | grep -qF -- "$help_line"; then
    echo "powerpc-qemu: help did not print '$help_line'" >&2
    exit 1
  fi
done
if ! printf '%s\n' "$monitor_text" | grep -qF "monitor: command too long"; then
  echo "powerpc-qemu: the overlong command was not refused" >&2
  exit 1
fi
if ! printf '%s\n' "$monitor_text" | grep -qF "monitor: unknown command: bogus"; then
  echo "powerpc-qemu: the unknown command was not reported" >&2
  exit 1
fi

monitor_macio="$(printf '%s\n' "$monitor_text" \
  | grep -E '^monitor: macio base=0x[0-9a-f]+ size=[0-9]+ path=/[^ ]+ console=(a|b) input=(a|b)$' \
  | tail -n 1)" || true
if [ -z "$monitor_macio" ]; then
  echo "powerpc-qemu: the monitor did not report the Mac I/O selection" >&2
  exit 1
fi
mon_base=$(( $(printf '%s\n' "$monitor_macio" | sed -E 's/^monitor: macio base=(0x[0-9a-f]+) .*$/\1/') ))
mon_size=$(printf '%s\n' "$monitor_macio" | sed -E 's/^.* size=([0-9]+) path=.*$/\1/')
mon_path=$(printf '%s\n' "$monitor_macio" | sed -E 's/^.* path=([^ ]+) console=.*$/\1/')
mon_console=$(printf '%s\n' "$monitor_macio" | sed -E 's/^.* console=(a|b) input=(a|b)$/\1/')
mon_input=$(printf '%s\n' "$monitor_macio" | sed -E 's/^.* console=(a|b) input=(a|b)$/\2/')
if [ "$mon_base" -ne "$macio_base" ] || [ "$mon_size" -ne "$macio_size" ] \
  || [ "$mon_path" != "$macio_path" ]; then
  echo "powerpc-qemu: the monitor reports Mac I/O 0x$(printf '%x' "$mon_base") size=$mon_size path=$mon_path, expected 0x$(printf '%x' "$macio_base") size=$macio_size path=$macio_path" >&2
  exit 1
fi
if [ "$mon_console" != "$selected" ] || [ "$mon_input" != "$selected" ]; then
  echo "powerpc-qemu: the monitor console=$mon_console input=$mon_input, expected selected=$selected" >&2
  exit 1
fi

monitor_memory="$(printf '%s\n' "$monitor_text" \
  | sed -n 's/^monitor: memory base=0x[0-9a-f]* size=\([0-9][0-9]*\) MiB$/\1/p' \
  | tail -n 1)" || true
if [ -z "$monitor_memory" ] || [ "$monitor_memory" -ne "$ram_mib" ]; then
  echo "powerpc-qemu: the monitor reports '$monitor_memory' MiB, expected $ram_mib" >&2
  exit 1
fi

main_timebase="$(tr -d '\r' < "$SERIAL" \
  | sed -n 's/^ppc: timebase=\([0-9][0-9]*\) Hz$/\1/p' \
  | tail -n 1)" || true
monitor_timebase="$(printf '%s\n' "$monitor_text" \
  | sed -n 's/^monitor: timebase=\([0-9][0-9]*\) Hz$/\1/p' \
  | tail -n 1)" || true
if [ -z "$monitor_timebase" ] || [ "$monitor_timebase" != "$main_timebase" ]; then
  echo "powerpc-qemu: the monitor reports timebase '$monitor_timebase', expected $main_timebase" >&2
  exit 1
fi

monitor_ata="$(printf '%s\n' "$monitor_text" \
  | grep -E '^monitor: ata model=QEMU HARDDISK sectors=[0-9]+$' \
  | tail -n 1)" || true
if [ -z "$monitor_ata" ]; then
  echo "powerpc-qemu: the monitor did not report the ATA identify data" >&2
  exit 1
fi
monitor_ata_sectors="$(printf '%s\n' "$monitor_ata" | sed -E 's/^.*sectors=([0-9]+)$/\1/')"
if [ "$monitor_ata_sectors" -ne "$ata_sectors" ]; then
  echo "powerpc-qemu: the monitor reports $monitor_ata_sectors sectors, expected $ata_sectors" >&2
  exit 1
fi
if ! printf '%s\n' "$monitor_text" \
  | grep -qF "monitor: module status=0 tick=1 length=$MODULE_SIZE"; then
  echo "powerpc-qemu: the monitor did not report the loaded module" >&2
  exit 1
fi

# The monitor boot mirrors its whole session to channel B, including the
# prompt and the echoed commands.
monitor_a="$WORK/monitor-a.log"
monitor_b="$WORK/monitor-b.log"
sed -n '/^ppc: mmu enabled sdr1=0x/,$p' "$MONITOR_SERIAL" | tr -d '\r' > "$monitor_a"
tr -d '\r' < "$MONITOR_SERIAL_B" > "$monitor_b"
case "$(head -n 1 "$monitor_b")" in
  "ppc: mmu enabled sdr1=0x"*) ;;
  *)
    echo "powerpc-qemu: monitor channel B did not start with the kernel banner" >&2
    exit 1
    ;;
esac
if ! cmp -s "$monitor_a" "$monitor_b"; then
  echo "powerpc-qemu: monitor channel B is not a mirror of channel A" >&2
  diff -u "$monitor_a" "$monitor_b" 2>/dev/null | head -n 20 >&2 || true
  exit 1
fi

# B-selected console boot.  OpenBIOS publishes channel A under -nographic, so
# the gate stops auto-boot through the NVRAM seed, asks the firmware to name
# channel B, then boots the same kernel.  The monitor must select B and consume
# a pasted command burst from the channel B pipe.  Channel A stays attached so
# the OpenBIOS prompt can be driven; both kernel outputs are mirrored.
B_SERIAL="$WORK/serial-b-selected.log"
B_SERIAL_B="$WORK/serial-b-selected-b.log"
B_PIPE_A="$WORK/serial-b-a"
B_PIPE_B="$WORK/serial-b-b"
rm -f "$B_SERIAL" "$B_SERIAL_B" \
  "$B_PIPE_A.in" "$B_PIPE_A.out" "$B_PIPE_B.in" "$B_PIPE_B.out"
mkfifo "$B_PIPE_A.in" "$B_PIPE_A.out" "$B_PIPE_B.in" "$B_PIPE_B.out"
cat "$B_PIPE_A.out" > "$B_SERIAL" &
B_DRAIN_A=$!
cat "$B_PIPE_B.out" > "$B_SERIAL_B" &
B_DRAIN_B=$!
set +e
"$QEMU" \
  -M g3beige \
  -m "$MEMORY" \
  -kernel "$IMAGE" \
  -drive "file=$DISK,format=raw,if=ide,index=0,media=disk" \
  -nographic \
  -chardev "pipe,id=b-a,path=$B_PIPE_A" \
  -serial chardev:b-a \
  -chardev "pipe,id=b-b,path=$B_PIPE_B" \
  -serial chardev:b-b \
  -prom-env "auto-boot?=false" \
  -monitor none \
  -no-reboot &
B_PID=$!
set -e

stop_b() {
  kill "$B_PID" 2>/dev/null || true
  wait "$B_PID" 2>/dev/null || true
  kill "$B_DRAIN_A" "$B_DRAIN_B" 2>/dev/null || true
  wait "$B_DRAIN_A" "$B_DRAIN_B" 2>/dev/null || true
}

b_fail() {
  echo "powerpc-qemu: $*" >&2
  tr -d '\r' < "$B_SERIAL_B" 2>/dev/null | tail -n 40 >&2 || true
  stop_b
  exit 1
}

b_wait() {
  local file="$1"
  local pattern="$2"
  local seconds="${3:-30}"
  local deadline=$((SECONDS + seconds))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if grep -qF -- "$pattern" "$file" 2>/dev/null; then
      return 0
    fi
    sleep 0.05
  done
  return 1
}

b_wait "$B_SERIAL" "0 > " 30 || b_fail "OpenBIOS did not reach its prompt"
printf 'setenv output-device sccb\r' > "$B_PIPE_A.in"
b_wait "$B_SERIAL" "setenv output-device sccb  ok" 30 \
  || b_fail "OpenBIOS did not accept the channel B output device"
printf 'boot\r' > "$B_PIPE_A.in"
b_wait "$B_SERIAL_B" "ppc: monitor wait input=b" 30 \
  || b_fail "the kernel did not select channel B"
printf 'info\rhelp\rhalt\r' > "$B_PIPE_B.in"
b_wait "$B_SERIAL_B" "ppc: halted" 30 \
  || b_fail "the channel B monitor did not halt"
stop_b

b_console="$(tr -d '\r' < "$B_SERIAL_B" \
  | grep -E '^ppc: console output-device=sccb stdout-path=none selected=b mirror=a\+b$' \
  | tail -n 1)" || true
if [ -z "$b_console" ]; then
  echo "powerpc-qemu: the B-selected boot did not report selected=b" >&2
  exit 1
fi
for marker in "ppc: monitor wait input=b" "ppc: monitor ready" \
  "monitor: macio" "console=b input=b" "ppc> info" "ppc> help" "ppc> halt" \
  "monitor: help    list commands" "monitor: halt" "ppc: halted"; do
  if ! grep -qF -- "$marker" "$B_SERIAL_B"; then
    echo "powerpc-qemu: B-selected boot missing marker: $marker" >&2
    exit 1
  fi
done
b_macio="$(tr -d '\r' < "$B_SERIAL_B" \
  | grep -E '^monitor: macio base=0x[0-9a-f]+ size=[0-9]+ path=/[^ ]+ console=b input=b$' \
  | tail -n 1)" || true
if [ -z "$b_macio" ]; then
  echo "powerpc-qemu: the channel B monitor did not report console=b input=b" >&2
  exit 1
fi

echo "--- B-selected session ---"
tr -d '\r' < "$B_SERIAL_B" | sed -n '/^ppc: monitor wait/,$p'
echo "--------------------------"

# The no-input boot must never reach the prompt: the monitor wait expires and
# the machine halts with the same marker as before.
if grep -qF "ppc> " "$SERIAL"; then
  echo "powerpc-qemu: the no-input boot entered the monitor" >&2
  exit 1
fi

# Two controllers: the built-in Mac I/O is at PCI device 0x10 and carries the
# ESCC and the attached disk.  An extra macio-oldworld device takes device
# 0x03 and appears first in tree order with an ESCC but no disk.  The kernel
# must select the built-in node by its children, not the first /pci child.
TWO_SERIAL="$WORK/serial-two-controller.log"
TWO_SERIAL_B="$WORK/serial-two-controller-b.log"
rm -f "$TWO_SERIAL" "$TWO_SERIAL_B"
set +e
"$QEMU" \
  -M g3beige \
  -m "$MEMORY" \
  -kernel "$IMAGE" \
  -drive "file=$DISK,format=raw,if=ide,index=0,media=disk" \
  -device macio-oldworld \
  -nographic \
  -serial "file:$TWO_SERIAL" \
  -serial "file:$TWO_SERIAL_B" \
  -monitor none \
  -no-reboot &
TWO_PID=$!
deadline=$((SECONDS + 60))
while kill -0 "$TWO_PID" 2>/dev/null; do
  if grep -qF "ppc: halted" "$TWO_SERIAL" 2>/dev/null \
    && grep -qF "ppc: halted" "$TWO_SERIAL_B" 2>/dev/null; then
    break
  fi
  if [ "$SECONDS" -ge "$deadline" ]; then
    break
  fi
  sleep 0.1
done
kill "$TWO_PID" 2>/dev/null
wait "$TWO_PID" 2>/dev/null
set -e
two_path="$(tr -d '\r' < "$TWO_SERIAL" \
  | sed -n 's/^ppc: macio path=\(.*\)$/\1/p' \
  | tail -n 1)" || true
if [ "$two_path" != "$macio_path" ]; then
  echo "powerpc-qemu: selected '$two_path' with a second controller, expected '$macio_path'" >&2
  exit 1
fi
for marker in "ppc: escc=0x" "ATA-OK" "IRQ-OK count=" "GFX-OK" "MODULE-OK" \
  "ppc: monitor timeout" "ppc: keyboard timeout" "KEY-SKIP reason=no-key" \
  "ppc: halted"; do
  if ! grep -qF -- "$marker" "$TWO_SERIAL"; then
    echo "powerpc-qemu: two-controller boot missing marker: $marker" >&2
    exit 1
  fi
done
if ! grep -qF "MODULE-OK" "$TWO_SERIAL_B" \
  || ! grep -qF "ppc: halted" "$TWO_SERIAL_B"; then
  echo "powerpc-qemu: two-controller boot did not complete on channel B" >&2
  exit 1
fi

# The same kernel must refuse an image whose digest does not match.  Flip one
# byte after the module entry and require the digest marker, no module output
# and no MODULE-OK.
BAD_DISK="$WORK/ata-bad.img"
BAD_SERIAL="$WORK/serial-bad.log"
BAD_SERIAL_B="$WORK/serial-bad-b.log"
cp "$DISK" "$BAD_DISK"
printf '\377' | dd of="$BAD_DISK" bs=1 \
  seek=$((MODULE_IMAGE_LBA * 512 + 32)) conv=notrunc status=none
rm -f "$BAD_SERIAL" "$BAD_SERIAL_B"
set +e
"$QEMU" \
  -M g3beige \
  -m "$MEMORY" \
  -kernel "$IMAGE" \
  -drive "file=$BAD_DISK,format=raw,if=ide,index=0,media=disk" \
  -display none \
  -serial "file:$BAD_SERIAL" \
  -serial "file:$BAD_SERIAL_B" \
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
if ! grep -qF "GFX-OK" "$BAD_SERIAL"; then
  echo "powerpc-qemu: the framebuffer console did not run on the corrupted-module boot" >&2
  exit 1
fi

echo "powerpc-qemu: ok (${ram_mib} MiB RAM, ${ticks} decrementer ticks, ${irq_count} external interrupts, dsisr=$dsisr dar=$dar srr0=$srr0 isi_srr0=$isi_srr0, macio $macio_path escc=$escc_children ata=$ata_children, two-controller $two_path, ata ${ata_sectors} sectors sector4=$ata_sector, module ${MODULE_SIZE} bytes ${MODULE_SHA}, escc ch-a=0x$(printf '%x' "$escc_a") ch-b=0x$(printf '%x' "$escc_b") selected=$selected mirror=both, display ${display_width}x${display_height}x${display_depth} linebytes=$display_linebytes base=0x$(printf '%x' "$display_base") frame=$frame_size bat=$fb_range gfx=readback, monitor info/help/halt path=$mon_path console=$mon_console input=$mon_input timebase=$monitor_timebase burst=59b B-selected=sccb input=b, key '$key_line')"
