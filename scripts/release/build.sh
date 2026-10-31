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

# Kernel first: rom/build.rs embeds its linked segments.  The kernel and the
# build-std crates inside it use opt-level z, matching the CI boot gate; the M1
# image no longer fits the PROM at opt-level s.  Modules keep their own level.
CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
  CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tkernel/linker.ld" \
  cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
  --features cross --target "$TARGET_JSON" -p kernel

# ROM with the kernel embedded.
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Trom/linker.ld" \
  cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
  --features cross --target "$TARGET_JSON" -p rom

python3 rom/tools/make_prom.py \
  "$TARGET_DIR/$TARGET/release/rom" "$DIST/irix7-rom.bin"

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
  cargo build --release -Z build-std=core \
    -Z build-std-features=compiler-builtins-mem \
    --target "$X86_TARGET" -p kernel-x86 --features cross
cp "$TARGET_DIR/$X86_TARGET/release/kernel-x86" "$DIST/irix7-x86_64.elf"
echo "x86_64 image: $(stat -c%s "$DIST/irix7-x86_64.elf") bytes"

# The MCP control plane is a flat kernel module.
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tmodules/mcp/linker.ld" \
  cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
  --target "$TARGET_JSON" -p mcp-module
"$LLVM_OBJCOPY" -O binary \
  "$TARGET_DIR/$TARGET/release/mcp-module" "$DIST/mcp.module"

module_sha="$(sha256sum "$DIST/mcp.module" | awk '{print $1}')"
raw_base="${RAW_BASE:-https://raw.githubusercontent.com/${GITHUB_REPOSITORY:-irix7/os}/main}"

# Runtime manager and the signed dependency graphs (demo and M2 mesh). The
# manager image is limited to the first 256 KiB of slot B; the arena regions
# follow it.
runtime_dir="$DIST/runtime"
mkdir -p "$runtime_dir" "$TARGET_DIR/modules"
sign_key="${MODULE_SIGNING_KEY:-}"
sign_epoch="${MODULE_SIGNING_EPOCH:-1}"
trust_policy="$runtime_dir/release-trust.conf"
sign_args=()
if [ -n "$sign_key" ]; then
  cargo run --quiet --manifest-path tools/module-release/Cargo.toml -- \
    trust --out "$trust_policy" --seed "$sign_key" --epoch "$sign_epoch"
  sign_args=(--key "$sign_key" --epoch "$sign_epoch")
elif [ "${IRIX7_RELEASE_UNSIGNED:-0}" = "1" ]; then
  echo "warning: building an explicitly unsigned runtime release"
  cargo run --quiet --manifest-path tools/module-release/Cargo.toml -- \
    trust --out "$trust_policy" --epoch "$sign_epoch" --unsigned
  sign_args=(--allow-unsigned)
else
  echo "error: MODULE_SIGNING_KEY is required; set IRIX7_RELEASE_UNSIGNED=1 only for an explicitly unsigned build" >&2
  exit 1
fi

IRIX7_TRUST_POLICY="$trust_policy" \
CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tmodules/manager/linker.ld" \
  cargo build --release -Z build-std=core,alloc \
    -Z build-std-features=compiler-builtins-mem -Zjson-target-spec \
    --target "$TARGET_JSON" -p runtime-manager-module
"$LLVM_OBJCOPY" -O binary \
  "$TARGET_DIR/$TARGET/release/runtime-manager-module" "$TARGET_DIR/modules/manager.flat"

CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tmodules/demo-dep/linker.ld" \
  cargo build --release -Z build-std=core,alloc \
    -Z build-std-features=compiler-builtins-mem -Zjson-target-spec \
    --target "$TARGET_JSON" -p runtime-demo-dep-module
"$LLVM_OBJCOPY" -O binary \
  "$TARGET_DIR/$TARGET/release/runtime-demo-dep-module" "$TARGET_DIR/modules/demo-dep.flat"

CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tmodules/demo-root/linker.ld" \
  cargo build --release -Z build-std=core,alloc \
    -Z build-std-features=compiler-builtins-mem -Zjson-target-spec \
    --target "$TARGET_JSON" -p runtime-demo-root-module
"$LLVM_OBJCOPY" -O binary \
  "$TARGET_DIR/$TARGET/release/runtime-demo-root-module" "$TARGET_DIR/modules/demo-root.flat"

CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tmodules/m2-mesh/linker.ld" \
  cargo build --release -Z build-std=core,alloc \
    -Z build-std-features=compiler-builtins-mem -Zjson-target-spec \
    --target "$TARGET_JSON" -p runtime-m2-mesh-module
"$LLVM_OBJCOPY" -O binary \
  "$TARGET_DIR/$TARGET/release/runtime-m2-mesh-module" "$TARGET_DIR/modules/m2-mesh.flat"

CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tmodules/m2-root/linker.ld" \
  cargo build --release -Z build-std=core,alloc \
    -Z build-std-features=compiler-builtins-mem -Zjson-target-spec \
    --target "$TARGET_JSON" -p runtime-m2-root-module
"$LLVM_OBJCOPY" -O binary \
  "$TARGET_DIR/$TARGET/release/runtime-m2-root-module" "$TARGET_DIR/modules/m2-root.flat"

cargo run --quiet --manifest-path tools/module-release/Cargo.toml -- \
  build --spec packages/runtime-modules.toml --out "$runtime_dir" \
    --target-dir "$TARGET_DIR" --raw-base "$raw_base" --release "$VERSION" \
    "${sign_args[@]}"

manager_sha="$(sha256sum "$runtime_dir/manager.module" | awk '{print $1}')"
runtime_index_sha="$(sha256sum "$runtime_dir/package-index.conf" | awk '{print $1}')"
manager_size="$(stat -c%s "$runtime_dir/manager.module")"
if [ "$manager_size" -gt 262144 ]; then
  echo "error: manager.module is $manager_size bytes, over the 262144-byte slot-B region" >&2
  exit 1
fi

cat > "$DIST/boot-config.conf" <<CONF
# Generated by scripts/release/build.sh for $VERSION.
version=1
release=$VERSION
rom.url=https://github.com/${GITHUB_REPOSITORY:-irix7/os}/releases/download/$VERSION/irix7-rom.bin
mcp.url=$raw_base/boot/modules/$VERSION/mcp.module
mcp.sha256=$module_sha
CONF

# One manifest, one entry per machine.  The Indy entry carries the ROM and the
# signed runtime graph; the x86_64 entry carries the machine image and its
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
      "mcp_sha256": "$module_sha",
      "manager_sha256": "$manager_sha",
      "runtime_index_sha256": "$runtime_index_sha",
      "runtime_signatures": "$([ "${IRIX7_RELEASE_UNSIGNED:-0}" = "1" ] && echo unsigned || echo ed25519)"
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
echo
echo "Runtime artefacts in $runtime_dir:"
ls -lh "$runtime_dir"
