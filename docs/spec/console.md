# Console service

**Status:** draft
**Version:** 0.1
**Owner:** kernel / console
**Related:** [`architecture.md`](architecture.md), [`modules.md`](modules.md),
[`mcp.md`](mcp.md), [`boot-protocol.md`](boot-protocol.md).

## 1. One ordered byte stream

The kernel has one logical console byte stream.  `kernel::write_bytes` is the
only writer for kernel messages, diagnostics and native user `stdout`/`stderr`:

1. every byte is written to the Z85C30 serial port;
2. every byte is appended to the 8 KiB serial telemetry ring read by MCP
   `serial.read`;
3. the bytes are queued for a connected telnet client, with bare LF expanded to
   CRLF for terminal display.

Serial remains the low-level boot path.  It is available before the network,
telnet and MCP exist, and it is the authoritative transcript.  MCP reads the
same serial ring as before; it does not consume the telnet queue.

## 2. Telnet

The telnet console has two writers with separate framing:

- **Console output** from `write_bytes` is queued and drained by
  `telnet::poll` and at the end of each command, before the next `sash>`
  prompt.  This is why a user syscall can print while the socket set is already
  borrowed without re-entering smoltcp.  If the transmit window is full the
  queue retains the bytes; when the queue itself is full the oldest bytes are
  discarded.
- **Command replies** (`status`, `net`, `help`, fetches) are written directly to
  the requesting socket and are not copied to serial.  They are responses to a
  command, not part of the asynchronous console stream.

A client that connects after boot does not receive a serial replay.  It sees
new console output from that point; use MCP `serial.read` for the boot
transcript.  Bare LF in user output is expanded to CRLF before it reaches the
teletype.

## 3. Sash

The kernel console is the standalone shell, `sash`.  The same command parser
serves telnet and the local PS/2 keyboard/display; the local path is independent
of the network and works with `net=off`.  The prompt is `sash>`.

The clean-room command set is:

| Command | Meaning |
|---|---|
| `help`, `?` | Command list |
| `version` | Sash and kernel identity |
| `pwd` | Current bring-up directory |
| `cd DIRECTORY` | Change directory |
| `ls [DIRECTORY]` | List the ramfs namespace |
| `cat FILE` | Write a file to the console |
| `printenv NAME` | Read a PROM environment value |
| `echo TEXT` | Echo the argument |
| `status`, `net`, `mem`, `time` | Kernel, network, RAM and clock summaries |
| `hinv` | CPU, memory and network inventory |
| `packages` | List the GitHub package index |
| `packages install NAME` | Verify and load a module, or write a verified userland image to SCSI target 2 |
| `packages run NAME` | Fetch, verify and run a userland image from RAM |
| `mount URL [...]` | Inspect EROFS over HTTPS Range |
| `boot` | Retry the target-2 user image |
| `reboot` | Restart from the PROM |

Telnet additionally retains the kernel-only commands `diagnostics`, `scsi`,
`http`, `https`, `ntp`, `ping`, `module`, `user`, `userelf`, `panic`, `trap`,
`tlb` and `gfx`; local sash deliberately does not expose network fetches or
test hooks.  Sash has no userland, process table or job control dependency.

The local prompt is printed when the target-2 user image returns, or when no
image is attached.  Telnet sash is independently available whenever networking
is up.  The `packages` and `mount` commands are deferred so they run outside the
telnet socket borrow; the local console runs them directly.

## 4. Native user output

The provisional user ABI now has a small fd table: console descriptors 0/1/2,
`/dev/null`, a read/write ramfs fixture and read-only directory handles.  Writes
to a console fd go through the same console service, so they appear on serial,
in the MCP ring and on a connected telnet session.  The std bring-up image uses
fd 1/2 for `println!`; the full POSIX fd table and VFS remain future work.

## 5. Boot readiness

The kernel emits one `BOOT-READY` or `BOOT-NOT-READY` line after the RTC, network
and module autoload have run (see
[`boot-protocol.md`](boot-protocol.md) §10).  Test harnesses must poll that line;
the individual `network: dns`, `mcp: listening` and `autoload: module returned`
messages are diagnostics, not the readiness contract.
