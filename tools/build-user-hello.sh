#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the native n32 user hello ELF and wrap it as a raw SCSI image for
# the telnet `userelf` command.
set -eu

REPO=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TARGET="$REPO/targets/mips3-sgi-irix7-native-n32.json"
OUT=${1:-$REPO/target/user-hello}
mkdir -p "$OUT"

export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$REPO/target/user-hello-target}
(
    cd "$REPO/user/hello"
    CARGO_TARGET_MIPS3_SGI_IRIX7_NATIVE_N32_RUSTFLAGS="-Clink-arg=-T$REPO/user/hello/linker.ld" \
        cargo build --release -Zbuild-std=core -Zjson-target-spec --target "$TARGET"
)
cp "$CARGO_TARGET_DIR/mips3-sgi-irix7-native-n32/release/hello" "$OUT/hello.elf"

# The loader expects the ELF at LBA 0 on SCSI target 2 and reads 256 KiB.
cp "$OUT/hello.elf" "$OUT/hello-disk.raw"
truncate -s 262144 "$OUT/hello-disk.raw"
echo "ELF:  $OUT/hello.elf"
echo "disk: $OUT/hello-disk.raw"
