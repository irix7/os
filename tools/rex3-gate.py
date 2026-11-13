#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Gate the REX3 (Newport) fill path against the engine's own framebuffer.

The kernel's ``gfx`` diagnostic verifies the driver against the REX3's
registers: ``DRAWMODE1`` (rgbmode, planes, depth), the staged colour and
``WRMASK``, and the FIFO drain after a burst.  It cannot verify the pixels,
because Newport has no CPU framebuffer aperture -- the only way to read VRAM
is through the engine, and the PROM window at 0x1F0F_0000 stops at the 8 KiB
register block.  This gate supplies the missing half on the host side:

1. boot the ROM under iris with the Newport board at 1280x1024;
2. run the guest ``gfx`` diagnostic over telnet and require it to pass;
3. cross-check the guest's register readback against iris's independent
   ``rex status`` dump of the same registers;
4. dump the REX3 framebuffer (``rex fbdump``), read the probe rectangle the
   diagnostic leaves painted, and require all three channels to be present;
5. run ``gfx fault``, which clears ``rgbmode`` in the engine and paints the
   probe in colour-index mode, and require the same framebuffer check to
   fail on the faulted pixels -- the negative control that proves the pixel
   check has power over the exact failure mode #118 names;
6. run ``gfx`` again and require the probe to be restored.

Hardware versus iris (recorded, not worked around):

* **FIFO depth.**  The real REX3 GFIFO holds 32 entries and a full queue
  stalls or drops the GIO write; the guest burst is 1024 GO writes, so a real
  Indy must stall the CPU repeatedly.  iris queues 65536 entries, so it
  accepts the whole burst and reports a FIFO level of 0.  The gate therefore
  proves the driver *waits while the engine is busy* (``busy_polls > 0``) and
  that the queue drains; it cannot prove the absence of drops on hardware,
  because iris never drops.  See ``docs/project/testing.md`` section 5.
* **Framebuffer readback.**  iris exposes the framebuffer through the monitor
  (``rex fbdump``); a real Indy has no readback path at all.  On hardware the
  same probe must be checked with a screen grab (camera or video capture) and
  the register half is the only automated part.  That limit is why the guest
  diagnostic reports the probe coordinates and colour: an operator can verify
  them on the monitor.
* **rgbmode failure mode.**  A real REX3 in colour-index mode writes a single
  channel ("only the red channel reaches the framebuffer", the original #118
  symptom).  iris takes the colour index from ``COLORI``/``COLORRED`` and
  stores it as a single-channel word.  The gate accepts any one-channel
  result for the fault fill and requires it to differ from the RGB probe.

Usage:
  tools/rex3-gate.py [--rom PATH] [--work DIR] [--keep]

Environment:
  IRIS       iris binary (default: iris in PATH)
  IRIS_CI    iris-ci binary (default: iris-ci in PATH)
  CARGO      cargo binary (default: cargo); only needed to build the PROM
  CC         host C compiler used by build-std (optional)
"""

import argparse
import os
import re
import shutil
import socket
import struct
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# Console background, drawn by graphics_console::flush before the probe.
CONSOLE_BG = (0x08, 0x0C, 0x14)
# Framebuffer geometry (iris holds 2048x1024 words; the guest draws 1280x1024).
FB_STRIDE = 2048

SASH_PROMPT = b"sash> "
MONITOR_PROMPT = re.compile(rb"(^|\n)> $")


class GateFailure(Exception):
    pass


def log(msg):
    print(f"rex3-gate: {msg}", flush=True)


def free_port():
    """A port the kernel can bind right now (the gate is not a concurrent rig)."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def fail(msg):
    raise GateFailure(msg)


def run(cmd, **kw):
    log("run: " + " ".join(str(c) for c in cmd))
    return subprocess.run(cmd, check=True, **kw)


