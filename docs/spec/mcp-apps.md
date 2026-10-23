# MCP application endpoints

**Status:** draft
**Version:** 0.1
**Owner:** infra
**Related:** [`mcp.md`](mcp.md), [`../plan/milestones.md`](../plan/milestones.md) M9

## 1. Purpose and current reality

The MCP server is the development and control plane for the machine.  Today it is a
**kernel-resident MCP server packaged as the loadable `modules/httpd` image**.  The kernel
fetches it over HTTPS, verifies its SHA-256, loads the flat image at `0x88400000` and calls
its `_start`.  The image is then executed in kernel mode on the kernel stack, in the shared
kernel address space, driven by a 10 ms tick callback.  It owns two TCP slots and listens
on guest port 80.  Its tools (`block.read`, `fs.read`, `fs.list`, `fs.write`, `fs.remove`,
`shell.exec`, `screen.capture`, `serial.read`, `status`, `echo`) are compiled into the
image; there is no registry.

“Module” here describes download, link address and the versioned service-table ABI.  It
does **not** describe privilege: a module is not a process, has no MMU boundary, and is part
of the kernel TCB.  The static-image/loadable-image split exists to keep the compressed PROM
small and to let features be replaced independently; it is not a security boundary.

That is a development prototype.  IRIX 7 does not yet have userland processes or cells:
the kernel is a single address space, loadable images execute with kernel privilege, and
native cells/capabilities arrive in M9.  An application therefore cannot register an MCP
endpoint today, and an application must not receive a raw function pointer that the MCP
server calls in kernel mode.  That would execute application code on the kernel's tick path
and would bypass every isolation boundary the OS is trying to establish.

This document defines the target: one MCP **broker** on the LAN, one MCP server per
application, and a local registration/routing protocol between them.

## 2. Architecture

```text
agent  --- HTTP/JSON-RPC (bearer) --->  mcp broker
                                           |  registry
                                           |  router
                    +----------------------+----------------------+
                    |                      |                      |
              app A mcp server       app B mcp server       built-in tools
              (local endpoint)       (local endpoint)       (broker-owned)
```

The broker is the only thing bound to the LAN.  Applications run ordinary MCP servers on a
local endpoint.  The broker:

- authenticates the agent once;
- lists the application's tools/resources alongside its own;
- forwards a `tools/call` to the owning application and returns its result;
- enforces timeouts, quotas, cancellation and tool-name namespacing;
- removes an application's tools when it exits or its endpoint closes.

Application MCP servers are not exposed directly to the LAN.  They may be IRIX binaries,
native cells, or (during bring-up) a module acting as an application process.

## 3. Local transport

The preferred transport is **9P2000** over the planned local IPC mechanism, because IRIX 7
already needs 9P for remote mounts and because it gives a clean per-application channel.
A Unix-domain-socket transport is acceptable for the first prototype if the native cell
model provides one.  A TCP loopback port is a temporary fallback only while no local IPC
exists; it must bind `127.0.0.1` and must not be reachable from the LAN.

Each application endpoint speaks MCP 2026-07-28 JSON-RPC over a stream:

```text
initialize
notifications/initialized
tools/list
tools/call
resources/list
resources/read
notifications/progress      (server -> broker -> agent)
notifications/cancelled     (agent -> broker -> server)
```

The broker opens one session per application and reuses it.  It may multiplex several
agent sessions onto one application session, with one in-flight call per application
unless the application advertises concurrency.

## 4. Registration

### 4.1 Static manifest

An application may install a manifest before it starts:

```json
{
  "name": "editor",
  "version": "1.2.0",
  "endpoint": "/run/mcp/editor.sock",
  "protocol": "mcp-2026-07-28",
  "tools": ["open", "save"],
  "resources": ["buffers"],
  "capabilities": ["fs.read", "fs.write"]
}
```

The broker validates the manifest, assigns the namespace `app.editor`, and records the
endpoint.  The manifest is data, not executable code.

### 4.2 Dynamic registration

A running application may register over a control channel:

```text
mcp.register  { name, version, endpoint, protocol, capabilities }
mcp.unregister { name }
```

The control channel is local-only and authenticated by the application's capability.  The
broker rejects duplicate names, endpoints outside the local namespace, unsupported protocol
versions and applications that do not answer an `initialize` within a bounded time.

### 4.3 Namespacing and collisions

| Kind | Public name |
|---|---|
| Broker tool | `status`, `block.read`, ... |
| Application tool | `app.<name>.<tool>` |
| Broker resource | `irix7://...` |
| Application resource | `app://<name>/...` |

