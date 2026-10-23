#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-only
# Rebuild the golden EROFS fixtures.  Requires erofs-utils >= 1.9.4.
set -eu

cd "$(dirname "$0")"
umask 022

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
src="$work/root"

mkdir -p "$src/etc" "$src/dir"
printf 'Welcome to IRIX 7\n' > "$src/etc/motd"
printf 'hello, erofs\n' > "$src/hello.txt"
printf 'nested\n' > "$src/dir/nested.txt"
: > "$src/empty"
ln -s hello.txt "$src/link"
python3 - "$src/big.bin" <<'PY'
import sys

with open(sys.argv[1], "wb") as image:
    image.write(bytes((i * 7) & 0xFF for i in range(9000)))
PY

make_image() {
    output=$1
    shift
    mkfs.erofs -T 1700000000 --all-time --all-root -U clear -b 4096 \
        "$@" "$output" "$src" >/dev/null
    fsck.erofs "$output"
}

make_image root.erofs
make_image extended.erofs -Eforce-inode-extended
make_image 48bit.erofs -E48bit

sha256sum root.erofs extended.erofs 48bit.erofs