class Monitor:
    """Line-oriented iris monitor client."""

    def __init__(self, port, timeout=60):
        self.sock = socket.create_connection(("127.0.0.1", port), timeout=timeout)
        self.buf = b""
        self._until_prompt()

    def _until_prompt(self):
        while not MONITOR_PROMPT.search(self.buf):
            chunk = self.sock.recv(65536)
            if not chunk:
                fail("iris monitor closed")
            self.buf += chunk
        out, self.buf = self.buf, b""
        return out.decode("latin-1", "replace")

    def cmd(self, line):
        self.sock.sendall((line + "\n").encode())
        out = self._until_prompt()
        return re.sub(r"\n?> $", "", out).replace("\r", "").strip()

    def close(self):
        self.sock.close()


class Telnet:
    """Guest sash console over the forwarded telnet port."""

    def __init__(self, port, timeout=240):
        # The ROM downloads the kernel first (M1-ADR-0002), so the guest's telnet
        # listener can come up minutes after the gate starts.  The guest's listener
        # comes up with the kernel, slightly after BOOT-READY; a connection made too early is accepted by the NAT
        # forwarder and then closed by the guest stack.  Retry until the sash
        # prompt actually arrives.
        deadline = time.monotonic() + timeout
        while True:
            try:
                self.sock = socket.create_connection(("127.0.0.1", port), timeout=10)
                self.buf = b""
                self.read_until(SASH_PROMPT, timeout=15)
                return
            except (OSError, GateFailure) as exc:
                try:
                    self.sock.close()
                except OSError:
                    pass
                if time.monotonic() > deadline:
                    raise
                log(f"telnet not ready ({exc}); retrying")
                time.sleep(0.5)

    def read_until(self, marker, timeout=180):
        self.sock.settimeout(timeout)
        while marker not in self.buf:
            chunk = self.sock.recv(65536)
            if not chunk:
                fail(f"guest telnet console closed after {len(self.buf)} buffered bytes")
            self.buf += chunk
        out, self.buf = self.buf, b""
        return out.decode("latin-1", "replace")

    def command(self, line, timeout=180):
        log(f"guest: {line}")
        self.sock.sendall(line.encode() + b"\r\n")
        out = self.read_until(SASH_PROMPT, timeout)
        log(f"guest: {line} returned {len(out)} bytes")
        return out

    def close(self):
        self.sock.close()


def parse_kv(line):
    """Parse `key=value` and `key:value` tokens into a dict."""
    out = {}
    for token in line.split():
        for sep in ("=", ":"):
            if sep in token:
                key, value = token.split(sep, 1)
                out[key] = value
                break
    return out


def parse_gfx_report(text):
    for line in text.splitlines():
        m = re.match(r"gfx: self-test (ok|FAILED) (.+)$", line.strip())
        if m:
            report = parse_kv(m.group(2))
            report["ok"] = m.group(1) == "ok"
            return report
    fail("guest did not report a gfx self-test result:\n" + text)


def parse_rex_status(text):
    """Pull the fields the gate cross-checks out of `rex status`."""
    fields = {}
    for line in text.splitlines():
        line = line.strip()
        m = re.match(r"DRAWMODE1\s*:\s*([0-9a-fA-F]+)", line)
        if m:
            fields["drawmode1"] = int(m.group(1), 16)
        m = re.match(
            r"COLORRED\s*:\s*([0-9a-fA-F]+)\s+COLORGRN\s*:\s*([0-9a-fA-F]+)\s+"
            r"COLORBLUE\s*:\s*([0-9a-fA-F]+)",
            line,
        )
        if m:
            fields["colour"] = [int(m.group(i), 16) for i in (1, 2, 3)]
        m = re.match(r"DRAW BUSY\s*:\s*(\S+)\s+GFIFO\s*:\s*(\d+)/(\d+)", line)
        if m:
            fields["busy"] = m.group(1)
            fields["fifo_used"] = int(m.group(2))
        m = re.match(r"GO TOTAL\s*:\s*(\d+)", line)
        if m:
            fields["go_total"] = int(m.group(1))
    return fields


