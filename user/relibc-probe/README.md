# relibc-linked C probe

`probe.c` is a C program linked against the relibc IRIX 7 PAL
(`librelibc.a`) rather than the freestanding oracle in [`../c-probe`](../c-probe).
It uses relibc's `write`, `stat`, `fstat`, `getpid`, `getauxval`,
`__errno_location`, `open`, `lseek`, `pread`, `pwrite`, `dup`, `dup2`,
`fcntl`, `pipe`, `pipe2`, `poll`, `opendir`, `readdir`, `seekdir`, `telldir`
and `posix_getdents`, and relibc's MIPS n32 `crt0` `_start`.

Build and run it with [`../../tools/relibc-pal/build.sh`](../../tools/relibc-pal/build.sh)
and [`../../tools/relibc-pal/README.md`](../../tools/relibc-pal/README.md).

The fixed marker is `IRIX7-RELIB-PROBE-3`.  The program exits 0 only if every
check passes:

- `write` returns the marker length;
- `argc == 1` and `argv[0]` is present (System V stack);
- `getpid() == 1`;
- `getauxval(AT_PAGESZ)`, `AT_ENTRY` and `AT_CLKTCK` are the values the loader
  built (`$a3`);
- `$28` is non-zero and `__errno_location() == TP + 8`;
- `stat("/hello.txt")` reports a 28-byte regular file with one link, 4096-byte
  blocks and one block;
- `fstat(999)` returns `-1` and sets `EBADF`; `stat("/missing")` returns `-1`
  and sets `ENOENT`;
- `lseek` implements `SEEK_SET`, `SEEK_CUR`, `SEEK_END`, `SEEK_DATA` and
  `SEEK_HOLE`, rejects a negative result and a bad `whence` with `EINVAL`, and
  writes then reads back at an offset;
- `pread`/`pwrite` transfer at an explicit offset without moving the position
  of the open file description;
- `dup` and `dup2` share that position, and `dup2` from a bad descriptor is
  `EBADF`;
- `O_CLOEXEC`, `fcntl(F_GETFD)`, `fcntl(F_SETFD)`, `dup` and `dup2` carry
  close-on-exec as per-descriptor state;
- `fcntl(F_GETFL)` reports the access mode, `fcntl(F_SETFL)` sets and clears
  `O_NONBLOCK`, and `dup` shares the status flags with its description;
- a pipe round-trips four bytes, a non-blocking empty read is `EAGAIN` while
  the writer is open, a write after the reader closes is `EPIPE`, and closing
  every writer makes the reader report end-of-file; `lseek` on a pipe is
  `ESPIPE`;
- `poll` with a zero timeout reports an empty pipe as not readable and its
  writer as `POLLOUT`, sees data as `POLLIN`, reports `POLLHUP` after the last
  writer closes and `POLLERR` on a write end with no reader, ignores a negative
  descriptor and reports a closed one as `POLLNVAL`;
- `poll` with a negative timeout on an idle pipe returns after the kernel's
  fixed bound, and a negative timeout on the always-writable console returns
  immediately;
- `pipe2(O_CLOEXEC)` marks both descriptors and rejects unknown flags;
- `posix_getdents` fills three records in one 1024-byte call with `d_off`
  cookies 1, 2, 3, continues with 4 and 5, returns 0 at the end, resumes at
  cookie 1 after `lseek`, and rejects a buffer smaller than one record with
  `EINVAL`;
- `opendir`/`readdir` return the five entries, `seekdir(0)` rewinds and
  `telldir` reports cookie 1.
