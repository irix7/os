# Seeq receive interrupt and IP2 dispatch

**Status:** implemented
**Date:** 2026-11-03
**Issues:** [#336](https://github.com/irix7/os/issues/336), [#248](https://github.com/irix7/os/issues/248), audit [#330](https://github.com/irix7/os/issues/330)
**Related:** [M3-ADR-0004](../adr/m3-0004-event-driven-io.md)

## 1. What was missing

The trap vector treated every `EXC_INT` as the timer: it acknowledged Compare,
added one to `TICKS` and returned.  The Seeq receive interrupt was not enabled
(`RX_CMD` had station/broadcast/multicast matching only, every descriptor was
armed without `XIE`, and the IOC's local-0 Ethernet mask was never written), so
`Status.IM2` stayed masked.  A first mesh request after an idle period could
only be noticed by the 10 ms tick.

## 2. The path

```
Seeq frame → ENET_INT → HPC3 INTSTAT.ENET_DEV (bit 4)
           → IOC local 0 bit 3 → CPU IP2 (Cause bit 10)
trap_vector (0x8000_0180)
  ExcCode != 0            → trap_entry (full frame)
  ExcCode == 0, IP7 set   → timer fast path: Compare = Count+330000,
                            TICKS++, clear LL, eret
  ExcCode == 0, IP7 clear → trap_entry (full frame)
trap_handler
  EXC_INT → net_irq::classify(cause)
            External (IP2) → net::interrupt()
            anything else  → counted, no Compare or TICKS write
net::interrupt()                       [interrupt context, EXL=1]
  read istat0 (+0x3_0000) and istat1 (+0x3_000C), merge bits 4:0 / 9:5
  no Ethernet bit            → spurious counter, return
  read Seeq RX status (reg 6), then TX status (reg 7)
  write CLRINT to HPC3 ENET_RX_RESET (+0x1_5014)
  W1C the ENET RX/TX DMA bits (never the device bit)
  set one deferred flag
  if the source survives the ack and ENET_RX_CTRL.RBO is set → mask IM2,
      record "parked" (drain rebuilds the ring and re-arms)
idle loop                             [thread context]
  net::take_pending() → net::drain(8) → net::poll_egress → module::poll
  module::poll wakes only the region that owns a ready slot
```

`net=poll` remains the path when the environment sets `net.rx=off`, when the
boot self-test fails, and when the handler parks a source it cannot clear.

### 2.1 Why the device bit is never written

The first implementation also wrote the `INTSTAT.ENET_DEV` bit back as a W1C.
That is wrong in both the model and the documented register: the device bit is
the Seeq's level-triggered pin, latched by the HPC3, and only `CLRINT` drops
it.  A frame that raised the pin between `CLRINT` and the W1C left IP2
asserted with a clear status bit.  The next entry then found no source, so it
counted a spurious interrupt and returned without acknowledging anything: the
kernel livelocked at about 225,000 interrupts/s.  The fix is that `ENET_W1C`
excludes the device bit; the ack for it is `CLRINT`.

The general backstop is a 64-entry undrained streak, reset by every thread-side
frame service.  It cannot trigger on real traffic; it exists so an unforeseen
unackable source parks the line instead of livelocking.  The TLS client pumps
the interface through raw pointers, so `poll_raw` is a thread-side service
too: an early version reset the streak only in `poll` and `drain`, and a long
HTTPS boot fetch hit the backstop, masked `IM2` and rebuilt the receive ring
with no overflow.  The boot transcript's `network: rx ring reset ctrl=0x2b0`
line is how that was found; the final kernel reports `parked=0` and no reset.

### 2.2 Handler rules

Interrupt context does exactly four things: read the split HPC3 status,
acknowledge the Seeq and HPC3, set one atomic flag, and (only for a source it
cannot clear) mask `IM2`.  There is no allocation, console output, smoltcp
call or module call.  `TRY` for a nested exception is impossible because
`EXL` stays set; `wait_from_handler` therefore masks every device interrupt
for its one-instruction `wait` window, because a nested trap would overwrite
the single static `TRAP_FRAME`.

### 2.3 Partial wake, bounded drain

`module::poll` builds a per-slot readiness mask and maps each slot to the
module region that created it (`CURRENT_REGION` around entry and tick calls).
Only that region runs early; every region still runs on its 10 ms deadline, so
join/route/lease/migration timers are unchanged.  The interrupt wake takes at
most one eight-descriptor ring per pass; the line stays asserted and the next
pass takes the rest.

`EV_TX` is not a wake source.  It means "a send was short and there is room
now", which stays true until the next send; as a level wake it kept an idle
node on the 0.5 ms observation forever.  The peer's acknowledgment arrives as
a frame, so the receive interrupt already wakes the owner when transmit space
returns.  With the interrupt armed the idle loop always uses the 10 ms `wait`
and never `wait_short`: that is what makes the idle load fall to 1 %.

## 3. Measurements

Host: 6-core x86_64, iris `v0.1.13+irix7`, two kernels through the M2 host
relay, 64 hot samples at a 20 ms gap and 48 cold samples at a 250 ms gap per
direction, median of three series.  Load average 2.6-3.3 during the runs.

| arm | hot p50 | hot p95 | hot p99 | cold p50 | cold p95 | cold p99 | idle passes / 20 s | idle |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `net.rx=off` (poll only) | 2 ms | 3 ms | 7–9 ms | **10 ms** | 20 ms | 20 ms | 2,657 | 1 % |
| receive interrupt | 2 ms | 2–11 ms | 3–12 ms | **2 ms** | 2 ms | 2 ms | 2,010 | 1 % |

The poll arm's cold distribution is the tick: p50 10 ms, and its floor (2–4 ms)
is only reachable when a sample happens to land near a tick boundary.  The
interrupt arm's cold p50 is 2 ms in both directions and its p95 and p99 are
both 2 ms; a repeat of the same gate measured cold p50 1 ms and p99 10–12 ms
(one host deschedule in the upstream node), still inside the gate's 25 ms
bound.  Idle is about 2,010 passes in 20.0 s, one per tick, with 1 % of counted
time in the loop body; the poll arm takes 2,657 passes because its 0.5 ms
observation stays active.

Interrupt counters moved from 430 to 2,766 across the run with
`spurious=0 unexpected=2 parked=0 armed=true`.

An earlier run of the same gate at load average 7.8 measured hot p50 2 ms,
hot p95 4–10 ms, hot p99 8–21 ms, cold p50 2 ms, cold p95 6–7 ms, cold p99
11–14 ms and 1 % idle.  A single `unexpected` is a level line that dropped
between the exception and the handler's `Cause` read; it is counted, not
fatal.

Across eight runs while another job shared the six cores, cold p50 moved from
1 ms to 16 ms as the load moved from 2.6 to 13.7.  The mechanism's own floor
stayed at 1 ms in every run, including the loaded ones; the percentiles above
p50 are host scheduling, which is why the gate repeats each series and takes
the median.

Gate: `tools/mesh-irq-gate.sh` (host tests + `m2-rig irq-gate`), and
`M2_IRQ_GATE_COMPARE=1` adds the polled control arm.

## 4. Limits

- **No real Indy was available.**  The split `istat0`/`istat1` merge and the
  Seeq/`CLRINT` ordering follow the PROM driver and the hardware notes, but the
  HPC3 bug is modelled as a mirror in iris, so the two-register workaround is
  exercised, not proven against the hardware.
- **The receive-buffer-overflow recovery is not exercised by normal traffic.**
  A frame that overruns its descriptor sets `ENET_RX_CTRL.RBO`; `CLRINT` cannot
  clear it, so the handler masks `IM2` and the drain rebuilds the ring.  The
  path exists and is bounded, but no run reached it.
- **The host scheduling tail is not a kernel property.**  On the shared test
  host, cold p50 moved between 2 ms and 16 ms across runs as the load moved
  between 3.8 and 13.7.  The gate repeats each series three times and takes the
  median; the p95/p99 thresholds are deliberately loose, and the hot p99 is
  reported rather than gated.
- **One handler, one line.**  Only IP2/Ethernet is claimed.  A second IP2
  source (SCSI, the IOC's mappable bank) still needs its own `Cause`/line
  decode and must not be acknowledged by the Seeq handler; the IOC masks only
  the Ethernet bit, so this is a design limit, not a latent bug.
- **Slot A is nearly full.**  The module image is 418,568 bytes and its `.bss`
  plus image is within a few hundred bytes of the 512 KiB slot A limit; adding
  512 bytes of latency samples failed the slot check.  Larger measurements
  need a slot or arena change, not another array.