def read_fb_word(path, x, y):
    with open(path, "rb") as f:
        f.seek((y * FB_STRIDE + x) * 4)
        raw = f.read(4)
    if len(raw) != 4:
        fail(f"{path}: short read at ({x}, {y})")
    return struct.unpack(">I", raw)[0]


def fb_word(r, g, b):
    """The REX3 framebuffer word for an RGB fill.

    The engine packs the 24-bit pixel as ``blue << 16 | green << 8 | red``
    (``Rex3Context::get_colori``), so the red channel is the low byte.  A
    colour-index fill of index N stores N in that low byte: the "only the red
    channel" failure mode #118 names.
    """
    return (b << 16) | (g << 8) | r


def word_rgb(word):
    """Split a framebuffer word into (red, green, blue)."""
    return (word & 0xFF, (word >> 8) & 0xFF, (word >> 16) & 0xFF)


class Rig:
    def __init__(self, args):
        self.args = args
        self.work = Path(args.work)
        self.rom = Path(args.rom) if args.rom else self.work / "irix7-rom.bin"
        self.telnet_port = args.telnet_port
        self.monitor_port = args.monitor_port
        self.proc = None
        self.serial = self.work / "serial.log"
        self.out = self.work / "iris.out"

    def prepare(self):
        self.work.mkdir(parents=True, exist_ok=True)
        (self.work / "machine.toml").write_text(
            f"""banks = [32, 0, 0, 0]
no_audio = true

[machine]
profile = "indy_ip24"

[graphics]
board = "newport"
heads = 1
resolution = "1280x1024"

[scsi]

[[port_forward]]
proto = "tcp"
host_port = {self.telnet_port}
guest_port = 23
bind = "localhost"
"""
        )
        # A DS1386 image with a fixed MAC; anything else is left blank so the
        # kernel rewrites it, as on a fresh machine.
        nvram = bytearray(8192)
        nvram[314:320] = bytes([0x02, 0x11, 0x22, 0x33, 0x44, 0x55])
        (self.work / "nvram.bin").write_bytes(nvram)
        for stale in ("iris.sock", "serial.log", "iris.out"):
            p = self.work / stale
            if p.exists():
                p.unlink()

    def build_rom(self):
        if self.rom.exists() and not self.args.rebuild:
            log(f"using PROM {self.rom}")
            return
        cargo = os.environ.get("CARGO", "cargo")
        if shutil.which(cargo) is None:
            fail(f"{cargo} is not in PATH; set CARGO or pass --rom")
        target = "targets/mips3-sgi-irix7-n32.json"
        env = dict(os.environ)
        env["CARGO_TARGET_DIR"] = str(self.work / "target")
        env["CARGO_PROFILE_RELEASE_OPT_LEVEL"] = "z"
        env["CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS"] = (
            "-Clink-arg=-Tkernel/linker.ld"
        )
        run(
            [cargo, "build", "--release", "-Z", "build-std=core,alloc",
             "-Zjson-target-spec", "--features", "cross", "--target", target, "-p", "kernel"],
            cwd=REPO, env=env,
        )
        env["CARGO_TARGET_MIPS3_SGI_IRIX7_N32_RUSTFLAGS"] = "-Clink-arg=-Trom/linker.ld"
        run(
            [cargo, "build", "--release", "-Z", "build-std=core,alloc",
             "-Zjson-target-spec", "--features", "cross", "--target", target, "-p", "rom"],
            cwd=REPO, env=env,
        )
        rom_bin = self.work / "target" / "mips3-sgi-irix7-n32" / "release" / "rom"
        run(
            [sys.executable, str(REPO / "rom/tools/make_prom.py"), str(rom_bin), str(self.rom)],
            cwd=REPO,
        )
        log(f"built PROM {self.rom} ({self.rom.stat().st_size} bytes)")

    def start(self):
        iris = self.args.iris
        iris_ci = self.args.iris_ci
        self.out.write_text("")
        env = dict(os.environ)
        env.setdefault("IRIS_NO_IDLE", "1")
        log_cmd = [
            iris, "--config", str(self.work / "machine.toml"), "--ci",
            "--prom", str(self.rom),
            "--nvram", str(self.work / "nvram.bin"),
            "--ci-socket", str(self.work / "iris.sock"),
            "--monitor-port", str(self.monitor_port),
            "--serial-log", str(self.serial),
        ]
        log("run: " + " ".join(log_cmd))
        # iris keeps its instance lock, NVRAM default and screenshots relative
        # to the process working directory; run it in the work directory so two
        # gates cannot fight over the repository's own iris.lock.
        with open(self.out, "w") as out:
            self.proc = subprocess.Popen(
                log_cmd, env=env, cwd=self.work, stdout=out, stderr=subprocess.STDOUT
            )
        deadline = time.monotonic() + 30
        while True:
            if self.proc.poll() is not None:
                self.tail()
                fail(f"iris exited ({self.proc.returncode}) before the control socket appeared")
            probe = subprocess.run(
                [iris_ci, "--socket", str(self.work / "iris.sock"), "status", "--wait-ready", "5"],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            )
            if probe.returncode == 0:
                break
            if time.monotonic() > deadline:
                self.tail()
                fail("iris control socket did not become ready")
            time.sleep(0.25)
        run([iris_ci, "--socket", str(self.work / "iris.sock"), "start"], stdout=subprocess.DEVNULL)
        # Readiness through the CI socket, not the serial-log file: the file
        # is a mirror a second sink can reopen, so tailing it is not a reliable
        # readiness signal.  The guest's telnet prompt is checked separately by
        # the Telnet session, which retries until the kernel is in its idle loop.
        # `m1-post` is printed by the first idle-loop pass, after the guest has
        # returned from the SCSI userelf attempt.  Waiting for it means the
        # kernel is servicing the network and telnet, so the first connection
        # below is not accepted by the NAT forwarder while the guest is still
        # busy loading (which would time out and leave the connection half
        # open).
        for marker in ("BOOT-READY", "m1-post"):
            boot = subprocess.run(
                [iris_ci, "--socket", str(self.work / "iris.sock"), "serial-wait",
                 marker, "--timeout", "240"],
                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
            )
            if boot.returncode != 0:
                self.tail()
                fail(f"guest never printed {marker}: "
                     f"{boot.stderr.decode(errors='replace').strip()}")

    def stop(self):
        if self.proc is None or self.proc.poll() is not None:
            return
        try:
            run([self.args.iris_ci, "--socket", str(self.work / "iris.sock"), "quit"],
                stdout=subprocess.DEVNULL)
        except subprocess.CalledProcessError:
            self.proc.kill()
        try:
            self.proc.wait(timeout=30)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait(timeout=10)

    def tail(self):
        for name in (self.serial, self.out):
            p = Path(name)
            if p.exists():
                lines = p.read_text(errors="replace").splitlines()[-20:]
                print(f"--- {p} (tail)\n" + "\n".join(lines), file=sys.stderr)


