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

# A 2 MiB raw disk for the virtio-blk self-test: sector 1 carries the marker the
# kernel reads back, and the rest is zero.  Recreated on every run.
DISK="$WORK/disk.img"
dd if=/dev/zero of="$DISK" bs=512 count=16384 2>/dev/null
printf 'IRIX7-X86-DISK-OK' | dd of="$DISK" bs=512 seek=1 conv=notrunc 2>/dev/null
# The machine's boot configuration: key=value lines in sectors 100-103.
printf 'test.key=config-ok\n' | dd of="$DISK" bs=512 seek=100 conv=notrunc 2>/dev/null

# The MCP control-plane module, linked for the x86 window, and its header on
# sector 8: magic, length and SHA-256 (see kernel-x86/src/module.rs).
MODULE_TARGET="$WORK/module-target"
CARGO_TARGET_X86_64_UNKNOWN_NONE_RUSTFLAGS="-Clink-arg=-T$REPO/modules/mcp/linker-x86.ld -Crelocation-model=static" \
  CARGO_TARGET_DIR="$MODULE_TARGET" \
  "$CARGO" build --release -Z build-std=core,alloc \
    -Z build-std-features=compiler-builtins-mem \
    --target x86_64-unknown-none -p mcp-module
OBJCOPY="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin/llvm-objcopy"
MODULE="$WORK/mcp-x86.module"
"$OBJCOPY" -O binary "$MODULE_TARGET/x86_64-unknown-none/release/mcp-module" "$MODULE"
WASM="$WORK/wasm-x86.module"
CARGO_TARGET_X86_64_UNKNOWN_NONE_RUSTFLAGS="-Clink-arg=-T$REPO/modules/wasm/linker-x86.ld -Crelocation-model=static" \
  CARGO_TARGET_DIR="$MODULE_TARGET" \
  "$CARGO" build --release -Z build-std=core,alloc \
    -Z build-std-features=compiler-builtins-mem \
    --target x86_64-unknown-none -p wasm-module
"$OBJCOPY" -O binary "$MODULE_TARGET/x86_64-unknown-none/release/wasm-module" "$WASM"
# Autoload fixture: the host serves the module, and the guest reaches it at the
# user-network gateway, 10.0.2.2.  Sector 9 names the URL.
FIXTURE_DIR="$WORK/fixture"
mkdir -p "$FIXTURE_DIR"
cp "$MODULE" "$FIXTURE_DIR/mcp-x86.module"
cp "$WASM" "$FIXTURE_DIR/wasm-x86.module"
FIXTURE_PORT="$(python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
(cd "$FIXTURE_DIR" && exec python3 -m http.server "$FIXTURE_PORT" --bind 127.0.0.1) >/dev/null 2>&1 &
FIXTURE_PID=$!
printf '10.0.2.2:%s/mcp-x86.module' "$FIXTURE_PORT" | dd of="$DISK" bs=512 seek=9 conv=notrunc 2>/dev/null
printf '10.0.2.2:%s/wasm-x86.module' "$FIXTURE_PORT" | dd of="$DISK" bs=512 seek=11 conv=notrunc 2>/dev/null
python3 -I - "$DISK" "$MODULE" 8 <<'PY'
import hashlib, struct, sys
disk, module, header_sector = sys.argv[1], sys.argv[2], int(sys.argv[3])
data = open(module, "rb").read()
header = b"IRIX7MOD" + struct.pack("<Q", len(data)) + hashlib.sha256(data).digest()
with open(disk, "r+b") as f:
    f.seek(header_sector * 512)
    f.write(header)
PY
python3 -I - "$DISK" "$WASM" 10 <<'PY'
import hashlib, struct, sys
disk, module, header_sector = sys.argv[1], sys.argv[2], int(sys.argv[3])
data = open(module, "rb").read()
header = b"IRIX7MOD" + struct.pack("<Q", len(data)) + hashlib.sha256(data).digest()
with open(disk, "r+b") as f:
    f.seek(header_sector * 512)
    f.write(header)
PY
# Autoload fixture: the host serves the module, and the guest reaches it at the
# user-network gateway, 10.0.2.2.  Sector 9 names the URL.
FIXTURE_DIR="$WORK/fixture"
mkdir -p "$FIXTURE_DIR"
cp "$MODULE" "$FIXTURE_DIR/mcp-x86.module"
cp "$WASM" "$FIXTURE_DIR/wasm-x86.module"
FIXTURE_PORT="$(python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
(cd "$FIXTURE_DIR" && exec python3 -m http.server "$FIXTURE_PORT" --bind 127.0.0.1) >/dev/null 2>&1 &
FIXTURE_PID=$!
printf '10.0.2.2:%s/mcp-x86.module' "$FIXTURE_PORT" | dd of="$DISK" bs=512 seek=9 conv=notrunc 2>/dev/null
printf '10.0.2.2:%s/wasm-x86.module' "$FIXTURE_PORT" | dd of="$DISK" bs=512 seek=11 conv=notrunc 2>/dev/null
rm -f "$SERIAL"

# The guest serves the MCP module on port 80.  Forward a free host port to it,
# so the gate can probe the HTTP server while the machine is running.
HTTP_PORT="${X86_HTTP_PORT:-$(python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')}"
set +e
timeout 150 "$QEMU" \
  -M q35 \
  -m "$MEMORY" \
  -kernel "$KERNEL" \
  -display none \
  -serial "file:$SERIAL" \
  -monitor none \
  -no-reboot \
  -device isa-debug-exit,iobase=0x501,iosize=0x04 \
  -netdev "user,id=net0,hostfwd=tcp:127.0.0.1:$HTTP_PORT-:80" \
  -device virtio-net-pci,netdev=net0,disable-modern=on \
  -drive file="$DISK",if=none,id=disk0,format=raw \
  -device virtio-blk-pci,drive=disk0,disable-modern=on &
QEMU_PID=$!

# Wait for the module to start listening, then probe until it answers.
deadline=$((SECONDS + 90))
until grep -qF "mcp: listening on port 80" "$SERIAL" 2>/dev/null \
  || [ "$SECONDS" -ge "$deadline" ] || ! kill -0 "$QEMU_PID" 2>/dev/null; do
  sleep 0.5
done
http_status=""
for _ in $(seq 1 60); do
  http_status="$(curl -s -m 3 -i "http://127.0.0.1:$HTTP_PORT/" 2>/dev/null | head -1 | tr -d '\r' || true)"
  case "$http_status" in HTTP/1.*) break ;; esac
  sleep 1
done
wait "$QEMU_PID"
status=$?
kill "$FIXTURE_PID" 2>/dev/null || true
set -e

echo "x86-qemu: http probe: ${http_status:-no response}"
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
  "x86: blk capacity="
  "x86: blk read sector 1=IRIX7-X86-DISK-OK"
  "x86: blk write readback ok"
  "x86: config test.key=config-ok"
  "x86: block service read/write ok"
  "x86: autoload mcp fetched "
  "x86: autoload mcp sha256 ok"
  "x86: autoload wasm fetched "
  "x86: autoload wasm sha256 ok"
  "mcp: entry status=0"
  "wasm: entry status=0"
  "wasm: run=42"
  "m1-wasm: "
  "x86: module serving"
  "mcp: listening on port 80"
  "x86: deliberate fault: int3"
  "x86: exception vector=3 (breakpoint)"
  "x86: halted"
)
case "$http_status" in
  HTTP/1.*) ;;
  *) echo "x86-qemu: the module did not answer on port 80" >&2; exit 1 ;;
esac
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