An application cannot register a name in the broker namespace, and two applications cannot
claim the same name.  The broker keeps the application's advertised schema verbatim; it
never invents a schema for it.

## 5. Request routing

- `tools/list`: the broker queries each live application, validates its response, and
  concatenates the tools with the broker's own.
- `tools/call`: the broker parses the `app.<name>.<tool>` prefix, forwards the call with
  the application's original tool name, and maps progress notifications back to the agent
  request id.
- `resources/list` / `resources/read`: the same prefixing rule applies to `app://` URIs.
- Unknown applications, malformed tool names and unsupported methods return the normal MCP
  error object; they do not fall through to another application.
- The broker caps response size and applies a per-call timeout.  A cancelled agent request
  sends `notifications/cancelled` to the application and abandons a late response.
- Streaming tools use SSE from the broker to the agent; the application's notifications are
  the source.

Application results are returned unchanged except for the namespace mapping.  A failed
application call is `isError: true`, not a transport failure, unless the application
connection itself is gone.

## 6. Security

- The LAN bearer token authenticates only the agent to the broker.  It is never forwarded
  to applications.
- Each application receives a capability scoped to the tools it may serve; the broker
  enforces the capability on every forwarded call.
- Applications do not get raw kernel memory, MMIO, DMA or arbitrary root by default.  Those
  remain broker tools with explicit capabilities and audit.
- The broker rate-limits per application and per session; it rejects registrations that
  exceed fixed endpoint or tool limits.
- An application exit, crash or endpoint close removes its tools atomically.  No stale tool
  may remain callable.
- The broker never calls application code in kernel mode.  The boundary is always a local
  IPC message with a bounded parser.

## 7. Kernel-image bridge (temporary)

Until native cells exist, a loadable kernel image can act as a test application.  It is
still kernel code, so this bridge only tests the broker path; it is not the userland
contract.  A future ABI may append a registration service with a static descriptor:

```c
struct mcp_endpoint {
    const char *name;
    const char *version;
    const char *schema_json;
    int (*call)(uint32_t tool, const uint8_t *args, size_t args_len,
                uint8_t *out, size_t out_len);
    void *context;
};
```

This is a bring-up bridge only.  It executes loadable kernel code inside the kernel, so it
cannot be the userland contract and must not be exposed to native cells.  The first
implementation of this document should preferably use a kernel image that speaks MCP over a
local endpoint, so the broker path is exercised exactly as a real application will exercise
it.  The target broker itself moves to a de-privileged native cell in stage 4; until then it
is also kernel code.

## 8. Implementation stages

1. **Broker skeleton** — move the MCP HTTP/JSON-RPC front end into a broker cell/module with
   a fixed endpoint table and no application registration.  Keep the existing tools.
2. **Local MCP client** — add a 9P or local-socket MCP client to the broker and a tiny
   application harness; register one test application and forward `tools/list`/`tools/call`.
3. **Lifecycle and security** — dynamic register/unregister, capabilities, quotas, timeouts,
   cancellation, and tool removal on exit.
4. **Native cell** — run the broker as a de-privileged native cell with full root in v1 and
   per-application capabilities; connect native applications through the native IPC ABI.
5. **IRIX compatibility** — provide a small MCP shim for o32 binaries so they can expose
   tools without knowing the native IPC details.

## 9. Open questions

- Which local IPC is available first: 9P over a cell channel, a Unix socket, or a
  broker-owned shared-memory ring?
- Does an application advertise capabilities or does the broker grant them from a static
  manifest?
- How are tool names stable across application restarts and version changes?
- What is the maximum schema size and tool count per application on a 32 MB machine?
- Should the broker expose applications as separate MCP servers (federation) as well as
  aggregating them under `app.*`?

## 10. Example flow

```text
1. editor starts and opens /run/mcp/editor.sock.
2. editor sends mcp.register {name:"editor", endpoint:"/run/mcp/editor.sock", ...}.
3. broker connects, sends initialize, then tools/list.
4. agent sends tools/list; broker returns app.editor.open and app.editor.save
   alongside status, fs.read and the other broker tools.
5. agent sends tools/call app.editor.open {path:"/etc/motd"}.
6. broker forwards tools/call open {...} to the editor endpoint.
7. editor streams notifications/progress; broker relays them as SSE.
8. editor returns content; broker returns it with the agent's request id.
9. editor exits; broker drops the endpoint and app.editor.* from tools/list.
```
