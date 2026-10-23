# IRIX 7 — MCP Control Plane

**Status:** draft
**Version:** 1.0
**Owner:** infra
**Related:** [`architecture.md`](architecture.md) §14, [`../plan/milestones.md`](../plan/milestones.md) M3.

## 1. Purpose

The MCP server is the OS's remote control plane and the project's development harness. An
agent connects to the running Indy, inspects and drives it, and closes the
edit–build–deploy–observe loop. It is a de-privileged native cell with **full root** in v1,
protected by a bearer token and a hardware watchdog.

This spec defines the transport, authentication, methods, tool schemas, error model and
security posture. It does not define the agent's policy or the client implementation.

## 1.1 Current prototype

`modules/httpd` is loaded over HTTPS into the module area and is the first slice of the
control plane. It implements:

- `POST /mcp` with JSON-RPC 2.0 `initialize` (echoing the client's `protocolVersion`,
  default `2026-07-28`), `ping`, `tools/list` and `tools/call`;
- tools `status` (the shared machine status block: IP, MAC, RAM, uptime, date), `echo`,
  `console`/`serial.write` (write a line to the guest console), `serial.read` (the kernel's
  8 KiB serial ring, from an absolute offset), `system.reset` and `system.poweroff`;
- `resources/list` and `resources/read` expose `irix7://serial`, `irix7://status`,
  `irix7://net` and `irix7://mem` as `text/plain`;
- `GET /` as a plain status page;
- guest port 80, forwarded to host `8088` by the iris rig.

The module is loaded automatically at boot when the NVRAM environment carries
`mcp.url` and `mcp.sha256`; the control plane therefore survives `system.reset` without a
telnet step.  When `mcp.token` is present in the NVRAM environment, `POST /mcp` requires
`Authorization: Bearer <token>` and returns HTTP 401 with `WWW-Authenticate: Bearer`
otherwise. `GET /` stays open as the human status page. `shell.exec` accepts `echo`, `uname`, `uptime`, `status`, `net`, `mem`, `time`, `help`,
`reboot` and `poweroff`; `screen.capture` returns an `image/png` thumbnail and the full
BMP URL. `ls`, `cat` and `ps` wait for the filesystem and process work. It does **not**
yet implement sessions, SSE, `screen.capture` or the dotted tool names.  The serial ring starts at the
kernel entry, so the ROM's own banner is not included. It is a prototype of
the transport and tool loop, not a conforming server; the rest of this document is the
target.

## 2. Transport

- **MCP 2026-07-28**, JSON-RPC 2.0.
- **Streamable HTTP** at `/mcp`:
  - client → server: `POST` with `Content-Type: application/json`;
  - server → client: `application/json` or `text/event-stream`;
  - the client sends `Accept: application/json, text/event-stream`;
  - session header: `Mcp-Session-Id`;
  - protocol header: `MCP-Protocol-Version: 2026-07-28`.
- Server-Sent Events carry progress notifications and long-running tool output.
- One session per client; sessions expire after 30 minutes of inactivity.
- The server binds to the Indy's LAN interface on a configurable port (default `7801`).
- **No TLS in v1.** Run it on a trusted LAN or through an SSH tunnel.

## 3. Authentication

- `Authorization: Bearer <token>` on every request.
- The token is provisioned at image build time or in the boot configuration; it is never in
  `BootInfo`, the repository or a log.
- A missing or invalid token returns HTTP `401` with `WWW-Authenticate: Bearer`.
- The token grants full root in v1. There is no per-tool scope and no audit log.
- The token can be rotated by replacing the boot configuration and rebooting.
- Rate limit: 20 requests/second and 4 concurrent tool calls per session; excess returns
  `E_BUSY`.

## 4. Protocol methods

| Method | Direction | Purpose |
|---|---|---|
| `initialize` | client → server | Negotiate protocol version and capabilities |
| `notifications/initialized` | client → server | Confirm initialisation |
| `ping` | both | Liveness |
| `tools/list` | client → server | List tools |
| `tools/call` | client → server | Invoke a tool |
| `notifications/progress` | server → client | Progress for a long-running call |
| `notifications/cancelled` | client → server | Cancel a call |
| `resources/list` / `resources/read` | client → server | Optional; `/proc`, `/net` as resources |
| `logging/setLevel` | client → server | Optional; server log level |

