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
PINNED=d484536c49fed26a5377d2c511962b891e1c8dfb
WANT=${RELIBC_COMMIT:-$PINNED}
REMOTE=${RELIBC_REMOTE:-https://github.com/irix7/relibc}
TARGET="$REPO/targets/mips3-sgi-irix7-native-n32.json"
PROBE="$REPO/user/relibc-probe"
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

if [ ! -d "$SRC/.git" ]; then
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
"$CC" $LDFLAGS -T "$PROBE/linker.ld" -o "$OUT/probe.elf" "$OUT/probe.o" \
    -Wl,--start-group "$OUT/lib/libcrt0.a" "$OUT/lib/librelibc.a" \
    -Wl,--end-group

# The kernel's ELF loader reads a 256 KiB raw target-2 image.  The linked
# segment ends well inside the slot, but the unstripped symbol table pushes the
# file over it; strip before padding.
STRIP=$(dirname "$CC")/mips64-buildroot-linux-uclibc-strip
if [ ! -x "$STRIP" ]; then
    STRIP=$(command -v llvm-strip || command -v strip || true)
fi
if [ -n "$STRIP" ]; then
    "$STRIP" --strip-all "$OUT/probe.elf" -o "$OUT/probe.stripped.elf"
    mv "$OUT/probe.stripped.elf" "$OUT/probe.elf"
fi

size=$(stat -c%s "$OUT/probe.elf")
if [ "$size" -gt 262144 ]; then
    echo "build-relibc-probe: probe.elf is $size bytes; the image slot is 262144" >&2
    exit 1
fi
cp "$OUT/probe.elf" "$OUT/probe.raw"
truncate -s 262144 "$OUT/probe.raw"

{
    echo "relibc=$have"
    echo "cargo=$($CARGO --version)"
    echo "cc=$($CC --version | sed -n '1p')"
    echo "probe_elf=$size bytes"
} >"$OUT/BUILD-PROVENANCE.txt"
sha256sum "$OUT/probe.elf" "$OUT/probe.raw" >"$OUT/SHA256SUMS"

echo "build-relibc-probe: ELF $OUT/probe.elf"
echo "build-relibc-probe: raw $OUT/probe.raw"
cat "$OUT/BUILD-PROVENANCE.txt"
