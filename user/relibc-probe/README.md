# relibc-linked C probe

`probe.c` is the first C program linked against the relibc IRIX 7 PAL
(`librelibc.a`) rather than the freestanding oracle in [`../c-probe`](../c-probe).
It uses relibc's `write`, `stat`, `fstat`, `getpid`, `getauxval` and
`__errno_location`, and relibc's MIPS n32 `crt0` `_start`.

Build and run it with [`../../tools/relibc-pal/build.sh`](../../tools/relibc-pal/build.sh)
and [`../../tools/relibc-pal/README.md`](../../tools/relibc-pal/README.md).

The fixed marker is `IRIX7-RELIB-PROBE-1`.  The program exits 0 only if every
check passes:

- `write` returns the marker length;
- `argc == 1` and `argv[0]` is present (System V stack);
- `getpid() == 1`;
- `getauxval(AT_PAGESZ)`, `AT_ENTRY` and `AT_CLKTCK` are the values the loader
  built (`$a3`);
- `$28` is non-zero and `__errno_location() == TP + 8`;
- `stat("/hello.txt")` reports a 28-byte regular file with one link, 4096-byte
  blocks and one block;
- `fstat(999)` returns `-1` and sets `EBADF`;
- `stat("/missing")` returns `-1` and sets `ENOENT`.
