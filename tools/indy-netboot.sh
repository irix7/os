#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Build the IRIX 7 kernel for a netboot from the SGI factory PROM.
#
# The kernel is built with the `factory-prom` feature (entry from ARCS, RAM
# sized from the memory controller, NVRAM left unwritten, SCSI disabled) and
# converted to ECOFF as `irix7` in the TFTP directory.
#
# Usage:
#   tools/indy-netboot.sh [TFTP-DIR]        default: target/netboot
#
# Serving the image, either:
#   - the lab host's dnsmasq (jupiter-os `jupiter.sgiNetboot`), which serves
#     /var/lib/sgi-netboot: run `tools/indy-netboot.sh /var/lib/sgi-netboot`;
#   - or this repository's server, as root for ports 67 and 69:
#       sudo tools/sgi-netboot/target/release/sgi-netboot serve \
#         --iface eno1 --server 10.1.1.3 --mac 08:00:69:12:12:12 \
#         --ip 10.1.1.112 --dir target/netboot
#   Run one of the two; both bind ports 67 and 69.
#
# Then at the Indy's PROM Command Monitor:  boot -f bootp()irix7
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

OUT="${1:-$ROOT/target/netboot}"
TARGET="mips3-sgi-irix7-n32"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
mkdir -p "$OUT"

# Same optimisation level as the release kernel.
CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
  CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS="-Clink-arg=-Tkernel/linker.ld" \
  cargo build --release -Z build-std=core,alloc -Zjson-target-spec \
  --features cross,factory-prom --target "targets/$TARGET.json" -p kernel

cargo build --release --manifest-path tools/sgi-netboot/Cargo.toml
tools/sgi-netboot/target/release/sgi-netboot image \
  "$TARGET_DIR/$TARGET/release/kernel" "$OUT/irix7"
sha256sum "$OUT/irix7"
