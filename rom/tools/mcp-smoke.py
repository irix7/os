#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Smoke-test the guest MCP control plane.

Starts iris (unless --no-launch), waits for the MCP endpoint, and checks the
JSON-RPC surface: initialize, tools/list, status, shell.exec, serial.read,
echo, block.read and the EROFS fs.read, fs.list, fs.stat, fs.readlink, fs.write
and fs.remove tools.  With --reset it also proves the autoload loop: system.reset,
wait for the reboot, then initialize, status and the module tools again.

The guest needs SCSI target 1 (the IRIX disk, normally scsi1.raw from the rig
configuration); pass --skip-block for a diskless guest with no target 1.  The
fs.* checks need the EROFS fixture at SCSI target 2, LBA 0.  When this script
launches iris, pass --erofs-image PATH (or the raw --scsi2 PATH) so it attaches
the fixture; the checked-in image is crates/erofs/tests/fixtures/root.erofs.
With --no-launch the caller must already have attached target 2, or pass
--skip-fs.

The module must autoload from NVRAM (mcp.url/mcp.sha256), or be given
--module-url/--module-sha256 so the script can load it through the telnet
console.  The token must match the guest's mcp.token.

Usage:

    rom/tools/mcp-smoke.py --iris /mnt/europa/iris/target/release/iris \
        --prom /tmp/rom.bin --cwd /mnt/europa/iris \
        --erofs-image crates/erofs/tests/fixtures/root.erofs \
        --token secret-token

That attaches the EROFS fixture at SCSI ID 2 and runs the whole surface.
Against an already-running guest whose target 2 is attached:

    rom/tools/mcp-smoke.py --no-launch --token secret-token

Against a diskless guest with neither SCSI target, the module surface is still
checked:

    rom/tools/mcp-smoke.py --no-launch --skip-block --skip-fs \
        --token secret-token

--reset needs NVRAM autoload: a module loaded only through --module-url does not
come back after the reset.  Paths given to --erofs-image and --scsi2 are
resolved against the caller's working directory, not --cwd.  No Python packages
are required.
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

# The fixture every fs.* check below expects: /etc/motd, /hello.txt, /link and
# the rest of the tree described in crates/erofs/tests/fixtures/README.md.
FIXTURE_IMAGE = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..",
    "..",
    "crates",
    "erofs",
    "tests",
    "fixtures",
    "root.erofs",
)

MOTD = "Welcome to IRIX 7"
SECTOR_MAGIC = "4d436f6d"  # "MComprHD", the SGI volume header magic
SMOKE_PATH = "/tmp/mcp-smoke"
SMOKE_DATA = "mcp-smoke overlay"


class SmokeError(Exception):
    """A failed check, or the endpoint did not do what was expected."""


class RpcError(SmokeError):
    """A transport, HTTP or JSON-RPC error from the MCP endpoint."""

    def __init__(self, method, message, code=None, data=None, status=None):
        super().__init__(message)
        self.method = method
        self.message = message
        self.code = code
        self.data = data
        self.status = status

    def __str__(self):
        if self.status is not None:
            text = f"{self.method}: HTTP {self.status}"
            if self.message:
                text += f": {self.message}"
        elif self.code is not None:
            text = f"{self.method}: JSON-RPC error {self.code}: {self.message}"
        else:
            text = f"{self.method}: {self.message}"
        if self.data is not None:
            text += f" (data: {self.data!r})"
        return text


class ProtocolError(SmokeError):
    """The reply is well-formed HTTP but not the MCP shape we expect."""


class ToolError(SmokeError):
    """A tool reported isError instead of a result."""


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


