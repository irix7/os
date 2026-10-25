#!/usr/bin/env bash
# Build the M1 local-policy Wasm guest and place the flat module where the
# slot-B image embeds it.
#
# The guest is a standalone no_std wasm32 crate.  It uses `-Z build-std=core`
# so a pinned nightly with rust-src is enough; no wasm32 target component is
# required in the toolchain.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${1:-$ROOT/modules/wasm/src/guest.wasm}"
CRATE="$ROOT/modules/wasm/guest"

release="$(rustc -vV | sed -n 's/^release: //p')"
case "$release" in
  *nightly*) ;;
  *)
    echo "build-wasm-guest: a nightly Rust toolchain is required for -Z build-std" >&2
    exit 2
    ;;
esac

cd "$CRATE"
cargo build --release --target wasm32-unknown-unknown -Z build-std=core
cp "target/wasm32-unknown-unknown/release/wasm_policy_guest.wasm" "$OUT"
echo "wasm guest: $OUT ($(stat -c%s "$OUT") bytes)"
