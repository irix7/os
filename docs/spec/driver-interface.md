# IRIX 7 — Driver Interface (`ctl` / `data` / `status`)

**Status:** draft
**Version:** 1.0
**Owner:** drivers
**Related:** [`architecture.md`](architecture.md) §9, [`../plan/milestones.md`](../plan/milestones.md) M2–M4.

## 1. Purpose

Every IRIX 7 driver is a de-privileged cell that exposes the same three files:

```text
/dev/<class>/<instance>/
  ctl      ASCII verbs, one per write
  data     the byte stream or packet stream
  status   read-only key=value state
```

This is the single most valuable Plan 9 interface idea that survives in IRIX 7 (D31). It
makes a driver scriptable, testable and MCP-wrappable without a new ABI per device. The
legacy IRIX `ioctl` surface maps onto `ctl`; `read`/`write` map onto `data`; `stat` maps onto
`status`.

## 2. Capabilities

A driver cell receives only the capabilities it needs:

| Capability | Use |
|---|---|
| `Device` | Identifies the device instance; carries the MMIO range, IRQ and DMA window rights |
| `Irq` | Receives interrupt events on the cell's `Port` |
| `DmaWindow` | A physically contiguous, cache-managed DMA buffer |
| `Mmio` | A mapped, uncached register window |

The driver never programs the DMA TLB, the interrupt controller or the CPU's exception
vectors. The framework does that as part of the capability grant.

## 3. `ctl`

- Write-only by default; each write is one ASCII command terminated by `\n`.
- The command is parsed case-insensitively; arguments are whitespace-separated.
- A successful write returns the number of bytes consumed.
- A failed write returns `EINVAL`/`EIO`/`EBUSY` with a message; the driver state is unchanged
  where possible.
- Reads from `ctl` return the last result as `ok\n` or `E<code> <message>\n`.
- Commands are serialised: one writer at a time; concurrent writers get `EBUSY`.

### 3.1 Common verbs

| Verb | Meaning |
|---|---|
| `start` | Start the device (DMA, interrupts, engine) |
| `stop` | Stop the device gracefully; drain or abort per the device |
| `reset` | Hardware reset; returns to the post-probe state |
| `info` | Print a one-line description |
| `version` | Print the driver/interface version |
| `selftest` | Run the device's built-in test, if any |
| `power on` / `power off` | Optional; only where the hardware supports it |

### 3.2 Device-specific examples

| Device | Verbs |
|---|---|
| SCSI (`/dev/scsi/0`) | `target <id>`, `lun <n>`, `read <lba> <count>`, `write <lba> <count>`, `sync`, `rescan` |
| Seeq (`/dev/net/0`) | `mac <aa:bb:...>`, `promisc on/off`, `mtu <n>`, `link`, `stats clear` |
| HAL2 (`/dev/audio/0`) | `rate <hz>`, `channels <n>`, `format <s16be>`, `gain <0-100>`, `loopback on/off`, `capture start/stop` |
| Newport (`/dev/gfx/0`) | `mode <w>x<h>x<bpp>`, `palette <index> <rgb>`, `cursor <x> <y>`, `blank on/off` |
| Vino (`/dev/video/0`) | `standard ntsc/pal`, `source camera/test/black/off`, `capture start/stop`, `channel a/b` |
| Serial (`/dev/serial/0`) | `baud <n>`, `parity none/even/odd`, `stopbits 1/2`, `flow none/rtscts` |

The verbs are per-driver; the common lifecycle verbs are mandatory.

## 4. `data`

- A byte stream for character devices (serial, audio, video) and a packet/block stream for
  SCSI, Ethernet and block devices.
- Read semantics:
  - character devices: return up to the requested length; block until data or timeout;
  - packet devices: one read returns one packet/frame; a short buffer truncates and sets
    `EMSGSIZE`;
  - block devices: reads are block-aligned; unaligned requests return `EINVAL`.
- Write semantics:
  - character devices: accept up to the requested length; partial writes are allowed;
  - packet devices: one write is one packet; the driver may reject oversized frames;
  - block devices: block-aligned; `sync` via `ctl`.
- Blocking: reads may block; a non-blocking mode is set with `ctl nonblock on` where the
  device supports it.
- Zero-copy: a native client may pass a `VMAR`/`VMO` buffer; the legacy path copies through
  the compat layer.
- Ownership: one reader and one writer per `data` file; additional opens get `EBUSY` unless
  the driver explicitly supports fan-out.

## 5. `status`

Read-only ASCII, one `key=value` per line, stable keys:

```text
state=ready
version=1.0
driver=wd33c93
instance=0
irq=5
dma=on
errors=0
irqs=1234
last_error=
rx_packets=0
tx_packets=0
dma_bytes=0
```

