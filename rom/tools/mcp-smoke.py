#!/usr/bin/env python3
"""Smoke-test the guest MCP control plane.

Starts iris (unless --no-launch), waits for the MCP endpoint, and checks the
JSON-RPC surface: initialize, tools/list, status, shell.exec, serial.read and
echo.  With --reset it also proves the autoload loop: system.reset, wait for
the reboot, then initialize and status again.

The guest must either autoload the module from NVRAM (mcp.url/mcp.sha256), or
be given --module-url/--module-sha256 so the script can load it through the
telnet console.  The token must match the guest's mcp.token.

Usage:
    rom/tools/mcp-smoke.py --iris /path/to/iris --prom /tmp/rom.bin \
        --token secret-token

No Python packages are required.
"""

import argparse
import json
import os
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request


def wait_for(path, needle, timeout):
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


def rpc(url, token, method, params=None, timeout=10.0):
    body = json.dumps(
        {"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}
    ).encode()
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(url, data=body, headers=headers)
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.load(response)


def wait_mcp(url, token, timeout=120.0):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            result = rpc(
                url,
                token,
                "initialize",
                {"protocolVersion": "2026-07-28"},
                timeout=3.0,
            )
            if "result" in result:
                return result
        except (urllib.error.URLError, OSError, ValueError):
            pass
        time.sleep(0.5)
    return None


def call_text(url, token, name, arguments):
    result = rpc(url, token, "tools/call", {"name": name, "arguments": arguments})
    return result["result"]["content"][0]["text"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--iris", help="iris binary (required unless --no-launch)")
    parser.add_argument("--prom", help="raw PROM image (required unless --no-launch)")
    parser.add_argument("--cwd", default=os.getcwd(), help="iris working directory")
    parser.add_argument(
        "--config",
        default=os.path.join(os.path.dirname(__file__), "..", "iris-rig.toml"),
        help="iris rig configuration",
    )
    parser.add_argument("--bank0", default="32")
    parser.add_argument("--bank1", default="0")
    parser.add_argument("--monitor-port", type=int, default=9998)
    parser.add_argument("--telnet-port", type=int, default=2323)
    parser.add_argument("--host-port", type=int, default=8088)
    parser.add_argument("--serial-log", default="/tmp/opencode/mcp-smoke.log")
    parser.add_argument("--token", default=os.environ.get("IRIX7_MCP_TOKEN", ""))
    parser.add_argument("--module-url", help="load this module through telnet")
    parser.add_argument("--module-sha256", help="digest for --module-url")
    parser.add_argument(
        "--reset", action="store_true", help="call system.reset and check recovery"
    )
    parser.add_argument(
        "--no-launch", action="store_true", help="test an already-running guest"
    )
    args = parser.parse_args()

    if not args.no_launch and (not args.iris or not args.prom):
        parser.error("--iris and --prom are required unless --no-launch")

    url = f"http://127.0.0.1:{args.host_port}/mcp"
    proc = None
    try:
        if not args.no_launch:
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
            time.sleep(3)
            monitor(args.monitor_port, "machine-start")
            if wait_for(log, "network: dns", 60) is None:
                print("FAIL: kernel did not reach DHCP", file=sys.stderr)
                return 1

        if args.module_url:
            if not args.module_sha256:
                parser.error("--module-url needs --module-sha256")
            transcript = telnet(
                args.telnet_port,
                [f"module {args.module_url} {args.module_sha256}"],
                settle=1.0,
            )
            if "module: returned" not in transcript:
                print("FAIL: module did not load", file=sys.stderr)
                print(transcript, file=sys.stderr)
                return 1

        initialized = wait_mcp(url, args.token)
        if initialized is None:
            print(
                "FAIL: MCP endpoint did not answer (token? autoload? module?)",
                file=sys.stderr,
            )
            return 1
        server = initialized["result"]["serverInfo"]["name"]
        if server != "irix7-mcp":
            print(f"FAIL: unexpected server {server!r}", file=sys.stderr)
            return 1

        tools = rpc(url, args.token, "tools/list")["result"]["tools"]
        names = {tool["name"] for tool in tools}
        for required in ("status", "shell.exec", "serial.read", "system.reset"):
            if required not in names:
                print(f"FAIL: tool {required!r} missing", file=sys.stderr)
                return 1

        status = call_text(url, args.token, "status", {})
        if "stage: kernel" not in status or "ip:" not in status:
            print(f"FAIL: bad status:\n{status}", file=sys.stderr)
            return 1
        uname = call_text(url, args.token, "shell.exec", {"command": "uname"})
        if "IRIX 7" not in uname:
            print(f"FAIL: bad uname: {uname!r}", file=sys.stderr)
            return 1
        serial = call_text(url, args.token, "serial.read", {"length": 256})
        if not serial:
            print("FAIL: serial ring empty", file=sys.stderr)
            return 1
        echo = call_text(url, args.token, "echo", {"text": "mcp-smoke"})
        if echo != "mcp-smoke":
            print(f"FAIL: bad echo: {echo!r}", file=sys.stderr)
            return 1

        print("PASS: initialize, tools/list, status, uname, serial.read, echo")
        print(status.strip())

        if args.reset:
            try:
                rpc(url, args.token, "tools/call", {"name": "system.reset"})
            except (urllib.error.URLError, OSError, ValueError):
                pass
            # Let the old server go away before polling for the new boot.
            time.sleep(3)
            if wait_mcp(url, args.token, timeout=120.0) is None:
                print("FAIL: MCP did not return after reset", file=sys.stderr)
                return 1
            status = ""
            for _ in range(20):
                try:
                    status = call_text(url, args.token, "status", {})
                    if "stage: kernel" in status:
                        break
                except (urllib.error.URLError, OSError, ValueError):
                    pass
                time.sleep(1)
            if "stage: kernel" not in status:
                print(f"FAIL: bad post-reset status:\n{status}", file=sys.stderr)
                return 1
            print("PASS: system.reset -> autoload -> MCP")
        return 0
    finally:
        if proc is not None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()


if __name__ == "__main__":
    raise SystemExit(main())
