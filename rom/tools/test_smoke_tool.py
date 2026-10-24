#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Tests for the boot-readiness helper in rom/tools/smoke.py."""

import importlib.util
import os
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "smoke.py")


def load_smoke():
    spec = importlib.util.spec_from_file_location("rom_smoke", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ReadinessTests(unittest.TestCase):
    def _file(self, text):
        handle = tempfile.NamedTemporaryFile("w", delete=False)
        handle.write(text)
        handle.close()
        self.addCleanup(os.unlink, handle.name)
        return handle.name

    def test_ready_marker(self):
        module = load_smoke()
        path = self._file("kernel: up\nBOOT-READY v=1 boot=9 kernel=up network=up mcp=listening\n")
        self.assertIn("BOOT-READY", module.wait_ready(path, timeout=1))

    def test_not_ready_marker(self):
        module = load_smoke()
        path = self._file(
            "BOOT-NOT-READY v=1 boot=9 kernel=up network=down "
            "mcp=failed reason=network-down\n"
        )
        with self.assertRaises(RuntimeError) as context:
            module.wait_ready(path, timeout=1)
        self.assertIn("network-down", str(context.exception))

    def test_missing_marker_times_out(self):
        module = load_smoke()
        path = self._file("kernel: up\n")
        with self.assertRaises(RuntimeError):
            module.wait_ready(path, timeout=0.2)


if __name__ == "__main__":
    unittest.main()