Device-specific keys are added after the common ones. Values are decimal, hex (`0x...`) or
`on`/`off`. A missing key means "not applicable". The file is cheap to read and does not
block.

## 6. Errors

| Code | Meaning |
|---|---|
| `EINVAL` | Invalid argument or verb |
| `EBUSY` | Device busy or already open |
| `EIO` | Device I/O failure |
| `ENODEV` | Device disappeared |
| `ENOSPC` | Buffer or ring full |
| `ETIMEDOUT` | Operation timed out |
| `EOPNOTSUPP` | Verb not supported by this device |
| `EMSGSIZE` | Packet/frame too large |

Errors are returned as `E<CODE> <message>` on `ctl` and mapped to the legacy `errno` by the
compat layer. `data` read/write errors use the same codes in the legacy path.

## 7. Lifecycle

```text
probe → init → start → (running) → stop → reset → remove
                  │                    │
                  └── error ───────────┘
```

- **probe**: the framework identifies the device and grants capabilities.
- **init**: the driver maps MMIO, allocates DMA windows, registers `ctl`/`data`/`status`.
- **start**: the driver enables interrupts/DMA and moves to `state=running`.
- **stop**: quiesce, disable interrupts/DMA, flush.
- **reset**: hardware reset; return to `state=ready`.
- **remove**: release capabilities; the framework revokes them.

The driver is a cell; a fault is a kernel fault in v1 (the restartable-driver model is a
future goal). The framework may revoke capabilities and restart the cell once that model
exists.

## 8. Interrupts

The framework posts an event to the driver's `Port`:

```rust
struct IrqEvent {
    irq: u32,
    status: u32,
    timestamp: u64,
}
```

The driver reads the event, services the device, acknowledges the source through the `Irq`
capability, and re-arms if necessary. The top half never calls into the driver; it only posts
the event. A driver that cannot keep up gets `ENOSPC` on its `Port` and the framework
coalesces events.

## 9. DMA rules

1. The driver requests a `DmaWindow` from the framework: size, alignment, direction.
2. The framework allocates physically contiguous memory, programs the VDMA DMA TLB where
   possible, and returns the physical address, virtual address and a handle.
3. Before handing the buffer to the device, the driver calls `sync_for_device` (flush
   D-cache, `sync`).
4. After the device signals completion, the driver calls `sync_for_cpu` (invalidate
   D-cache).
5. The driver never dereferences the physical address; it uses the mapped virtual address.
6. The driver never programs the DMA TLB directly.
7. The buffer is freed only after the device is stopped and the framework has revoked the
   window.

The HPC3 PBUS descriptor count is 14 bits (16,383 bytes per descriptor); larger transfers
are split into a descriptor chain. Descriptors are 8-byte aligned; endianness is big-endian
and fixed at boot.

## 10. Legacy mapping

| IRIX operation | IRIX 7 mapping |
|---|---|
| `open("/dev/...")` | Opens the `ctl`/`data`/`status` files |
| `ioctl(fd, cmd, arg)` | Writes the corresponding `ctl` verb; reads the result |
| `read(fd, buf, n)` | Reads `data` |
| `write(fd, buf, n)` | Writes `data` |
| `fstat(fd)` | Reads `status` |
| `select`/`poll` | Waits on the `data` readiness event |

The compat layer owns the mapping table; the driver does not need an IRIX-specific ABI.

## 11. Versioning

- The interface version is `1.0`; it is reported by `ctl version` and `status version`.
- New verbs are additive; removing or changing a verb is a new major version.
- New `status` keys are additive; clients ignore unknown keys.
- A driver may support a subset of the common verbs; unsupported verbs return `EOPNOTSUPP`.

## 12. Test plan

- [ ] A mock device exercises every common verb and error path.
- [ ] `ctl` parser fuzzing: long lines, bad arguments, NUL bytes, missing newline.
- [ ] `data` read/write boundaries: zero length, short buffer, oversized packet, partial
      write.
- [ ] `status` is stable and parseable.
- [ ] IRQ event delivery and coalescing.
- [ ] DMA sync correctness on iris (cache model) and a real Indy.
- [ ] Lifecycle: start/stop/reset/remove, repeated 100 times.
- [ ] Legacy `ioctl` mapping for SCSI, Seeq, HAL2, Newport and Vino.
- [ ] A driver fault is contained or panics with a clear diagnostic (v1).

## 13. Open questions

- Whether `data` should support `mmap` for zero-copy packet rings.
- The exact `select`/`poll` event format for legacy clients.
- Whether `status` should be machine-readable JSON as an option.
- The restartable-driver model and its capability revocation semantics.
- Whether the `ctl` grammar should be shared with the MCP device tools.
