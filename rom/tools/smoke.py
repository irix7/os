#!/usr/bin/env python3
"""Smoke-test the ROM/kernel under iris.

Starts iris with the shipped rig configuration, releases the CPU (iris starts
stopped when driven this way), waits for DHCP, then drives the kernel's telnet
console and prints the replies.

Usage:
    rom/tools/smoke.py --iris /path/to/iris --prom /tmp/rom.bin

The script expects to run from a directory where iris can find its disk
images, or pass --cwd.  It needs no Python packages.
"""

import argparse
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time


def wait_for(path, needle, timeout):
    """Wait until `needle` appears in `path`."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with open(path, "r", errors="replace") as f:
                text = f.read()
            if needle in text:
                return text
        except FileNotFoundError:
            pass
        time.sleep(0.2)
    return None


def monitor(port, command, timeout=5.0):
    """Send one monitor command and return the reply."""
    with socket.create_connection(("127.0.0.1", port), timeout=timeout) as s:
        s.settimeout(timeout)
        s.sendall(command.encode() + b"\r\n")
        out = b""
        while True:
            try:
                chunk = s.recv(4096)
                if not chunk:
                    break
                out += chunk
            except socket.timeout:
                break
    return out.decode(errors="replace")


def wait_monitor(port, command, timeout=30.0, proc=None):
    """Send one monitor command once iris is accepting connections."""
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        if proc is not None and proc.poll() is not None:
            raise RuntimeError(f"iris exited with status {proc.returncode}")
        try:
            return monitor(port, command)
        except OSError as error:
            last = error
            time.sleep(0.25)
    raise RuntimeError(f"iris monitor on port {port} did not answer: {last}")


def wait_ready(path, timeout=90.0):
    """Return the boot marker line, or raise with a NOT-READY diagnostic."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with open(path, "r", errors="replace") as f:
                text = f.read()
        except FileNotFoundError:
            text = ""
        if "BOOT-READY v=1 " in text:
            return text
        if "BOOT-NOT-READY v=1 " in text:
            line = next(
                (line for line in text.splitlines() if line.startswith("BOOT-NOT-READY")),
                "BOOT-NOT-READY (line lost)",
            )
            raise RuntimeError(f"guest reported not ready: {line}")
        time.sleep(0.2)
    raise RuntimeError("guest did not emit BOOT-READY within the timeout")


def stop_iris(proc):
    """Terminate iris and wait for the process to disappear."""
    if proc is None or proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            print(f"warning: iris pid {proc.pid} did not exit", file=sys.stderr)


def telnet(port, commands, settle=0.8, timeout=3.0):
    """Connect, run commands, and return the transcript."""
    with socket.create_connection(("127.0.0.1", port), timeout=timeout) as s:
        s.settimeout(timeout)
        time.sleep(settle)
        transcript = ""
        for command in commands:
            s.sendall(command.encode() + b"\r\n")
            time.sleep(settle)
            out = b""
            while True:
                try:
                    chunk = s.recv(8192)
                    if not chunk:
                        break
                    out += chunk
                except socket.timeout:
                    break
            transcript += out.decode(errors="replace")
        return transcript


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--iris", required=True, help="iris binary")
    parser.add_argument("--prom", required=True, help="raw PROM image")
    parser.add_argument(
        "--cwd",
        default=os.getcwd(),
        help="working directory for iris (needs scsi1.raw etc.)",
    )
    parser.add_argument(
        "--config",
        default=os.path.join(os.path.dirname(__file__), "..", "iris-rig.toml"),
        help="iris rig configuration",
    )
    parser.add_argument("--bank0", default="32")
    parser.add_argument("--bank1", default="0")
    parser.add_argument("--monitor-port", type=int, default=9998)
    parser.add_argument("--telnet-port", type=int, default=2323)
    parser.add_argument(
        "--serial-log",
        default=None,
        help="serial log path (default: <work-dir>/serial.log)",
    )
    parser.add_argument(
        "--work-dir",
        default=None,
        help="per-run directory (default: a fresh temporary directory)",
    )
    parser.add_argument(
        "--keep-artifacts",
        action="store_true",
        help="keep the temporary work directory after the run",
    )
    parser.add_argument(
        "--commands",
        default="status,net,mem,time,tlb,gfx",
        help="comma-separated telnet commands",
    )
    args = parser.parse_args()

    created_workdir = args.work_dir is None
    if created_workdir:
        workdir = tempfile.mkdtemp(prefix="irix7-smoke-")
    else:
        workdir = os.path.abspath(args.work_dir)
        os.makedirs(workdir, exist_ok=True)
    log = os.path.abspath(args.serial_log) if args.serial_log else os.path.join(workdir, "serial.log")
    try:
        os.unlink(log)
    except FileNotFoundError:
        pass

    cmd = [
        args.iris,
        "--ci",
        "--config",
        os.path.abspath(args.config),
        "--prom",
        os.path.abspath(args.prom),
        "--bank0",
        args.bank0,
        "--bank1",
        args.bank1,
        "--noaudio",
        "--monitor-port",
        str(args.monitor_port),
        "--serial-log",
        log,
    ]
    print("+ " + " ".join(cmd))
    print(f"serial log: {log}")
    try:
        proc = subprocess.Popen(cmd, cwd=args.cwd)
    except OSError as error:
        print(f"FAIL: could not start iris: {error}", file=sys.stderr)
        if created_workdir and not args.keep_artifacts:
            shutil.rmtree(workdir, ignore_errors=True)
        return 1
    try:
        wait_monitor(args.monitor_port, "machine-start", proc=proc)
        # iris currently has no acknowledgement for machine-start; retry if
        # the ROM banner has not started after one short window.
        if wait_for(log, "ROM:", 8) is None:
            wait_monitor(args.monitor_port, "machine-start", proc=proc)
            if wait_for(log, "ROM:", 8) is None:
                raise RuntimeError("guest did not start after machine-start")
        wait_ready(log, 90)
        commands = [c.strip() for c in args.commands.split(",") if c.strip()]
        transcript = telnet(args.telnet_port, commands)
        print(transcript)
        return 0
    except (RuntimeError, OSError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        try:
            print(open(log, errors="replace").read()[-2000:], file=sys.stderr)
        except FileNotFoundError:
            pass
        return 1
    finally:
        stop_iris(proc)
        if created_workdir and not args.keep_artifacts:
            shutil.rmtree(workdir, ignore_errors=True)
        elif created_workdir:
            print(f"artifacts: {workdir}")


if __name__ == "__main__":
    raise SystemExit(main())
