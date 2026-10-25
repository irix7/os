#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the synthetic o32 probe, rewrite e_flags to EF_MIPS_ABI_O32 and pad it
# to the 256 KiB target-2 image.  This is a loader/syscall personality probe,
# not an IRIX 6.5 application.

set -eu

REPO=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT=${1:-$REPO/target/o32-hello}
TARGET="$REPO/targets/mips3-sgi-irix7-native-n32.json"
TARGET_DIR=${CARGO_TARGET_DIR:-$REPO/target/o32-hello-target}

mkdir -p "$OUT" "$TARGET_DIR"
(
    cd "$REPO/user/o32-hello"
    RUSTFLAGS="-Clink-arg=-T$REPO/user/o32-hello/linker.ld -Clink-arg=-e_start" \
        CARGO_TARGET_DIR="$TARGET_DIR" \
        cargo build --release -Zbuild-std=core -Zjson-target-spec --target "$TARGET"
)

cp "$TARGET_DIR/mips3-sgi-irix7-native-n32/release/o32-hello" "$OUT/o32-hello.elf"
python3 - "$OUT/o32-hello.elf" <<'PY'
import struct
import sys

path = sys.argv[1]
data = bytearray(open(path, "rb").read())
flags = struct.unpack_from(">I", data, 0x24)[0]
# Keep the MIPS architecture and noreorder bits, clear the n32/o64 ABI bits,
# and select o32.
flags = (flags & ~0xF020) | 0x1000
struct.pack_into(">I", data, 0x24, flags)
open(path, "wb").write(data)
PY
cp "$OUT/o32-hello.elf" "$OUT/o32-hello.raw"
size=$(stat -c%s "$OUT/o32-hello.raw")
if [ "$size" -gt 262144 ]; then
    echo "o32 ELF is $size bytes; the target-2 image limit is 262144" >&2
    exit 1
fi
truncate -s 262144 "$OUT/o32-hello.raw"
echo "ELF:  $OUT/o32-hello.elf"
echo "disk: $OUT/o32-hello.raw"
