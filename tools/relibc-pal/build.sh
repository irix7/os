#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the relibc-linked n32 C probe and wrap it as a 256 KiB SCSI target-2
# image.  The relibc source is the fork branch `irix7-pal` at the commit pinned
# below; the patch in patches/ is a review mirror, not the build input.
#
# Usage:
#   tools/relibc-pal/build.sh [OUT_DIR]
#
# Environment:
#   RELIBC_SRC       relibc checkout (default: /tmp/opencode/relibc-pal-src)
#   RELIBC_COMMIT    required commit (default: the pinned hash below)
#   RELIBC_REMOTE    clone URL used when the checkout is absent
#   CARGO            cargo binary (default: cargo; must select the repo's
#                    rust-toolchain.toml, nightly-2026-05-24)
#   MIPS_CC          n32 C compiler (default: mips64-buildroot-linux-uclibc-gcc
#                    from PATH, then the Bootlin 2026.08-1 tree if present)
#   MIPS_AR          matching ar
#   CARGO_TARGET_DIR build directory for the Rust static libraries
#
# The C compiler must be an n32 big-endian soft-float toolchain.  The pinned
# one is Bootlin mips64-n32--uclibc--stable-2026.08-1 (GCC 15.3.0, binutils
# 2.45.1).

set -eu

