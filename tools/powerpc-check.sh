#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Prove the PowerBook dev shell works: build a dependency-free no_std crate
# with -Zbuild-std for the PowerPC target and check that the archive holds
# 32-bit big-endian PowerPC ELF objects.
#
# Usage:
#   tools/powerpc-check.sh
#
# Environment:
#   CARGO              cargo binary (default: cargo)
#   CARGO_TARGET_DIR   build directory (default: <repo>/target/powerpc-check)
#   POWERPC_CHECK_READER  force an ELF reader (llvm-readobj or readelf)
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
TARGET_JSON="$REPO/targets/powerpc-unknown-irix7-ppc.json"
TARGET="powerpc-unknown-irix7-ppc"
CRATE="irix7-fmt"
CARGO="${CARGO:-cargo}"

fail() {
  echo "powerpc-check: $*" >&2
  exit 1
}

if ! command -v "$CARGO" >/dev/null 2>&1; then
  fail "$CARGO is not in PATH; enter the shell with 'nix develop .#powerbook'"
fi
if [ ! -f "$TARGET_JSON" ]; then
  fail "target spec missing: $TARGET_JSON"
fi

cd "$REPO"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO/target/powerpc-check}"

echo "powerpc-check: $(rustc --version 2>/dev/null || echo 'rustc not on PATH')"
echo "powerpc-check: target $TARGET_JSON"
echo "powerpc-check: build $CRATE"

"$CARGO" build -Zbuild-std=core -Zjson-target-spec \
  --target "$TARGET_JSON" -p "$CRATE"

RLIB="$CARGO_TARGET_DIR/$TARGET/debug/libirix7_fmt.rlib"
[ -f "$RLIB" ] || fail "expected rlib not found: $RLIB"

# Locate an ELF reader.  The llvm-tools-preview component ships llvm-readobj
# next to rust-lld; a distribution binutils readelf is the fallback.
READER="${POWERPC_CHECK_READER:-}"
if [ -z "$READER" ]; then
  if command -v llvm-readobj >/dev/null 2>&1; then
    READER="$(command -v llvm-readobj)"
  elif command -v rustc >/dev/null 2>&1; then
    host="$(rustc -vV | sed -n 's/^host: //p')"
    candidate="$(rustc --print sysroot)/lib/rustlib/$host/bin/llvm-readobj"
    if [ -x "$candidate" ]; then
      READER="$candidate"
    fi
  fi
fi
if [ -z "$READER" ] && command -v readelf >/dev/null 2>&1; then
  READER="$(command -v readelf)"
fi
[ -n "$READER" ] || fail "no llvm-readobj or readelf in PATH; install llvm-tools-preview"
case "$(basename "$READER")" in
  *readelf*) READER_KIND=gnu ;;
  *) READER_KIND=llvm ;;
esac

if [ "$READER_KIND" = "llvm" ]; then
  headers="$("$READER" --file-headers "$RLIB")"
  formats="$(grep -c '^Format:' <<<"$headers" || true)"
  [ "$formats" -gt 0 ] || fail "llvm-readobj reported no ELF members in $RLIB"
  if grep '^Format:' <<<"$headers" | grep -qv '^Format: elf32-powerpc$'; then
    fail "not every member is elf32-powerpc: $(grep '^Format:' <<<"$headers" | sort -u | tr '\n' ' ')"
  fi
  grep -q '^[[:space:]]*Machine: EM_PPC ' <<<"$headers" || fail "missing EM_PPC machine header"
  grep -q '^[[:space:]]*Class: 32-bit' <<<"$headers" || fail "missing 32-bit class header"
  grep -q '^[[:space:]]*DataEncoding: BigEndian' <<<"$headers" || fail "missing big-endian data encoding"
else
  headers="$("$READER" --file-header "$RLIB")"
  classes="$(grep -c 'Class:[[:space:]]*ELF32' <<<"$headers" || true)"
  [ "$classes" -gt 0 ] || fail "readelf reported no ELF32 members in $RLIB"
  if grep -q 'Class:[[:space:]]*ELF64' <<<"$headers"; then
    fail "archive contains a 64-bit ELF member"
  fi
  grep -q 'Data:.*big endian' <<<"$headers" || fail "missing big-endian data encoding"
  if grep 'Data:' <<<"$headers" | grep -qv 'big endian'; then
    fail "archive contains a little-endian ELF member"
  fi
  grep -q 'Machine:.*PowerPC' <<<"$headers" || fail "missing PowerPC machine header"
  formats="$classes"
fi

size="$(stat -c%s "$RLIB")"
echo "powerpc-check: $RLIB ($size bytes, $formats ELF members, ${READER##*/})"
echo "powerpc-check: OK: elf32-powerpc, big-endian"