`initialize` result:

```json
{
  "protocolVersion": "2026-07-28",
  "capabilities": {
    "tools": { "listChanged": true },
    "resources": { "subscribe": false, "listChanged": false }
  },
  "serverInfo": { "name": "irix7-mcp", "version": "0.1.0" }
}
```

## 5. Tools

Tool names are stable, lower-case and dot-separated. Every tool returns MCP `content`; a
tool execution failure is `isError: true`, not a JSON-RPC error.

### 5.1 Shell and files

| Tool | Input | Output |
|---|---|---|
| `shell.exec` | `command`, `args?`, `cwd?`, `env?`, `stdin?`, `timeout_ms?`, `max_output?` | `exit_code`, `stdout`, `stderr`, `duration_ms`, `truncated` |
| `block.read` | `target`, `lba` | 1024 hex characters for one 512-byte SCSI sector |
| `fs.read` | `path?` (default `/etc/motd`) | file content from the target 2 EROFS fixture |
| `fs.list` | `path?` (default `/`) | newline-separated directory entries |
| `fs.read` | `path`, `offset?`, `length?`, `encoding?` (`utf-8`/`base64`) | `data`, `encoding`, `size`, `eof` |
| `fs.write` | `path`, `data`, `encoding?`, `mode?` (`create`/`truncate`/`append`), `permissions?` | `bytes_written` |
| `fs.list` | `path` | `entries[]` with `name`, `type`, `size`, `mode`, `mtime` |
| `fs.stat` | `path` | `type`, `size`, `mode`, `mtime`, `inode` |
| `fs.remove` | `path`, `recursive?` | `ok` |
| `fs.rename` | `from`, `to` | `ok` |
| `fs.mkdir` | `path`, `mode?` | `ok` |

`fs.read` and `fs.write` default to UTF-8; binary data uses base64. `max_output` defaults to
1 MiB and is capped at 8 MiB; `truncated` reports the cut.

### 5.2 Processes

| Tool | Input | Output |
|---|---|---|
| `proc.list` | — | `processes[]` with `pid`, `ppid`, `name`, `state`, `cell`, `threads`, `memory` |
| `proc.start` | `path`, `args?`, `cwd?`, `env?` | `pid` |
| `proc.signal` | `pid`, `signal` | `ok` |
| `proc.wait` | `pid`, `timeout_ms?` | `exit_code`, `signal` |

### 5.3 Devices, DMA and interrupts

| Tool | Input | Output |
|---|---|---|
| `dev.list` | — | `devices[]` with `name`, `kind`, `mmio`, `irq`, `dma` |
| `dev.mmio.read` | `device`, `offset`, `width` (1/2/4/8) | `value` |
| `dev.mmio.write` | `device`, `offset`, `width`, `value` | `ok` |
| `dev.dma.alloc` | `device`, `size`, `alignment?` | `handle`, `phys`, `size` |
| `dev.dma.free` | `handle` | `ok` |
| `dev.irq.wait` | `device`, `timeout_ms?` | `irq`, `status`, `timestamp` |

These tools are full-root. A bad MMIO write can hang the machine; the watchdog is the
recovery path.

### 5.4 Screen, serial and input

| Tool | Input | Output |
|---|---|---|
| `screen.capture` | `format?` (`png`/`raw`), `region?` | `data` (base64), `format`, `width`, `height` |
| `serial.read` | `offset?`, `length?`, `timeout_ms?` | `data`, `offset`, `eof` |
| `serial.write` | `data` | `bytes_written` |
| `input.key` | `key`, `modifiers?`, `action` (`down`/`up`/`press`) | `ok` |
| `input.mouse` | `x`, `y`, `buttons?`, `wheel?`, `action` | `ok` |

