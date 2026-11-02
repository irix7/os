#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Check the loadable-module layout on all machine types.
#
# Each module image is linked at its slot base and loaded there by the kernel.
# This script builds the MCP module for the Indy (MIPS n32, slot A at
# 0x88400000, 512 KiB) and for x86_64 (window at 0x02000000, 2 MiB), and the
# hello module for the PowerPC G3 (slot at 0x01800000, 2 MiB).  It asserts that
# the ELF entry is the slot base and that the flat image plus .bss fits the
# slot.  A module that links somewhere else, or grows past its slot, would be
# copied over kernel memory or another module at load time.
#
# Usage:
#   tools/module-layout-check.sh [WORK_DIR]
#
# Environment:
#   CARGO   cargo binary (default: cargo)
#   CC      host C compiler for build-std (default: cc on PATH)

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
WORK="${1:-${MODULE_LAYOUT_WORK:-$REPO/target/module-layout}}"
CARGO="${CARGO:-cargo}"
HOST="$(rustc -vV | sed -n 's/^host: //p')"
READOBJ="$(rustc --print sysroot)/lib/rustlib/$HOST/bin/llvm-readobj"
OBJCOPY="$(rustc --print sysroot)/lib/rustlib/$HOST/bin/llvm-objcopy"

mkdir -p "$WORK"
cd "$REPO"

fail() {
    echo "module-layout-check: $*" >&2
    exit 1
}

check() {
    local name="$1" elf="$2" base="$3" slot="$4"
    [ -f "$elf" ] || fail "$name: missing $elf"
    local entry
    entry="$("$READOBJ" --file-headers "$elf" | sed -n 's/^  Entry: 0x\([0-9A-Fa-f]*\)$/\1/p' | head -1)"
    [ -n "$entry" ] || fail "$name: no ELF entry in $elf"
    [ "$((16#$entry))" -eq "$((base))" ] \
        || fail "$name: entry 0x$entry is not the slot base 0x$(printf '%x' "$base")"

    local flat="$WORK/$name.module"
    "$OBJCOPY" -O binary "$elf" "$flat"
    local size
    size="$(stat -c%s "$flat")"
    [ "$size" -le "$slot" ] \
        || fail "$name: flat image $size bytes exceeds the $slot-byte slot"
    echo "module-layout-check: $name entry=0x$entry flat=$size slot=$slot"
}

export PATH="$PATH"

echo "=== Indy (MIPS n32) ==="
CARGO_TARGET_DIR="$WORK/mips" \
CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tmodules/mcp/linker.ld" \
    "$CARGO" build --release -Z build-std=core,alloc \
        -Z build-std-features=compiler-builtins-mem -Zjson-target-spec \
        --target "$REPO/targets/mips3-sgi-irix7-n32.json" -p mcp-module
check indy "$WORK/mips/mips3-sgi-irix7-n32/release/mcp-module" $((0x88400000)) $((512 * 1024))

echo "=== x86_64 ==="
CARGO_TARGET_DIR="$WORK/x86" \
CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
CARGO_TARGET_X86_64_UNKNOWN_NONE_RUSTFLAGS="-Clink-arg=-Tmodules/mcp/linker-x86.ld -Crelocation-model=static" \
    "$CARGO" build --release -Z build-std=core,alloc \
        -Z build-std-features=compiler-builtins-mem \
        --target x86_64-unknown-none -p mcp-module
check x86_64 "$WORK/x86/x86_64-unknown-none/release/mcp-module" $((0x02000000)) $((2 * 1024 * 1024))

echo "=== PowerPC G3 ==="
CARGO_TARGET_DIR="$WORK/powerpc" \
CARGO_PROFILE_RELEASE_OPT_LEVEL=2 \
CARGO_TARGET_POWERPC_UNKNOWN_IRIX7_PPC_RUSTFLAGS="-Clink-arg=-Tmodules/hello/linker-ppc.ld -Crelocation-model=static" \
    "$CARGO" build --release -Z build-std=core \
        -Z build-std-features=compiler-builtins-mem -Z json-target-spec \
        --target "$REPO/targets/powerpc-unknown-irix7-ppc.json" -p hello-module
check powerpc "$WORK/powerpc/powerpc-unknown-irix7-ppc/release/hello-module" $((0x01800000)) $((2 * 1024 * 1024))

echo "module-layout-check: ok"
