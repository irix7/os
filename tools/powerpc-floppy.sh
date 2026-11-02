#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Build a 1.44 MB HFS floppy image that Open Firmware 2.x can boot on a
# PowerBook G3.  The image contains the PowerPC kernel as an XCOFF file and a
# short README; the boot command is `boot fd2:,\IRIX7.XCF` with the floppy
# module in the left MediaBay.
#
# This is a direct kernel boot, not a netbooter.  The XCOFF wrapper is
# `tools/ppc-xcoff`; the HFS filesystem is written with hfsutils.
#
# Usage:
#   tools/powerpc-floppy.sh [OUT_IMG]
#
# Environment:
#   CARGO        cargo binary (default: cargo)
#   HFSUTILS_DIR directory holding hformat/hmount/hcopy (default: PATH)
#   PPC_IMAGE    boot this flat kernel image instead of building one
#   PPC_XCOFF    use this XCOFF file instead of wrapping the image

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
OUT="${1:-$REPO/dist/irix7-powerpc-floppy.img}"
WORK="${PPC_FLOPPY_WORK:-${TMPDIR:-/tmp}/irix7-powerpc-floppy}"
CARGO="${CARGO:-cargo}"

mkdir -p "$WORK" "$(dirname "$OUT")"

if [ -n "${HFSUTILS_DIR:-}" ]; then
    export PATH="$HFSUTILS_DIR:$PATH"
fi
for tool in hformat hmount hcopy humount hls; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "powerpc-floppy: $tool not found; install hfsutils or set HFSUTILS_DIR" >&2
        exit 2
    }
done

if [ -n "${PPC_XCOFF:-}" ]; then
    XCOFF="$PPC_XCOFF"
else
    if [ -n "${PPC_IMAGE:-}" ]; then
        RAW="$PPC_IMAGE"
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
        RAW="$WORK/kernel-ppc.bin"
        "$OBJCOPY" -O binary "$ELF" "$RAW"
    fi

    # The kernel entry is 0x01000000.  XCOFF o_entry is a function-descriptor
    # address, so put the descriptor in .data at 0x01F00000; Open Firmware
    # reads the word there and jumps to it.
    XCOFF="$WORK/IRIX7.XCF"
    "$CARGO" run --quiet --manifest-path "$REPO/tools/ppc-xcoff/Cargo.toml" -- \
        "$RAW" 0x01000000 0x01F00000 "$XCOFF"
fi

[ -f "$XCOFF" ] || {
    echo "powerpc-floppy: missing XCOFF image $XCOFF" >&2
    exit 1
}
if [ "$(stat -c%s "$XCOFF")" -gt $((1440 * 1024 - 4096)) ]; then
    echo "powerpc-floppy: XCOFF image is too large for the floppy" >&2
    exit 1
fi

README="$WORK/README.TXT"
{
    echo "IRIX 7 PowerPC G3 floppy"
    echo "========================"
    echo
    echo "Boot command at the Open Firmware prompt:"
    echo
    echo "    boot fd2:,\\IRIX7.XCF"
    echo
    echo "The floppy module must be in the left MediaBay."
    echo "The image boots the PowerPC kernel directly.  It is not a netbooter."
    echo
    echo "Kernel: $(git -C "$REPO" describe --always --dirty --tags 2>/dev/null || echo unknown)"
    echo "Commit: $(git -C "$REPO" rev-parse HEAD 2>/dev/null || echo unknown)"
    echo
    echo "Console: the kernel drives both ESCC channels and mirrors the boot"
    echo "log, so the Geoport and the internal IR link both show it.  The"
    echo "Open Firmware output-device selection is logged at boot."
} >"$README"

rm -f "$OUT"
truncate -s 1474560 "$OUT"
hformat -l IRIX7 "$OUT" >/dev/null
hmount "$OUT" >/dev/null
hcopy -r "$XCOFF" :IRIX7.XCF
hattrib -t tbxi -c UNIX :IRIX7.XCF
hcopy -r "$README" :README.TXT
humount >/dev/null

listing="$(hmount "$OUT" 2>/dev/null; hls -l; humount >/dev/null 2>&1)"
echo "$listing"
case "$listing" in
    *IRIX7.XCF*) ;;
    *) echo "powerpc-floppy: IRIX7.XCF is missing from the image" >&2; exit 1 ;;
esac

size="$(stat -c%s "$OUT")"
[ "$size" -eq 1474560 ] || {
    echo "powerpc-floppy: image is $size bytes, expected 1474560" >&2
    exit 1
}
echo "powerpc-floppy: $OUT ($size bytes, $(stat -c%s "$XCOFF")-byte XCOFF)"
