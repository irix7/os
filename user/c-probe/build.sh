#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the freestanding n32 C probe and wrap it as a 256 KiB SCSI target-2
# image.  The compiler is clang with the n32 big-endian MIPS backend; the
# linker is rust-lld from the pinned toolchain.  Neither glibc nor relibc is
# linked: the probe is the C oracle for the kernel ABI, not the production libc.
#
# Usage:
#   user/c-probe/build.sh [OUT_DIR]
#
# Environment:
#   CLANG  clang binary (default: clang)
#   LLD    rust-lld or ld.lld binary (default: rust-lld from rustc sysroot)

set -eu

REPO=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
OUT=${1:-$REPO/target/c-probe}
HERE=$REPO/user/c-probe
CLANG=${CLANG:-clang}
LLD=${LLD:-}

if ! command -v "$CLANG" >/dev/null 2>&1; then
    echo "build-c-probe: clang not found; set CLANG to a clang with the mips target" >&2
    exit 2
fi

if [ -z "$LLD" ]; then
    SYSROOT=$(rustc --print sysroot)
    LLD=$SYSROOT/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin/rust-lld
fi
if [ ! -x "$LLD" ]; then
    echo "build-c-probe: rust-lld not found at $LLD; set LLD" >&2
    exit 2
fi

mkdir -p "$OUT"
CFLAGS="--target=mips-linux-gnu -mabi=n32 -march=mips3 -msoft-float -EB \
  -nostdlib -ffreestanding -fno-pic -mno-abicalls -G0 -O2 \
  -fno-stack-protector -Wall -Wextra"

# shellcheck disable=SC2086
"$CLANG" $CFLAGS -c "$HERE/probe.c" -o "$OUT/probe.o"
# shellcheck disable=SC2086
"$CLANG" $CFLAGS -c "$HERE/startup.S" -o "$OUT/startup.o"
# shellcheck disable=SC2086
"$CLANG" $CFLAGS -c "$HERE/syscalls.S" -o "$OUT/syscalls.o"

"$LLD" -flavor gnu -m elf32btsmipn32 \
    -T "$HERE/linker.ld" \
    -o "$OUT/probe.elf" \
    "$OUT/startup.o" "$OUT/syscalls.o" "$OUT/probe.o"

cp "$OUT/probe.elf" "$OUT/probe.raw"
truncate -s 262144 "$OUT/probe.raw"

echo "c-probe ELF: $OUT/probe.elf"
echo "c-probe raw: $OUT/probe.raw"