def wait_monitor(port, command, timeout=30.0, proc=None):
    """Send one monitor command once iris is accepting connections."""
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        if proc is not None and proc.poll() is not None:
            raise SmokeError(f"iris exited with status {proc.returncode}")
        try:
            return monitor(port, command)
        except OSError as error:
            last = error
            time.sleep(0.5)
    raise SmokeError(f"iris monitor on port {port} did not answer: {last}")


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
    """POST one JSON-RPC request and return the reply object.

    Raises RpcError for HTTP and JSON-RPC errors (with the server's code and
    message) and ProtocolError for a reply that is not JSON-RPC at all.
    """
    body = json.dumps(
        {"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}
    ).encode()
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(url, data=body, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            payload = response.read()
    except urllib.error.HTTPError as error:
        detail = ""
        try:
            detail = error.read().decode("utf-8", "replace").strip()
        except OSError:
            pass
        if not detail:
            detail = str(error.reason)
        raise RpcError(method, detail, status=error.code) from None
    except urllib.error.URLError as error:
        raise RpcError(method, str(error.reason)) from None
    except OSError as error:
        raise RpcError(method, str(error)) from None

    try:
        reply = json.loads(payload)
    except ValueError:
        raise ProtocolError(
            f"{method}: response is not JSON: {payload[:200]!r}"
        ) from None
    if not isinstance(reply, dict):
        raise ProtocolError(f"{method}: response is not a JSON object")

    error = reply.get("error")
    if error is not None:
        if isinstance(error, dict):
            raise RpcError(
                method,
                str(error.get("message", "")),
                code=error.get("code"),
                data=error.get("data"),
            )
        raise RpcError(method, f"malformed error member: {error!r}")
    if "result" not in reply:
        raise ProtocolError(f"{method}: response has no result: {payload[:200]!r}")
    return reply


def wait_mcp(url, token, timeout=120.0):
    """Poll initialize until the server answers, or return None."""
    deadline = time.time() + timeout
    last = None
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
        except RpcError as error:
            # A 401 will not fix itself; report it instead of polling for two
            # minutes with the wrong token.
            if error.status == 401:
                raise
            last = error
        except ProtocolError as error:
            last = error
        time.sleep(0.5)
    if last is not None:
        print(f"note: last MCP probe failed: {last}", file=sys.stderr)
    return None


def wait_mcp_gone(url, token, timeout=30.0):
    """Wait until the endpoint stops answering, proving the guest rebooted."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            rpc(
                url,
                token,
                "initialize",
                {"protocolVersion": "2026-07-28"},
                timeout=2.0,
            )
        except (RpcError, ProtocolError):
            return True
        time.sleep(0.5)
    return False


def call_tool(url, token, name, arguments, timeout=10.0):
    try:
        reply = rpc(
            url,
            token,
            "tools/call",
            {"name": name, "arguments": arguments},
            timeout=timeout,
        )
    except RpcError as error:
        # Name the tool in the diagnostic: "tools/call fs.read: ..." is much
        # more useful than a bare "tools/call" when one check fails.
        raise RpcError(
            f"tools/call {name}",
            error.message,
            code=error.code,
            data=error.data,
            status=error.status,
        ) from None
    result = reply.get("result")
    if not isinstance(result, dict):
        raise ProtocolError(f"tools/call {name}: result is not an object")
    content = result.get("content")
    text = ""
    if isinstance(content, list) and content and isinstance(content[0], dict):
        text = content[0].get("text", "")
    if result.get("isError"):
        raise ToolError(f"tools/call {name}: tool error: {text or 'no text'}")
    if not isinstance(content, list) or not content:
        raise ProtocolError(f"tools/call {name}: result has no content")
    return text


def require(condition, message):
    if not condition:
        raise SmokeError(message)


def read_must_fail(url, token, path):
    """Check that fs.read fails for a path that is not in the overlay."""
    try:
        text = call_tool(url, token, "fs.read", {"path": path})
    except ToolError:
        return
    except RpcError as error:
        if error.code is None:
            # Transport or HTTP failure: the endpoint itself is broken, which is
            # not evidence that the file is gone.
            raise
        return
    raise SmokeError(f"fs.read {path} unexpectedly returned {text[:80]!r}")


def parse_stat(text, path):
    """Parse the JSON object the module returns in the fs.stat text field."""
    try:
        stat = json.loads(text)
    except ValueError:
        raise ProtocolError(
            f"fs.stat {path}: text is not JSON: {text[:120]!r}"
        ) from None
    if not isinstance(stat, dict):
        raise ProtocolError(f"fs.stat {path}: result is not a JSON object")
    return stat


def expect_stat(url, token, path, **expected):
    """Check a fs.stat result without depending on JSON member order."""
    stat = parse_stat(call_tool(url, token, "fs.stat", {"path": path}), path)
    for field, want in expected.items():
        got = stat.get(field)
        require(
            got == want,
            f"fs.stat {path}: {field} is {got!r}, expected {want!r} "
            f"(full result {stat!r})",
        )
    return stat


def readlink_must_fail(url, token, path, reason):
    """Check that fs.readlink returns a JSON-RPC error naming the reason."""
    try:
        text = call_tool(url, token, "fs.readlink", {"path": path})
    except ToolError as error:
        raise SmokeError(
            f"fs.readlink {path}: expected a JSON-RPC error, got a tool error: "
            f"{error}"
        ) from None
    except RpcError as error:
        if error.code is None:
            # A transport or HTTP failure is not evidence about the path.
            raise
        require(
            reason in error.message.lower(),
            f"fs.readlink {path}: JSON-RPC error {error.code} does not mention "
            f"{reason!r}: {error.message}",
        )
        return error.message
    raise SmokeError(f"fs.readlink {path} unexpectedly returned {text[:80]!r}")


def iris_command(args, scsi2):
    """Build the iris argv (and serial log path) for one smoke run."""
    log = os.path.abspath(args.serial_log)
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
    if scsi2:
        cmd += ["--scsi2", os.path.abspath(scsi2)]
    return cmd, log


def stop_iris(proc):
    """Terminate iris, and make sure it is really gone before returning."""
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


def run(args, scsi2, fs_checks):
    url = f"http://127.0.0.1:{args.host_port}/mcp"
    proc = None
    try:
        if not args.no_launch:
            try:
                os.unlink(os.path.abspath(args.serial_log))
            except FileNotFoundError:
                pass
            cmd, log = iris_command(args, scsi2)
            print("+ " + " ".join(cmd))
            try:
                proc = subprocess.Popen(
                    cmd, cwd=args.cwd, stdin=subprocess.DEVNULL
                )
            except OSError as error:
                raise SmokeError(f"could not start iris: {error}") from None
            wait_monitor(args.monitor_port, "machine-start", proc=proc)
            if wait_for(log, "network: dns", 60) is None:
                raise SmokeError(
                    f"kernel did not reach DHCP; see {log} "
                    "(check --cwd contains scsi1.raw)"
                )

        if args.module_url:
            if not args.module_sha256:
                raise SmokeError("--module-url needs --module-sha256")
            try:
                transcript = telnet(
                    args.telnet_port,
                    [f"module {args.module_url} {args.module_sha256}"],
                    settle=1.0,
                )
            except OSError as error:
                raise SmokeError(
                    f"telnet console on port {args.telnet_port} did not answer: {error}"
                ) from None
            if "module: returned" not in transcript:
                raise SmokeError(
                    "module did not load through the telnet console:\n" + transcript
                )

        initialized = wait_mcp(url, args.token)
        if initialized is None:
            raise SmokeError(
                "MCP endpoint did not answer (check the token, NVRAM autoload, "
                "and whether the module was loaded)"
            )
        result = initialized.get("result") or {}
        server = (result.get("serverInfo") or {}).get("name")
        require(server == "irix7-mcp", f"unexpected server {server!r}")

        reply = rpc(url, args.token, "tools/list")
        tools = (reply.get("result") or {}).get("tools")
        require(isinstance(tools, list), "tools/list returned no tools array")
        names = {
            tool.get("name") for tool in tools if isinstance(tool, dict)
        }
        required = (
            "status",
            "shell.exec",
            "serial.read",
            "system.reset",
            "block.read",
            "fs.read",
            "fs.list",
            "fs.stat",
            "fs.readlink",
            "fs.write",
            "fs.remove",
        )
        missing = [name for name in required if name not in names]
        require(not missing, f"tools/list is missing {', '.join(missing)}")

        status = call_tool(url, args.token, "status", {})
        require(
            "stage: kernel" in status and "ip:" in status,
            f"bad status:\n{status}",
        )
        uname = call_tool(url, args.token, "shell.exec", {"command": "uname"})
        require("IRIX 7" in uname, f"bad uname: {uname!r}")
        serial = call_tool(url, args.token, "serial.read", {"length": 256})
        require(bool(serial), "serial ring empty")
        echo = call_tool(url, args.token, "echo", {"text": "mcp-smoke"})
        require(echo == "mcp-smoke", f"bad echo: {echo!r}")

        print("PASS: initialize, tools/list, status, uname, serial.read, echo")
        print(status.strip())

        if not args.skip_block:
            sector = call_tool(
                url, args.token, "block.read", {"target": 1, "lba": 0}
            )
            require(
                sector.startswith(SECTOR_MAGIC),
                f"block.read target 1 lba 0 returned {sector[:16]!r}, "
                f"expected MComprHD ({SECTOR_MAGIC})",
            )
            require(
                len(sector) == 1024
                and all(ch in "0123456789abcdef" for ch in sector),
                f"block.read returned {len(sector)} characters, expected 1024 hex",
            )
            print(
                f"PASS: block.read target 1 lba 0 starts {SECTOR_MAGIC} (MComprHD)"
            )
        else:
            print("SKIP: block.read checks (--skip-block)")

        if fs_checks:
            motd = call_tool(url, args.token, "fs.read", {"path": "/etc/motd"})
            require(MOTD in motd, f"fs.read /etc/motd returned {motd!r}")
            print("PASS: fs.read /etc/motd")

            expect_stat(url, args.token, "/", type="dir", origin="erofs")
            print("PASS: fs.stat / is a dir from erofs")

            expect_stat(
                url,
                args.token,
                "/link",
                type="symlink",
                target="hello.txt",
                origin="erofs",
            )
            print("PASS: fs.stat /link is a symlink to hello.txt")

            expect_stat(
                url,
                args.token,
                "/etc/motd",
                type="file",
                size=18,
                mode="100644",
                origin="erofs",
            )
            print("PASS: fs.stat /etc/motd is an 18-byte file with mode 100644")

            target = call_tool(url, args.token, "fs.readlink", {"path": "/link"})
            require(
                target == "hello.txt", f"fs.readlink /link returned {target!r}"
            )
            print("PASS: fs.readlink /link -> hello.txt")

            message = readlink_must_fail(
                url, args.token, "/etc/motd", "symlink"
            )
            print(f"PASS: fs.readlink /etc/motd rejected: {message}")

            listing = call_tool(url, args.token, "fs.list", {"path": "/"})
            entries = {
                line.strip() for line in listing.splitlines() if line.strip()
            }
            missing = {"etc", "link"} - entries
            require(
                not missing,
                f"fs.list / is missing {', '.join(sorted(missing))}: {sorted(entries)}",
            )
            print("PASS: fs.list / includes etc and link")

            written = call_tool(
                url,
                args.token,
                "fs.write",
                {"path": SMOKE_PATH, "data": SMOKE_DATA},
            )
            require(
                written.strip() == "ok",
                f"fs.write {SMOKE_PATH} returned {written!r}, expected 'ok'",
            )
            expect_stat(
                url,
                args.token,
                SMOKE_PATH,
                type="file",
                size=len(SMOKE_DATA),
                mode="100644",
                origin="overlay",
            )
            print(f"PASS: fs.stat {SMOKE_PATH} is from overlay")
            content = call_tool(url, args.token, "fs.read", {"path": SMOKE_PATH})
            require(
                content == SMOKE_DATA,
                f"fs.read {SMOKE_PATH} returned {content!r}, "
                f"expected {SMOKE_DATA!r}",
            )
            removed = call_tool(url, args.token, "fs.remove", {"path": SMOKE_PATH})
            require(
                removed.strip() == "ok",
                f"fs.remove {SMOKE_PATH} returned {removed!r}, expected 'ok'",
            )
            read_must_fail(url, args.token, SMOKE_PATH)
            print(f"PASS: fs.write/fs.read/fs.remove {SMOKE_PATH}")

            motd = call_tool(url, args.token, "fs.read", {"path": "/etc/motd"})
            require(
                MOTD in motd,
                f"fs.read /etc/motd after removing {SMOKE_PATH} returned {motd!r}",
            )
            print("PASS: fs.read /etc/motd after the overlay change")
        elif args.skip_fs:
            print("SKIP: fs.* checks (--skip-fs)")
        else:
            print(
                "SKIP: fs.* checks (no SCSI target 2: pass --erofs-image PATH, "
                f"e.g. {os.path.normpath(FIXTURE_IMAGE)})"
            )

        if args.reset:
            try:
                rpc(url, args.token, "tools/call", {"name": "system.reset"})
            except RpcError as error:
                if error.code is not None:
                    raise SmokeError(f"system.reset failed: {error}") from None
                # A dropped connection is expected once the guest resets.
            except ProtocolError:
                pass
            # The old server must go away before the next one can be evidence
            # that NVRAM autoload brought the module back.
            if not wait_mcp_gone(url, args.token):
                raise SmokeError(
                    "MCP still answered after system.reset; the guest did not reboot"
                )
            if wait_mcp(url, args.token, timeout=120.0) is None:
                raise SmokeError(
                    "MCP did not return after reset (does NVRAM autoload the module?)"
                )
            status = ""
            for _ in range(20):
                try:
                    status = call_tool(url, args.token, "status", {})
                    if "stage: kernel" in status:
                        break
                except (RpcError, ProtocolError):
                    pass
                time.sleep(1)
            require("stage: kernel" in status, f"bad post-reset status:\n{status}")

            if not args.skip_block:
                sector = call_tool(
                    url, args.token, "block.read", {"target": 1, "lba": 0}
                )
                require(
                    sector.startswith(SECTOR_MAGIC),
                    "block.read failed after reset: the module did not autoload",
                )
            else:
                print("SKIP: block.read after reset (--skip-block)")
            if fs_checks:
                motd = call_tool(url, args.token, "fs.read", {"path": "/etc/motd"})
                require(
                    MOTD in motd,
                    f"fs.read /etc/motd failed after reset: {motd!r}",
                )
                expect_stat(url, args.token, "/", type="dir", origin="erofs")
                target = call_tool(
                    url, args.token, "fs.readlink", {"path": "/link"}
                )
                require(
                    target == "hello.txt",
                    f"fs.readlink /link failed after reset: {target!r}",
                )
                print("PASS: fs.stat/fs.readlink after reset")
            print("PASS: system.reset -> autoload -> MCP and module tools")
        return 0
    finally:
        stop_iris(proc)


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
    parser.add_argument(
        "--erofs-image",
        metavar="PATH",
        help=(
            "EROFS fixture to attach at SCSI ID 2 when launching iris "
            f"(for example {os.path.normpath(FIXTURE_IMAGE)})"
        ),
    )
    parser.add_argument(
        "--scsi2",
        metavar="PATH",
        help="raw image for SCSI ID 2 (instead of --erofs-image)",
    )
    parser.add_argument(
        "--skip-block",
        action="store_true",
        help="skip the block.read checks (guest has no SCSI disk at target 1)",
    )
    parser.add_argument(
        "--skip-fs",
        action="store_true",
        help="skip the fs.* checks (guest has no EROFS disk at target 2)",
    )
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
    if args.erofs_image and args.scsi2:
        parser.error("pass only one of --erofs-image and --scsi2")
    if args.module_url and not args.module_sha256:
        parser.error("--module-url needs --module-sha256")

    scsi2 = args.scsi2 or args.erofs_image
    fs_checks = not args.skip_fs
    if args.no_launch:
        if scsi2:
            print(
                "note: --erofs-image/--scsi2 only apply when launching iris",
                file=sys.stderr,
            )
        scsi2 = None
    elif scsi2 is not None:
        scsi2 = os.path.abspath(scsi2)
        if not os.path.isfile(scsi2):
            parser.error(f"SCSI target 2 image not found: {scsi2}")
    else:
        # Nothing to attach, so the guest cannot have the fixture.
        fs_checks = False

    try:
        return run(args, scsi2, fs_checks)
    except SmokeError as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
