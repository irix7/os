#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the Rust `std` rlib for the native IRIX 7 target
# (targets/mips3-sgi-irix7-native-n32.json) from a patched rust-src tree.
#
# This is the reproducible recipe behind docs/research/rust-std-build-spike.md.
# It does **not** link or run anything: the product is an rlib.  The patched
# `libc` module is compile-only scaffolding; see the report for the gaps.
#
# Usage:
#   tools/std-spike/build-std.sh            # full clean build
#   tools/std-spike/build-std.sh --no-clean # reuse the patched scratch trees
#
# Environment:
#   STD_SPIKE_WORK  scratch root (default: /mnt/europa).  Must have ~1 GiB.
#   CC              host C compiler for build scripts (default: cc)
#
# What it does:
#   1. copies the pinned rust-src to $STD_SPIKE_WORK/rust-src-std-spike
#   2. copies the vendored libc 0.2.189 to $STD_SPIKE_WORK/libc-std-spike
#   3. applies patches/01-std-irix7.patch and patches/02-libc-irix7.patch
#   4. wires the libc copy into the library workspace with [patch.crates-io]
#   5. runs `cargo build -Zbuild-std=std` for the native target
#
# The build is deliberately started from a clean target directory by default.
# Cargo's mtime fingerprinting did not pick up edits to the patched trees in
# every case on this build host (see the report, "Cargo freshness"), and a
# stale build script silently re-adds `restricted_std`.  `--no-clean` reuses
# the trees and the target directory; it does not re-apply the patches, so a
# changed patch file needs a clean run.

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
TARGET_JSON="$REPO_ROOT/targets/mips3-sgi-irix7-native-n32.json"

CLEAN=1
for arg in "$@"; do
    case "$arg" in
        --no-clean) CLEAN=0 ;;
        -h|--help) sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

WORK=${STD_SPIKE_WORK:-/mnt/europa}
RUST_SRC="$WORK/rust-src-std-spike"
LIBC_SRC="$WORK/libc-std-spike"
TARGET_DIR="$WORK/target-std-spike"
PATCHES="$SCRIPT_DIR/patches"

: "${CC:=cc}"
export CC

if ! command -v cargo >/dev/null 2>&1; then
    echo "cargo not found; source the project environment (see /mnt/europa/env.sh)" >&2
    exit 1
fi
if ! command -v "$CC" >/dev/null 2>&1; then
    echo "host C compiler '$CC' not found (build scripts need one)" >&2
    exit 1
fi

SYSROOT=$(rustc --print sysroot)
RUST_SRC_ORIG="$SYSROOT/lib/rustlib/src/rust"
if [ ! -d "$RUST_SRC_ORIG/library/std" ]; then
    echo "rust-src not found at $RUST_SRC_ORIG (rustup component add rust-src)" >&2
    exit 1
fi

echo "==> toolchain: $(rustc -V) ($SYSROOT)"
echo "==> work:      $WORK"

if [ "$CLEAN" = 1 ]; then
    echo "==> removing $RUST_SRC $LIBC_SRC $TARGET_DIR"
    rm -rf "$RUST_SRC" "$LIBC_SRC" "$TARGET_DIR" "$WORK/.std-spike-ready"
fi

if [ -f "$WORK/.std-spike-ready" ]; then
    echo "==> reusing the patched trees (remove --no-clean to rebuild them)"
else
    if [ ! -d "$RUST_SRC/library/std" ]; then
        echo "==> copying rust-src"
        mkdir -p "$RUST_SRC"
        cp -a "$RUST_SRC_ORIG/." "$RUST_SRC/"
    fi

    if [ ! -d "$LIBC_SRC/src/unix" ]; then
        echo "==> copying libc 0.2.189"
        cp -a "$RUST_SRC_ORIG/library/vendor/libc-0.2.189" "$LIBC_SRC"
    fi

    echo "==> applying patches"
    patch -p1 -d "$RUST_SRC" --batch < "$PATCHES/01-std-irix7.patch"
    patch -p1 -d "$LIBC_SRC" --batch < "$PATCHES/02-libc-irix7.patch"

    # Wire the patched libc into the *library* workspace: -Zbuild-std resolves
    # std's dependencies from there, not from the caller's workspace.
    if ! grep -q "^libc = { path = \"$LIBC_SRC\" }" "$RUST_SRC/library/Cargo.toml"; then
        sed -i "/^\[patch.crates-io\]\$/a libc = { path = \"$LIBC_SRC\" }" \
            "$RUST_SRC/library/Cargo.toml"
    fi

    : > "$WORK/.std-spike-ready"
fi

echo "==> building std (this takes a couple of minutes)"
(
    cd "$SCRIPT_DIR/probe"
    CARGO_TARGET_DIR="$TARGET_DIR" \
    __CARGO_TESTS_ONLY_SRC_ROOT="$RUST_SRC/library" \
    cargo build -v -Zjson-target-spec -Zbuild-std=std --target "$TARGET_JSON" \
        > "$WORK/std-spike-build.log" 2>&1
) || {
    echo "build failed; tail of $WORK/std-spike-build.log:" >&2
    tail -40 "$WORK/std-spike-build.log" >&2
    exit 1
}

RLIB=$(find "$TARGET_DIR" -name 'libstd-*.rlib' -print | head -n 1)
if [ -z "$RLIB" ]; then
    echo "no std rlib was produced (see $WORK/std-spike-build.log)" >&2
    exit 1
fi

if grep -q -- '--cfg restricted_std' "$WORK/std-spike-build.log"; then
    echo "ERROR: std was built with restricted_std; the build script is stale." >&2
    echo "       Re-run without --no-clean." >&2
    exit 1
fi

echo "==> ok"
echo "rlib:  $RLIB"
ls -l "$RLIB"
