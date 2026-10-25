#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the static Rust std hello from the patched rust-src tree prepared by
# tools/std-spike/build-std.sh, then wrap it as a 256 KiB SCSI target-2 image.
set -eu

REPO=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT=${1:-$REPO/target/std-hello}
STD_WORK=${STD_SPIKE_WORK:-$REPO/target/std-spike}
RUST_SRC="$STD_WORK/rust-src-std-spike/library"
TARGET="$REPO/targets/mips3-sgi-irix7-native-n32.json"
SYS="$OUT/syslib"

if [ ! -d "$RUST_SRC/std" ]; then
    echo "build-std-hello: patched rust-src not found at $RUST_SRC" >&2
    echo "build-std-hello: run tools/std-spike/build-std.sh first" >&2
    exit 2
fi

mkdir -p "$OUT" "$SYS"
for lib in c m rt pthread; do
    if [ ! -f "$SYS/lib$lib.a" ]; then
        ar cr "$SYS/lib$lib.a"
    fi
done

CARGO_TARGET_DIR="$OUT/target" \
__CARGO_TESTS_ONLY_SRC_ROOT="$RUST_SRC" \
RUSTFLAGS="-Clink-arg=-T$REPO/user/hello/linker.ld \
  -Clink-arg=-e__start -Clink-arg=--gc-sections \
  -Clink-arg=-L$SYS" \
    cargo build --release \
        --manifest-path "$REPO/user/std-hello/Cargo.toml" \
        -Zbuild-std=std,panic_abort -Zbuild-std-features= \
        -Zjson-target-spec --target "$TARGET"

HOST=$(rustc -vV | sed -n 's/^host: //p')
STRIP="$(rustc --print sysroot)/lib/rustlib/$HOST/bin/llvm-strip"
if [ ! -x "$STRIP" ]; then
    echo "build-std-hello: llvm-strip not found at $STRIP" >&2
    exit 2
fi
"$STRIP" --strip-all \
    "$OUT/target/mips3-sgi-irix7-native-n32/release/std-hello" \
    -o "$OUT/std-hello.elf"
cp "$OUT/std-hello.elf" "$OUT/std-hello.raw"
truncate -s 262144 "$OUT/std-hello.raw"
echo "std ELF:  $OUT/std-hello.elf"
echo "std disk: $OUT/std-hello.raw"
