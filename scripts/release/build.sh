#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Build the IRIX 7 release artefacts.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

VERSION="${GITHUB_REF_NAME:-$(git describe --tags --always --dirty 2>/dev/null || echo dev)}"
DIST="$ROOT/dist"
TARGET="mips3-sgi-irix7-n32"
TARGET_JSON="targets/$TARGET.json"
X86_TARGET="x86_64-unknown-none"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

rm -rf "$DIST"
mkdir -p "$DIST"

echo "Building IRIX 7 $VERSION"

# Use the repository's pinned toolchain when CI installed it; otherwise the
# nightly currently on PATH is assumed to have rust-src.
if command -v rustup >/dev/null 2>&1; then
  if ! rustup run nightly-2026-10-03 rustc --version >/dev/null 2>&1; then
    rustup toolchain install nightly-2026-10-03 --component rust-src --component llvm-tools-preview
  fi
fi

HOST="$(rustc -vV | sed -n 's/^host: //p')"
LLVM_OBJCOPY="$(rustc --print sysroot)/lib/rustlib/$HOST/bin/llvm-objcopy"
test -x "$LLVM_OBJCOPY" || {
  echo "llvm-objcopy not found at $LLVM_OBJCOPY" >&2
  exit 1
}

# The kernel and the build-std crates use opt-level z, matching the CI boot gate.
CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
  CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tkernel/linker.ld" \
  cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
  --features cross --target "$TARGET_JSON" -p kernel

# The ROM downloads the kernel at boot; it does not embed it.  The PROM must fit
# the 512 KiB window, so the ROM also builds at opt-level z with one codegen
# unit (about 470 KB at this setting, against about 650 KB at opt-level 2).
CARGO_PROFILE_RELEASE_OPT_LEVEL=z CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
  CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Trom/linker.ld" \
  cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
  --features cross --target "$TARGET_JSON" -p rom

python3 rom/tools/make_prom.py \
  "$TARGET_DIR/$TARGET/release/rom" "$DIST/irix7-rom.bin"

# The kernel alone, as the flat image the ROM downloads (M1-ADR-0002).  Its
# first byte is the entry at 0x88040000; the ROM copies it there and jumps.
# The kernel carries every service; there are no loadable modules.
# The name is fixed so `releases/latest/download/kernel-indy-latest.bin` always
# resolves to the newest release, and the ROM never changes for a kernel update.
"$LLVM_OBJCOPY" -O binary \
  "$TARGET_DIR/$TARGET/release/kernel" "$DIST/kernel-indy-latest.bin"

rom_size="$(stat -c%s "$DIST/irix7-rom.bin")"
if [ "$rom_size" -gt 524288 ]; then
  echo "error: PROM image is $rom_size bytes, over the 524288-byte window" >&2
  exit 1
fi
echo "PROM image: $rom_size / 524288 bytes"

# The x86_64 machine image.  This slice is boot-only: QEMU's PVH path proves
# the 32-bit entry, long mode, the deterministic memory map, the IDT and the
# PIT.  The shared core and virtio devices follow; the image is published now
# so the release pipeline carries both machines from the first slice.
CARGO_TARGET_X86_64_UNKNOWN_NONE_RUSTFLAGS="-Clink-arg=-Tkernel-x86/linker.ld -Crelocation-model=static" \
  cargo build --release -Z build-std=core,alloc \
    -Z build-std-features=compiler-builtins-mem \
    --target "$X86_TARGET" -p kernel-x86 --features cross
cp "$TARGET_DIR/$X86_TARGET/release/kernel-x86" "$DIST/irix7-x86_64.elf"
echo "x86_64 image: $(stat -c%s "$DIST/irix7-x86_64.elf") bytes"

# One manifest, one entry per machine.  The Indy entry carries the ROM and the
# kernel; the x86_64 entry carries the machine image and its
# stage, so a consumer can tell what is actually runnable on each target.
cat > "$DIST/manifest.json" <<MANIFEST
{
  "name": "IRIX 7",
  "version": "$VERSION",
  "created": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "commit": "$(git rev-parse HEAD 2>/dev/null || echo unknown)",
  "machines": {
    "indy": {
      "arch": "mips3",
      "kernel_abi": "n32",
      "rom_abi": "o32",
      "rom_sha256": "$(sha256sum "$DIST/irix7-rom.bin" | awk '{print $1}')",
      "kernel_sha256": "$(sha256sum "$DIST/kernel-indy-latest.bin" | awk '{print $1}')"
    },
    "x86_64": {
      "arch": "x86_64",
      "image": "irix7-x86_64.elf",
      "image_sha256": "$(sha256sum "$DIST/irix7-x86_64.elf" | awk '{print $1}')",
      "stage": "boot"
    }
  }
}
MANIFEST

echo
echo "Artefacts in $DIST:"
ls -lh "$DIST"
