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
import socket
import subprocess
import sys
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
    parser.add_argument("--serial-log", default="/tmp/opencode/rom-smoke.log")
    parser.add_argument(
        "--commands",
        default="status,net,mem,time",
        help="comma-separated telnet commands",
    )
    args = parser.parse_args()

    log = os.path.abspath(args.serial_log)
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
    proc = subprocess.Popen(cmd, cwd=args.cwd)

    try:
        time.sleep(3)
        monitor(args.monitor_port, "machine-start")
        if wait_for(log, "network: dns", 30) is None:
            print("FAIL: kernel did not reach DHCP", file=sys.stderr)
            print(open(log, errors="replace").read()[-2000:], file=sys.stderr)
            return 1

        commands = [c.strip() for c in args.commands.split(",") if c.strip()]
        transcript = telnet(args.telnet_port, commands)
        print(transcript)
        return 0
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    raise SystemExit(main())
