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
  `telnet::poll` and at the end of each command, before the next `kernel>`
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

## 3. Native user output

The provisional user ABI now has a small fd table: console descriptors 0/1/2,
`/dev/null`, a read/write ramfs fixture and read-only directory handles.  Writes
to a console fd go through the same console service, so they appear on serial,
in the MCP ring and on a connected telnet session.  The std bring-up image uses
fd 1/2 for `println!`; the full POSIX fd table and VFS remain future work.

## 4. Boot readiness

The kernel emits one `BOOT-READY` or `BOOT-NOT-READY` line after the RTC, network
and module autoload have run (see
[`boot-protocol.md`](boot-protocol.md) §10).  Test harnesses must poll that line;
the individual `network: dns`, `mcp: listening` and `autoload: module returned`
messages are diagnostics, not the readiness contract.