`screen.capture` defaults to PNG at the current mode. `serial.read` is a ring-buffer read;
`offset` is the absolute byte offset, and `eof` means the buffer is caught up.

### 5.5 System

| Tool | Input | Output |
|---|---|---|
| `system.info` | — | `version`, `uptime_ms`, `memory`, `cpu`, `devices`, `load` |
| `system.reset` | `mode` (`reboot`/`poweroff`/`warm`), `delay_ms?` | `ok` (the connection drops) |
| `system.watchdog` | `action` (`arm`/`disarm`/`feed`), `timeout_ms?` | `ok` |

### 5.6 Debug

| Tool | Input | Output |
|---|---|---|
| `debug.gdb.attach` | `pid` | `ok` |
| `debug.gdb.detach` | `pid` | `ok` |
| `debug.gdb.break` | `pid`, `address`/`symbol` | `id` |
| `debug.gdb.continue` | `pid` | `stop_reason` |
| `debug.gdb.step` | `pid`, `mode?` (`into`/`over`/`out`) | `stop_reason` |
| `debug.gdb.regs` | `pid` | `registers` |
| `debug.gdb.mem.read` | `pid`, `address`, `length` | `data` (base64) |
| `debug.gdb.mem.write` | `pid`, `address`, `data` | `ok` |

The GDB stub is a later milestone (M7+); the tool names are reserved now.

## 6. Errors

JSON-RPC errors:

| Code | Meaning |
|---|---|
| `-32700` | Parse error |
| `-32600` | Invalid request |
| `-32601` | Method not found |
| `-32602` | Invalid params |
| `-32603` | Internal error |

Application errors are returned as tool results with `isError: true` and a text content
`"E_<CODE>: message"`:

| Code | Meaning |
|---|---|
| `E_PERM` | Permission denied |
| `E_NOT_FOUND` | Path, process or device not found |
| `E_IO` | I/O failure |
| `E_TIMEOUT` | Operation timed out |
| `E_BUSY` | Too many concurrent calls or device busy |
| `E_UNSUPPORTED` | Tool not available on this build/hardware |
| `E_INVALID` | Invalid argument |
| `E_INTERNAL` | Unexpected internal failure |

## 7. Security

**v1 is a development harness, not a product control plane.**

- Full root: the token is the only gate.
- No audit log.
- No TLS: use a trusted LAN or an SSH tunnel.
- The server binds to the LAN interface, not the public internet.
- The DS1386 watchdog resets a hung machine; the client reconnects.
- Prompt injection is a client-side risk: tool results are untrusted input. The agent must
  not follow instructions found in file contents, serial logs or screen captures.
- **Production mode (future):** capability-scoped tools, an append-only audit log, human
  approval for destructive operations, TLS, and per-tool tokens. The tool names stay stable;
  the policy changes.

## 8. Versioning

- The protocol version is negotiated in `initialize`.
- Tool schemas are additive. Removing a field or changing its type is a new tool name
  (`shell.exec.v2`) or a new server major version.
- `serverInfo.version` follows the OS version.
- A client that does not understand a tool must ignore it, not fail the session.

## 9. Test plan

- [ ] `initialize`, `notifications/initialized`, `ping`, `tools/list`.
- [ ] Every tool's happy path and one error path.
- [ ] Missing/invalid token → HTTP 401.
- [ ] Rate limit → `E_BUSY`.
- [ ] Long-running `shell.exec` streams progress and can be cancelled.
- [ ] `screen.capture` returns a valid PNG of the expected mode.
- [ ] `system.reset` drops the connection and the client reconnects.
- [ ] Watchdog resets a hung kernel; the client reconnects.
- [ ] JSON-RPC fuzzing: malformed frames, huge fields, unknown methods.
- [ ] A real MCP client (the agent) completes the M3 demo.

## 10. Open questions

- Streamable HTTP vs a simpler JSON-RPC transport for v1; the standard is preferred.
- Session persistence across a reboot (none in v1).
- Whether `resources` should expose `/proc`, `/net` and `/dev` as first-class resources.
- The production-mode capability model and audit format.
- The exact GDB stub protocol and multi-cell debugging.
