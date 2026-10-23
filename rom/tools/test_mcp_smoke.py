#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Tests for mcp-smoke.py against a fake MCP endpoint.

The real conformance run needs iris, a booted guest and the SCSI images.  This
suite covers the script's own logic -- request/response handling, the module
tool checks, JSON-RPC error reporting and the launch argv -- with the Python
standard library only:

    python3 -m unittest discover -s rom/tools -p 'test_*.py'
"""

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "mcp-smoke.py")

STATUS = "stage: kernel\nip: 10.1.1.2\nram: 32 MiB\n"
MOTD = "Welcome to IRIX 7\n"
LOWER = {"/etc/motd": MOTD, "/hello.txt": "hello, erofs\n"}
ROOT = ["etc", "hello.txt", "big.bin", "empty", "dir", "link"]
TOOLS = [
    "status",
    "echo",
    "console",
    "serial.read",
    "serial.write",
    "system.reset",
    "system.poweroff",
    "shell.exec",
    "block.read",
    "fs.read",
    "fs.list",
    "fs.write",
    "fs.remove",
    "screen.capture",
]


def load_script():
    spec = importlib.util.spec_from_file_location("mcp_smoke", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class FakeMcpHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def _send(self, status, body, content_type="application/json", extra=None):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        for key, value in (extra or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body)

    def _reply(self, request_id, result=None, error=None):
        reply = {"jsonrpc": "2.0", "id": request_id}
        if error is not None:
            reply["error"] = {"code": error[0], "message": error[1]}
        else:
            reply["result"] = result
        self._send(200, json.dumps(reply).encode())

    def _content(self, text):
        return {"content": [{"type": "text", "text": text}]}

    def _tool(self, name, arguments):
        """Return (text, error message) for one tools/call."""
        if name == "status":
            return STATUS, None
        if name == "echo":
            return arguments.get("text", ""), None
        if name == "serial.read":
            return "serial ring", None
        if name == "shell.exec":
            if arguments.get("command") == "uname":
                return "IRIX 7 7.0.0", None
            return "ok", None
        if name == "block.read":
            if arguments.get("target") == 1 and arguments.get("lba") == 0:
                return "4d436f6d" + "00" * 508, None
            return None, "block read failed"
        if name == "fs.read":
            path = arguments.get("path", "/etc/motd")
            if path in self.server.overlay:
                return self.server.overlay[path], None
            if path in LOWER and path not in self.server.removed:
                return LOWER[path], None
            return None, "fs read failed"
        if name == "fs.list":
            path = arguments.get("path", "/")
            if path != "/":
                return None, "fs list failed"
            names = [name for name in ROOT if "/" + name not in self.server.removed]
            return "".join(name + "\n" for name in names), None
        if name == "fs.write":
            path = arguments["path"]
            self.server.overlay[path] = arguments.get("data", "")
            self.server.removed.discard(path)
            return "ok", None
        if name == "fs.remove":
            path = arguments["path"]
            if path in self.server.overlay:
                del self.server.overlay[path]
            elif path in LOWER:
                self.server.removed.add(path)
            else:
                return None, "fs remove failed"
            return "ok", None
        if name == "system.reset":
            # A reboot starts with an empty RAM overlay and a short outage
            # while the old module goes away.
            self.server.overlay.clear()
            self.server.removed.clear()
            self.server.down_until = time.time() + 1.0
            return "resetting", None
        return None, "unknown tool"

    def do_POST(self):
        if self.path != "/mcp":
            self._send(404, b"", "text/plain")
            return
        if getattr(self.server, "down_until", 0.0) > time.time():
            self._send(503, b"rebooting", "text/plain")
            return
        token = self.server.token
        if token and self.headers.get("Authorization") != f"Bearer {token}":
            self._send(
                401,
                b"",
                "text/plain",
                extra={"WWW-Authenticate": "Bearer"},
            )
            return
        length = int(self.headers.get("Content-Length") or 0)
        payload = self.rfile.read(length)
        try:
            request = json.loads(payload)
        except ValueError:
            self._reply(None, error=(-32700, "parse error"))
            return

        request_id = request.get("id", 1)
        method = request.get("method")
        if method == "initialize":
            version = (request.get("params") or {}).get(
                "protocolVersion", "2026-07-28"
            )
            self._reply(
                request_id,
                {
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "irix7-mcp", "version": "0.1.0"},
                },
            )
        elif method == "tools/list":
            self._reply(
                request_id,
                {
                    "tools": [
                        {"name": name, "description": "", "inputSchema": {}}
                        for name in TOOLS
                    ]
                },
            )
        elif method == "tools/call":
            params = request.get("params") or {}
            name = params.get("name")
            if self.server.fail_tool == name:
                self._reply(
                    request_id,
                    error=(-32603, f"{name} failed in the fake"),
                )
                return
            text, failure = self._tool(name, params.get("arguments") or {})
            if failure is not None:
                self._reply(request_id, error=(-32603, failure))
            else:
                self._reply(request_id, self._content(text))
        else:
            self._reply(request_id, error=(-32601, "method not found"))


class FakeMcp:
    """A fake MCP server on an ephemeral loopback port."""

    def __init__(self, token="", fail_tool=None):
        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), FakeMcpHandler)
        self.httpd.token = token
        self.httpd.fail_tool = fail_tool
        self.httpd.overlay = {}
        self.httpd.removed = set()
        self.httpd.down_until = 0.0
        self.thread = threading.Thread(
            target=self.httpd.serve_forever, daemon=True
        )
        self.thread.start()

    @property
    def port(self):
        return self.httpd.server_address[1]

    def close(self):
        self.httpd.shutdown()
        self.httpd.server_close()
        self.thread.join(timeout=5)


class SmokeScriptTests(unittest.TestCase):
    token = "secret-token"

    def start_fake(self, **kwargs):
        fake = FakeMcp(token=self.token, **kwargs)
        self.addCleanup(fake.close)
        return fake

    def run_script(self, fake, *extra):
        command = [
            sys.executable,
            SCRIPT,
            "--no-launch",
            "--host-port",
            str(fake.port),
            "--token",
            self.token,
        ]
        command += list(extra)
        return subprocess.run(
            command, capture_output=True, text=True, timeout=30
        )

    def test_full_surface_passes(self):
        fake = self.start_fake()
        result = self.run_script(fake)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(
            "PASS: initialize, tools/list, status, uname, serial.read, echo",
            result.stdout,
        )
        self.assertIn(
            "PASS: block.read target 1 lba 0 starts 4d436f6d", result.stdout
        )
        self.assertIn("PASS: fs.read /etc/motd", result.stdout)
        self.assertIn("PASS: fs.list / includes etc and link", result.stdout)
        self.assertIn(
            "PASS: fs.write/fs.read/fs.remove /tmp/mcp-smoke", result.stdout
        )
        self.assertIn(
            "PASS: fs.read /etc/motd after the overlay change", result.stdout
        )
        self.assertEqual(result.stderr, "")

    def test_skip_fs(self):
        fake = self.start_fake()
        result = self.run_script(fake, "--skip-fs")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("SKIP: fs.* checks (--skip-fs)", result.stdout)
        self.assertNotIn("PASS: fs.read /etc/motd\n", result.stdout)

    def test_json_rpc_error_is_reported(self):
        fake = self.start_fake(fail_tool="fs.read")
        result = self.run_script(fake)
        self.assertEqual(result.returncode, 1)
        self.assertIn(
            "FAIL: tools/call fs.read: JSON-RPC error -32603: "
            "fs.read failed in the fake",
            result.stderr,
        )

    def test_bad_token_fails_fast(self):
        fake = FakeMcp(token="other-token")
        self.addCleanup(fake.close)
        result = subprocess.run(
            [
                sys.executable,
                SCRIPT,
                "--no-launch",
                "--host-port",
                str(fake.port),
                "--token",
                self.token,
            ],
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("HTTP 401", result.stderr)

    def test_reset_checks_autoload_and_module_tools(self):
        fake = self.start_fake()
        result = self.run_script(fake, "--reset")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(
            "PASS: system.reset -> autoload -> MCP and module tools",
            result.stdout,
        )

    def test_help_lists_fixture_options(self):
        result = subprocess.run(
            [sys.executable, SCRIPT, "--help"],
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--erofs-image", result.stdout)
        self.assertIn("--scsi2", result.stdout)

    def test_missing_image_is_rejected_before_launch(self):
        result = subprocess.run(
            [
                sys.executable,
                SCRIPT,
                "--iris",
                "/bin/true",
                "--prom",
                "rom.bin",
                "--erofs-image",
                "/nonexistent/root.erofs",
            ],
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("SCSI target 2 image not found", result.stderr)

    def test_launch_failure_is_reported(self):
        module = load_script()
        with tempfile.TemporaryDirectory() as tmp:
            result = subprocess.run(
                [
                    sys.executable,
                    SCRIPT,
                    "--iris",
                    "/bin/sh",
                    "--prom",
                    "rom.bin",
                    "--cwd",
                    tmp,
                    "--serial-log",
                    os.path.join(tmp, "serial.log"),
                    "--monitor-port",
                    "0",
                    "--erofs-image",
                    os.path.normpath(module.FIXTURE_IMAGE),
                ],
                capture_output=True,
                text=True,
                timeout=30,
            )
        self.assertEqual(result.returncode, 1)
        self.assertIn("iris exited with status 2", result.stderr)


class LaunchCommandTests(unittest.TestCase):
    def args(self):
        return SimpleNamespace(
            iris="/path/to/iris",
            config="rom/iris-rig.toml",
            prom="/tmp/rom.bin",
            bank0="32",
            bank1="0",
            monitor_port=9998,
            serial_log="/tmp/opencode/mcp-smoke.log",
        )

    def test_image_is_attached_at_scsi2(self):
        module = load_script()
        image = os.path.abspath("root.erofs")
        command, log = module.iris_command(self.args(), image)
        self.assertIn("--scsi2", command)
        self.assertEqual(command[command.index("--scsi2") + 1], image)
        self.assertEqual(log, os.path.abspath("/tmp/opencode/mcp-smoke.log"))

    def test_no_image_leaves_scsi2_alone(self):
        module = load_script()
        command, _ = module.iris_command(self.args(), None)
        self.assertNotIn("--scsi2", command)

    def test_fixture_default_exists_in_repo(self):
        module = load_script()
        fixture = os.path.normpath(module.FIXTURE_IMAGE)
        self.assertTrue(
            os.path.isfile(fixture),
            f"checked-in fixture missing: {fixture}",
        )


if __name__ == "__main__":
    unittest.main()
