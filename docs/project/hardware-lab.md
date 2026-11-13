# IRIX 7 — Hardware Lab

**Status:** living document
**Scope:** the two real Indys, the emulator rig, serial/network/power, storage, spares,
safety and hardware-in-the-loop testing.
**Related:** [`../research/indy-hardware.md`](../research/indy-hardware.md), [`testing.md`](testing.md).

## 1. Machines

Record each machine's exact configuration. The two stock Indys are not identical.

| Field | Indy A | Indy B |
|---|---|---|
| Model / profile | Indy (IP24) | Indy (IP24) |
| CPU | R4400 (confirm speed) | R4400 (confirm speed) |
| RAM | Confirm banks | Confirm banks |
| Graphics board | Confirm (8-bit XL / 24-bit XL / XZ) | Confirm |
| Vino | Present? | Present? |
| Indy Video GIO board | Present? | Present? |
| SCSI | Internal disk? CD-ROM? | Confirm |
| Ethernet | Seeq 8003 | Seeq 8003 |
| PROM version | `hinv` / version | Confirm |
| PSU | Original / recapped? | Confirm |
| RTC battery | Voltage / replaced? | Confirm |
| Notes | Board that Linux may not drive | Confirm |

**First task:** run the PROM `hinv`, photograph the boards, and fill this table. Until then,
the hardware target is the iris default, not the real machines.

## 2. Network

- A dedicated switch or VLAN for the lab; no exposure to the public internet.
- DHCP reservations for each Indy; record MAC, IP and hostname.
- A lab server (any small Linux box) provides:
  - TFTP for the PROM fallback;
  - HTTP for the ROM and kernel/bundle during bring-up;
  - DNS;
  - NTP;
  - an SSH jump host for the MCP client.
- R2 is the production distribution channel; the lab server is the offline mirror.
- Firewall: allow only the lab subnet to reach the MCP port and the serial console.
- Record the network map in the lab notebook.

## 3. Serial console

- USB-to-serial adapter (FTDI/PL2303) per Indy; null-modem cable; DB9/DB25 adapters.
- Settings: 9600 8N1 by default; the PROM may use a different baud.
- `picocom`/`tio`/`screen` on the lab host; log to a file.
- The MCP `serial.read`/`serial.write` tools use the same console.
- Label both ends of every cable.

## 4. Storage

- **BlueSCSI or ZuluSCSI** for SCSI disk/CD emulation; keep the original disks untouched.
- SD cards with known-good images; label and checksum them.
- IRIX 6.5.7m installation media and the captured sysroot; back up to two locations.
- The root image and boot bundle are built on the host and cached on the lab server/R2.
- Never write to a real IRIX disk until the XFS write path is trusted; use read-only images
  and copies.

## 5. Power and remote reset

- A network-controlled smart plug per Indy for hard power cycling.
- A UPS for the lab host and the switch; the Indys may stay on the UPS if capacity allows.
- The DS1386 watchdog is the software reset path; the smart plug is the hardware last resort.
- Label the power switches and record the plug IPs.

## 6. Bring-up checklist

For each machine:

- [ ] Visual inspection: capacitors, battery leakage, dust, cable damage.
- [ ] PSU voltages checked (or PSU recapped by a competent person).
- [ ] RTC battery voltage measured; replace if below spec.
- [ ] RAM SIMMs reseated; parity errors checked.
- [ ] SCSI chain terminated correctly; IDs unique.
- [ ] Serial console connected and logging.
- [ ] PROM `hinv` captured; PROM version recorded.
- [ ] Network link and DHCP verified from the PROM or a known-good OS.
- [ ] Graphics board identified; Vino/Indy Video presence recorded.
- [ ] Backup of NVRAM/environment variables.
- [ ] Machine added to the lab notebook with photos.

## 7. Hardware-in-the-loop

A self-hosted GitHub Actions runner (`indy`) on the lab host:

- connects to the Indy's serial console and network;
- can power-cycle via the smart plug;
- runs `hardware.yml` on `workflow_dispatch` (and nightly when stable);
- uploads serial logs, screen captures and test results as artefacts;
- is never exposed to fork PRs; only `main` and maintainers can trigger it.

The runner is a normal Linux machine; the Indy is the device under test. The runner does not
need to be fast.

## 8. Safety

- **PSU:** original Indy PSUs are old; recapping is a specialist job. Do not open a PSU
  unless qualified. Use a residual-current device (RCD) and a fused outlet.
- **Battery:** the DS1386 contains a lithium cell; do not puncture, heat or short it. Replace
  with the correct part; dispose of it properly.
- **ESD:** wrist strap and grounded mat when handling boards.
- **Heat:** ensure airflow; do not stack machines; check fans.
- **Fire:** do not leave an unattended machine powered for long periods without a smoke
  alarm and a remote cutoff.
- **Data:** assume any old disk is failing; image it before use.

## 9. Spares and procurement

Keep, or record where to get:

- a spare PSU (or a recapped unit);
- spare RAM SIMMs (36-bit parity, matched sets);
- a spare SCSI disk or BlueSCSI/ZuluSCSI;
- a spare RTC battery;
- serial adapters and cables;
- a spare Seeq/SCSI terminator;
- GIO slot blanks and screws;
- a spare fan.

## 10. Compatibility matrix

| Configuration | Status | Notes |
|---|---|---|
| iris default: Indy, R4400, Newport XL, 256 MiB, HAL2, Seeq, WD33C93 | Primary target | The first reference |
| Real Indy A: confirm graphics/RAM | TBD | Fill from `hinv` |
| Real Indy B: confirm graphics/RAM | TBD | One board may not be Linux-compatible |
| 24-bit XL | Supported later | VRAM size undocumented |
| XZ/Extreme | Deferred | Rare; iris has bring-up support |
| Indy Video | Deferred | Required for video output |
| 32 MiB RAM | Not supported for the full stack | Lite mode TBD |
| 64 MiB RAM | Minimum candidate | Confirm during display bring-up |

## 11. Lab notebook

Keep a dated log in `docs/lab/YYYY-MM-DD.md` (or the wiki) with:

- machine, board, serial number;
- what was changed;
- command/output;
- photos/screenshots;
- result and next step.

The notebook is part of the project record; it is how a second person reproduces a result.

## 12. Open questions

- The exact configuration of both real Indys.
- Whether either has the Indy Video board.
- The minimum RAM target for the full stack.
- Whether the lab runner can be always-on or must be manually started.
- The spare-parts budget.