def dump_framebuffer(mon, work, name):
    out = work / name
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    reply = mon.cmd(f"rex fbdump {out}")
    if "framebuffer dump written" not in reply:
        fail(f"rex fbdump failed: {reply}")
    return out / "rgb.bin"


def probe_points(probe):
    """Corners and centre of the probe rectangle."""
    x, y, w, h = probe["x"], probe["y"], probe["w"], probe["h"]
    return [
        (x, y),
        (x + w - 1, y),
        (x, y + h - 1),
        (x + w - 1, y + h - 1),
        (x + w // 2, y + h // 2),
    ]


def probe_centre(probe):
    return (probe["x"] + probe["w"] // 2, probe["y"] + probe["h"] // 2)


def wait_for_screen(mon, rig, probe, check, what, attempts=20, delay=0.25):
    """Dump the framebuffer until the console has repainted and `check` passes.

    The tell-tale is the console background at (4, 500): it is only the console
    background after a full repaint.  Before that the burst's own fills still
    own the screen, and a dump taken then would mistake the burst band for the
    probe.  Every dump is a fresh file, so a torn read simply fails the retry.
    """
    last = None
    for attempt in range(attempts):
        fb = dump_framebuffer(mon, rig.work, f"dump-{attempt}")
        bg = read_fb_word(fb, 4, 500)
        if bg == fb_word(*CONSOLE_BG):
            ok, detail = check(fb)
            if ok:
                return fb, detail
            last = detail
        else:
            last = f"console background is {word_rgb(bg)}, not repainted yet"
        time.sleep(delay)
    fail(f"{what}: {last}")


def probe_state(fb, probe, expect):
    words = {pt: read_fb_word(fb, *pt) for pt in probe_points(probe)}
    if all(word == expect for word in words.values()):
        return True, words
    return False, ", ".join(f"{pt}={word_rgb(w)}" for pt, w in words.items())


def fault_state(fb, probe):
    """The faulted fill must be a one-channel colour, not the RGB probe."""
    word = read_fb_word(fb, *probe_centre(probe))
    r, g, b = word_rgb(word)
    lit = sum(1 for c in (r, g, b) if c != 0)
    if word == fb_word(probe["r"], probe["g"], probe["b"]):
        return False, f"colour-index fill produced the RGB probe colour {word_rgb(word)}"
    if lit > 1:
        return False, f"colour-index fill lit {lit} channels (rgb {r},{g},{b}); expected at most one"
    return True, word


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--iris", default=os.environ.get("IRIS", "iris"))
    ap.add_argument("--iris-ci", default=os.environ.get("IRIS_CI", "iris-ci"))
    ap.add_argument("--rom", default=os.environ.get("ROM"))
    ap.add_argument("--work", default=str(REPO / "target" / "rex3-gate"))
    ap.add_argument("--telnet-port", type=int, default=0, help="guest console forward (default: free)")
    ap.add_argument("--monitor-port", type=int, default=0, help="iris monitor (default: free)")
    ap.add_argument("--rebuild", action="store_true", help="rebuild the PROM even if it exists")
    ap.add_argument("--keep", action="store_true", help="leave iris running")
    args = ap.parse_args()
    if args.telnet_port == 0:
        args.telnet_port = free_port()
    if args.monitor_port == 0:
        args.monitor_port = free_port()
        while args.monitor_port == args.telnet_port:
            args.monitor_port = free_port()

    for name in ("iris", "iris_ci"):
        path = getattr(args, name)
        if shutil.which(path) is None and not Path(path).is_file():
            fail(f"{path} is not available; set IRIS/IRIS_CI or pass --{name.replace('_', '-')}")

    rig = Rig(args)
    rig.prepare()
    rig.build_rom()

    mon = None
    tel = None
    failures = []
    try:
        rig.start()
        mon = Monitor(args.monitor_port)
        tel = Telnet(args.telnet_port)

        before = parse_rex_status(mon.cmd("rex status"))
        if "go_total" not in before:
            fail("rex status did not report GO TOTAL")

        # 1. Guest diagnostic.
        out = tel.command("gfx")
        report = parse_gfx_report(out)
        if not report["ok"]:
            fail(f"guest gfx self-test failed: {out}")
        if report["rgbmode"] != "set":
            fail(f"guest read rgbmode={report['rgbmode']} from the engine")
        mode = int(report["mode"], 16)
        burst = int(report["fifo"].split(",")[0].split(":")[1])
        fifo = parse_kv(report["fifo"].replace(",", " "))
        if int(fifo["busy"]) < 1:
            fail("the FIFO wait never saw the engine busy; the burst did not need a wait")
        if int(fifo["level"]) != 0:
            fail(f"FIFO level {fifo['level']} after the drain")
        if int(fifo["waits"]) < 1:
            fail("the burst did not wait for the engine")

        probe = {
            "x": int(report["probe"].split(",")[0]),
            "y": int(report["probe"].split(",")[1]),
            "w": int(report["probe"].split(",")[2]),
            "h": int(report["probe"].split(",")[3]),
            "r": int(report["rgb"].split(",")[0], 16),
            "g": int(report["rgb"].split(",")[1], 16),
            "b": int(report["rgb"].split(",")[2], 16),
        }
        expect = fb_word(probe["r"], probe["g"], probe["b"])

        # 2. Cross-check the guest's engine readback against iris's own view.
        after = None
        for _ in range(40):
            after = parse_rex_status(mon.cmd("rex status"))
            if after.get("drawmode1") == mode and after.get("colour") == [
                probe["r"] << 11,
                probe["g"] << 11,
                probe["b"] << 11,
            ]:
                break
            time.sleep(0.1)
        else:
            fail(
                "guest readback disagrees with iris: "
                f"guest mode=0x{mode:08x} colour={report['colour']}, "
                f"iris mode={after.get('drawmode1')} colour={after.get('colour')}"
            )
        if after.get("busy") != "no" or after.get("fifo_used") != 0:
            fail(f"engine not idle after the burst: {after}")
        go_delta = after["go_total"] - before["go_total"]
        if go_delta < burst:
            fail(f"engine executed {go_delta} GOs, fewer than the {burst}-command burst")

        # 3. The engine's pixels: all three channels, at five points.
        fb, words = wait_for_screen(
            mon, rig, probe,
            lambda f: probe_state(f, probe, expect),
            f"probe never reached rgb{word_rgb(expect)}",
        )
        log(f"probe {probe['x']},{probe['y']} {probe['w']}x{probe['h']} = "
            f"rgb{word_rgb(expect)} at {len(words)} points")

        # 4. Negative control: clear rgbmode in the engine and require the
        #    same pixel check to see a single-channel fill.
        fault_out = tel.command("gfx fault")
        if "gfx: fault-injection detected" not in fault_out:
            fail(f"fault injection was not detected by the guest:\n{fault_out}")
        _, fault_word = wait_for_screen(
            mon, rig, probe,
            lambda f: fault_state(f, probe),
            "fault window never reached the framebuffer",
        )
        log(f"rgbmode cleared -> pixel rgb{word_rgb(fault_word)} "
            f"(single channel), not the RGB probe")

        # 5. The normal path restores the probe after the fault window.
        out = tel.command("gfx")
        report = parse_gfx_report(out)
        if not report["ok"]:
            fail(f"gfx did not recover after the fault window:\n{out}")
        wait_for_screen(
            mon, rig, probe,
            lambda f: probe_state(f, probe, expect),
            "probe was not restored after the fault window",
        )
        log("probe restored after the fault window")

        print()
        print("rex3-gate: PASS")
        print(f"  guest:   mode=0x{mode:08x} rgbmode=set colour={report['colour']}")
        print(f"  fifo:    burst={burst} waits={fifo['waits']} polls={fifo['polls']} "
              f"busy_polls={fifo['busy']} max_level={fifo['level']}")
        print(f"  iris:    GO TOTAL +{go_delta}, GFIFO 0/… used, draw busy no")
        print(f"  pixels:  rgb{word_rgb(expect)} at 5 points; "
              f"fault rgb{word_rgb(fault_word)} at the centre")
        print("  limits:  iris queues 64 K commands, so it cannot show drops that a "
              "32-entry hardware FIFO would cause;")
        print("           a real Indy has no framebuffer readback, so only the register "
              "half is automated there.")
    except GateFailure as exc:
        failures.append(str(exc))
        print(f"rex3-gate: FAILED: {exc}", file=sys.stderr)
        rig.tail()
    finally:
        if tel is not None:
            tel.close()
        if mon is not None:
            mon.close()
        if args.keep:
            log("leaving iris running (--keep)")
        else:
            rig.stop()

    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
