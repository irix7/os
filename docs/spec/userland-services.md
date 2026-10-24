# Userland service contract

Status: **draft**. This records the boundary between the kernel's native
services and the POSIX.1-2024 userland.

## Principle

The native capability ABI remains the kernel's internal object model. User
programs see **POSIX.1-2024** wherever POSIX defines the operation. A userland
program must not need a private IRIX 7 syscall to do something POSIX already
describes.

Kernel services that have no POSIX equivalent — Newport graphics, the module
ABI, the MCP control plane, hardware inventory — keep a documented native API.
They are not forced into misleading POSIX names.

## Service mapping

| Kernel/service | Userland contract | Layer |
|---|---|---|
| Serial and local console | `/dev/tty`, `termios`, `read`/`write`, `poll`, `ioctl(TIOCGWINSZ)` | POSIX device |
| Keyboard | `/dev/input` or terminal input events | Native device + POSIX tty |
| RTC and CP0 timer | `clock_gettime`, POSIX timers, `nanosleep` | POSIX |
| Frame allocator / VM | `mmap`, `mprotect`, `munmap` | POSIX |
| SCSI and block devices | block device namespace, `open`/`read`/`write`/`stat` | POSIX file layer |
| EROFS/RAM filesystems | `openat`, `read`, `stat`, `readdir`, `getcwd`, `chdir` | POSIX |
| Seeq network | `socket`, `bind`, `connect`, `send`, `recv`, `poll` | POSIX sockets |
| DNS resolver | `getaddrinfo`, `getnameinfo`, resolver configuration | POSIX libc |
| Process creation | `posix_spawn`, `execve`, `waitpid`, process groups | POSIX process layer |
| Signals | `sigaction`, `kill`, `raise`, `SIGCHLD`/`SIGWINCH`/`SIGINT` | POSIX |
| Newport graphics | native window/display API; X11 is a later server | Native |
| MCP control plane | native module/service API | Native |
| Module loader | native capability ABI | Native |

`fork` is not required for the native shell. The native ABI is spawn-based;
`posix_spawn`/`execve` is the intended process-creation path. A POSIX
compatibility layer may provide `fork` later where the implementation permits
it.

## TLS and HTTP

POSIX.1-2024 defines sockets and the resolver, not TLS. TLS is a **library
service**, not a syscall.

- The kernel's TLS client remains a bootstrap component for fetching the boot
  configuration and modules before userland exists.
- The TLS and HTTP implementations should be factored into Rust crates with a
  transport trait, so the kernel boot path and the userland library share one
  implementation rather than growing two TLS stacks.
- Userland TLS uses POSIX sockets and a CA trust store; it does not call a
  private kernel TLS syscall.
- HTTP is likewise a userland library on top of TLS or plain sockets. The
  kernel's HTTP client is only for bootstrap.

## Shell

The interactive shell is a userland program on the POSIX layer.

- The default interactive shell is a **Rust, fish-like shell** built on
  `reedline` (MIT) once Rust `std` and a terminal backend exist. The parser,
  executor and fish-like policies are ours; `reedline` supplies the editor UX.
- A POSIX `/bin/sh` is a separate program. `brush` (MIT) is the leading
  candidate once the POSIX process, fd, signal and terminal layers exist.
- Fish itself is not reused: its `GPL-2.0-only` licence is incompatible with
  this GPL-3.0-only project.
- A small `no_std` shell may exist as a bootstrap/debug console, but it is not
  the product shell and must not force the POSIX shell into the kernel.

## Dependency work

The shell and POSIX userland are gated on these primitives, in order:

1. Console read, tty/termios, file descriptors, `poll`, a writable namespace.
2. `posix_spawn`/`execve`, `waitpid`, pipes, redirection and signals.
3. Process groups, controlling terminal and job control.
4. Rust `std` support: filesystem, process, time, environment, threads,
   `std::os::unix` and the terminal backend.
5. `reedline` and the fish-like shell; `brush` for `/bin/sh`.
6. Userland TLS/HTTP over POSIX sockets, sharing the bootstrap implementation.

Every dependency is recorded with its licence, platform assumptions, build
requirements and port/replacement strategy. A dependency being large is not a
reason to drop a feature; the missing OS service is the work item.

## Current status

The kernel paints the boot log, hands over to the native ELF on SCSI target 2,
and a static Rust `std` image now runs with args, time, sleep, a small
read/write ramfs, `stat`, `read_dir`, `chdir` and relative paths.  The bring-up
uses a provisional 42-symbol libc shim; the production POSIX/C ABI layer will be
the [relibc fork](https://github.com/irix7/relibc), whose n32 big-endian
Linux/QEMU gate has passed (TLS, `setjmp`, pthreads).  A provisional IRIX PAL
scaffold builds and links C probes but has not been booted; the kernel error
channel, TLS and `stat` ABI are still open.

The next blockers are the real POSIX layers: a general namespace/filesystem,
threads and TLS, `posix_spawn`, signals, termios and sockets.  The kernel stack
top is now `0x88300000`, with linker assertions preventing `.bss`/stack overlap;
the loader uses 4 KiB pairs for image/stack and two 256 KiB pairs for a 1 MiB
heap.  See [`../research/std-hello-bringup.md`](../research/std-hello-bringup.md)
and [`../research/relibc-n32-bringup.md`](../research/relibc-n32-bringup.md).
