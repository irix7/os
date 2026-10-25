# Synthetic IRIX o32 probe

This is the smallest o32 ELF path, not a real IRIX 6.5 application.  It uses
the classic SysV/IRIX syscall numbers for `open(5)`, `read(3)`, `write(4)`,
`close(6)` and `exit(1)`, prints `hello from o32`, reads `/hello.txt` from the
bring-up ramfs, writes it to stdout and exits 42.

It is built with the native n32 Rust target for the instruction encoding, then
its `e_flags` is rewritten to `EF_MIPS_ABI_O32`.  The kernel treats the image as
o32 and dispatches the syscall numbers through the legacy bridge.  A real
container still needs the IRIX libc, `rld`, `sproc`, signals, termios and the
full syscall decoder; this probe only proves the loader personality and the
first five calls.

```sh
tools/build-o32-hello.sh
```

The resulting `o32-hello.raw` is padded to the 256 KiB target-2 image size.