REPO=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
OUT=${1:-$REPO/target/relibc-pal}
SRC=${RELIBC_SRC:-/tmp/opencode/relibc-pal-src}
PINNED=083698215ca72eada079fa8ce95331ee4320444f
WANT=${RELIBC_COMMIT:-$PINNED}
REMOTE=${RELIBC_REMOTE:-https://github.com/irix7/relibc}
TARGET="$REPO/targets/mips3-sgi-irix7-native-n32.json"
PROBE="$REPO/user/relibc-probe"
THREAD_PROBE="$REPO/user/relibc-thread-probe"
FUTEX_PROBE="$REPO/user/relibc-futex-probe"
PTHREAD_PROBE="$REPO/user/relibc-pthread-probe"
SLEEP_PROBE="$REPO/user/relibc-sleep-probe"
SLEEP_CELL_PROBE="$REPO/user/relibc-sleep-cell-probe"
FORKEXEC_PROBE="$REPO/user/relibc-forkexec-probe"
FORKEXEC_CHILD="$REPO/user/relibc-forkexec-child"
LINKER="$PROBE/linker.ld"
CARGO=${CARGO:-cargo}
TARGET_DIR=${CARGO_TARGET_DIR:-$OUT/target}

find_mips_cc() {
    if [ -n "${MIPS_CC:-}" ]; then
        printf '%s\n' "$MIPS_CC"
        return
    fi
    if command -v mips64-buildroot-linux-uclibc-gcc >/dev/null 2>&1; then
        command -v mips64-buildroot-linux-uclibc-gcc
        return
    fi
    for candidate in \
        /tmp/opencode/relibc-n32/toolchain/mips64-n32--uclibc--stable-2026.08-1/bin/mips64-buildroot-linux-uclibc-gcc \
        /opt/mips64-n32--uclibc--stable-2026.08-1/bin/mips64-buildroot-linux-uclibc-gcc
    do
        if [ -x "$candidate" ]; then
            printf '%s\n' "$candidate"
            return
        fi
    done
    return 1
}

CC=$(find_mips_cc) || {
    echo "build-relibc-probe: no n32 C compiler found; set MIPS_CC" >&2
    exit 2
}
if [ -n "${MIPS_AR:-}" ]; then
    AR=$MIPS_AR
elif [ -x "$(dirname "$CC")/mips64-buildroot-linux-uclibc-ar" ]; then
    AR=$(dirname "$CC")/mips64-buildroot-linux-uclibc-ar
else
    echo "build-relibc-probe: no matching n32 ar found; set MIPS_AR" >&2
    exit 2
fi

if [ ! -e "$SRC/.git" ]; then
    echo "build-relibc-probe: cloning $REMOTE into $SRC"
    git clone --recurse-submodules --branch irix7-pal "$REMOTE" "$SRC"
fi
have=$(git -C "$SRC" rev-parse HEAD)
if [ "$have" != "$WANT" ]; then
    echo "build-relibc-probe: $SRC is at $have, expected $WANT" >&2
    echo "build-relibc-probe: fetch the branch and check it out, or set RELIBC_COMMIT" >&2
    exit 2
fi

mkdir -p "$OUT" "$OUT/lib"

cd "$SRC"
export CARGO_TARGET_DIR="$TARGET_DIR"
export CC_mips3_sgi_irix7_native_n32="$CC"
export AR_mips3_sgi_irix7_native_n32="$AR"
export CFLAGS_mips3_sgi_irix7_native_n32="-mno-abicalls -fno-pic -mabi=n32 -msoft-float -G0"

echo "build-relibc-probe: relibc $have"
echo "build-relibc-probe: cargo  $($CARGO --version)"
echo "build-relibc-probe: cc     $CC"

$CARGO build --release --no-default-features \
    --target "$TARGET" -Zjson-target-spec \
    -Z build-std=core,alloc,compiler_builtins

$CARGO build -p crt0 --release \
    --target "$TARGET" -Zjson-target-spec -Z build-std=core

LIB="$TARGET_DIR/mips3-sgi-irix7-native-n32/release"
cp "$LIB/librelibc.a" "$LIB/libcrt0.a" "$OUT/lib/"

CFLAGS="-mabi=n32 -march=mips3 -msoft-float -mno-abicalls -fno-pic -G0 \
  -ffunction-sections -fdata-sections -O2 -Wall -Wextra"
LDFLAGS="-mabi=n32 -march=mips3 -msoft-float -mno-abicalls -fno-pic \
  -nostdlib -no-pie -Wl,-e,_start -Wl,--gc-sections"

# shellcheck disable=SC2086
"$CC" $CFLAGS -c "$PROBE/probe.c" -o "$OUT/probe.o"
# shellcheck disable=SC2086
"$CC" $LDFLAGS -T "$LINKER" -o "$OUT/probe.elf" "$OUT/probe.o" \
    -Wl,--start-group "$OUT/lib/libcrt0.a" "$OUT/lib/librelibc.a" \
    -Wl,--end-group

# shellcheck disable=SC2086
"$CC" $CFLAGS -c "$THREAD_PROBE/probe.c" -o "$OUT/thread-probe.o"
# shellcheck disable=SC2086
"$CC" $CFLAGS -c "$THREAD_PROBE/native.S" -o "$OUT/thread-native.o"
# shellcheck disable=SC2086
"$CC" $LDFLAGS -T "$LINKER" -o "$OUT/thread-probe.elf" \
    "$OUT/thread-probe.o" "$OUT/thread-native.o" \
    -Wl,--start-group "$OUT/lib/libcrt0.a" "$OUT/lib/librelibc.a" \
    -Wl,--end-group

# shellcheck disable=SC2086
"$CC" $CFLAGS -c "$FUTEX_PROBE/probe.c" -o "$OUT/futex-probe.o"
# shellcheck disable=SC2086
"$CC" $CFLAGS -c "$FUTEX_PROBE/native.S" -o "$OUT/futex-native.o"
# shellcheck disable=SC2086
"$CC" $LDFLAGS -T "$LINKER" -o "$OUT/futex-probe.elf" \
    "$OUT/futex-probe.o" "$OUT/futex-native.o" \
    -Wl,--start-group "$OUT/lib/libcrt0.a" "$OUT/lib/librelibc.a" \
    -Wl,--end-group

# shellcheck disable=SC2086
"$CC" $CFLAGS -c "$PTHREAD_PROBE/probe.c" -o "$OUT/pthread-probe.o"
# shellcheck disable=SC2086
"$CC" $LDFLAGS -T "$LINKER" -o "$OUT/pthread-probe.elf" \
    "$OUT/pthread-probe.o" \
    -Wl,--start-group "$OUT/lib/libcrt0.a" "$OUT/lib/librelibc.a" \
    -Wl,--end-group

# shellcheck disable=SC2086
"$CC" $CFLAGS -c "$SLEEP_PROBE/probe.c" -o "$OUT/sleep-probe.o"
# shellcheck disable=SC2086
"$CC" $LDFLAGS -T "$LINKER" -o "$OUT/sleep-probe.elf" \
    "$OUT/sleep-probe.o" \
    -Wl,--start-group "$OUT/lib/libcrt0.a" "$OUT/lib/librelibc.a" \
    -Wl,--end-group

# shellcheck disable=SC2086
"$CC" $CFLAGS -c "$SLEEP_CELL_PROBE/probe.c" -o "$OUT/sleep-cell-probe.o"
# shellcheck disable=SC2086
"$CC" $LDFLAGS -T "$LINKER" -o "$OUT/sleep-cell-probe.elf" \
    "$OUT/sleep-cell-probe.o" \
    -Wl,--start-group "$OUT/lib/libcrt0.a" "$OUT/lib/librelibc.a" \
    -Wl,--end-group

# The fork/exec probe is two images: the parent at target 2 and the child at
# target 3.  Both use the same relibc and linker script.
# shellcheck disable=SC2086
"$CC" $CFLAGS -c "$FORKEXEC_PROBE/probe.c" -o "$OUT/forkexec-probe.o"
# shellcheck disable=SC2086
"$CC" $LDFLAGS -T "$LINKER" -o "$OUT/forkexec-probe.elf" \
    "$OUT/forkexec-probe.o" \
    -Wl,--start-group "$OUT/lib/libcrt0.a" "$OUT/lib/librelibc.a" \
    -Wl,--end-group
# shellcheck disable=SC2086
"$CC" $CFLAGS -c "$FORKEXEC_CHILD/probe.c" -o "$OUT/forkexec-child.o"
# shellcheck disable=SC2086
"$CC" $LDFLAGS -T "$LINKER" -o "$OUT/forkexec-child.elf" \
    "$OUT/forkexec-child.o" \
    -Wl,--start-group "$OUT/lib/libcrt0.a" "$OUT/lib/librelibc.a" \
    -Wl,--end-group

# The kernel's ELF loader reads a 256 KiB raw target-2 image.  The linked
# segment ends well inside the slot, but the unstripped symbol table pushes the
# file over it; strip before padding.
STRIP=$(dirname "$CC")/mips64-buildroot-linux-uclibc-strip
if [ ! -x "$STRIP" ]; then
    STRIP=$(command -v llvm-strip || command -v strip || true)
fi

finish_image() {
    elf=$1
    raw=$2
    if [ -n "$STRIP" ]; then
        "$STRIP" --strip-all "$elf" -o "$elf.stripped"
        mv "$elf.stripped" "$elf"
    fi
    size=$(stat -c%s "$elf")
    if [ "$size" -gt 262144 ]; then
        echo "build-relibc-probe: $elf is $size bytes; the image slot is 262144" >&2
        exit 1
    fi
    cp "$elf" "$raw"
    truncate -s 262144 "$raw"
    echo "$size"
}

probe_size=$(finish_image "$OUT/probe.elf" "$OUT/probe.raw")
thread_size=$(finish_image "$OUT/thread-probe.elf" "$OUT/thread-probe.raw")
futex_size=$(finish_image "$OUT/futex-probe.elf" "$OUT/futex-probe.raw")
pthread_size=$(finish_image "$OUT/pthread-probe.elf" "$OUT/pthread-probe.raw")
sleep_size=$(finish_image "$OUT/sleep-probe.elf" "$OUT/sleep-probe.raw")
sleep_cell_size=$(finish_image "$OUT/sleep-cell-probe.elf" "$OUT/sleep-cell-probe.raw")
forkexec_size=$(finish_image "$OUT/forkexec-probe.elf" "$OUT/forkexec-probe.raw")
forkexec_child_size=$(finish_image "$OUT/forkexec-child.elf" "$OUT/forkexec-child.raw")

{
    echo "relibc=$have"
    echo "cargo=$($CARGO --version)"
    echo "cc=$($CC --version | sed -n '1p')"
    echo "probe_elf=$probe_size bytes"
    echo "thread_probe_elf=$thread_size bytes"
    echo "futex_probe_elf=$futex_size bytes"
    echo "pthread_probe_elf=$pthread_size bytes"
    echo "sleep_probe_elf=$sleep_size bytes"
    echo "sleep_cell_probe_elf=$sleep_cell_size bytes"
    echo "forkexec_probe_elf=$forkexec_size bytes"
    echo "forkexec_child_elf=$forkexec_child_size bytes"
} >"$OUT/BUILD-PROVENANCE.txt"
sha256sum "$OUT/probe.elf" "$OUT/probe.raw" \
    "$OUT/thread-probe.elf" "$OUT/thread-probe.raw" \
    "$OUT/futex-probe.elf" "$OUT/futex-probe.raw" \
    "$OUT/pthread-probe.elf" "$OUT/pthread-probe.raw" \
    "$OUT/sleep-probe.elf" "$OUT/sleep-probe.raw" \
    "$OUT/sleep-cell-probe.elf" "$OUT/sleep-cell-probe.raw" \
    "$OUT/forkexec-probe.elf" "$OUT/forkexec-probe.raw" \
    "$OUT/forkexec-child.elf" "$OUT/forkexec-child.raw" >"$OUT/SHA256SUMS"

echo "build-relibc-probe: ELF $OUT/probe.elf"
echo "build-relibc-probe: raw $OUT/probe.raw"
echo "build-relibc-probe: thread ELF $OUT/thread-probe.elf"
echo "build-relibc-probe: thread raw $OUT/thread-probe.raw"
echo "build-relibc-probe: futex ELF $OUT/futex-probe.elf"
echo "build-relibc-probe: futex raw $OUT/futex-probe.raw"
echo "build-relibc-probe: pthread ELF $OUT/pthread-probe.elf"
echo "build-relibc-probe: pthread raw $OUT/pthread-probe.raw"
echo "build-relibc-probe: sleep ELF $OUT/sleep-probe.elf"
echo "build-relibc-probe: sleep raw $OUT/sleep-probe.raw"
echo "build-relibc-probe: sleep-cell ELF $OUT/sleep-cell-probe.elf"
echo "build-relibc-probe: sleep-cell raw $OUT/sleep-cell-probe.raw"
echo "build-relibc-probe: forkexec ELF $OUT/forkexec-probe.elf"
echo "build-relibc-probe: forkexec raw $OUT/forkexec-probe.raw"
echo "build-relibc-probe: forkexec child ELF $OUT/forkexec-child.elf"
echo "build-relibc-probe: forkexec child raw $OUT/forkexec-child.raw"
cat "$OUT/BUILD-PROVENANCE.txt"
