# SPDX-License-Identifier: GPL-3.0-only
"""Tests for tools/std-abi-surface.py.

Run with:

    python3 -m unittest discover -s tools -p 'test_*.py'

The fixtures are a miniature Rust source tree, not the real `rust-src`.  The
audit is name-level, so the fixtures exercise cfg selection, module following,
`use libc::{...}` groups and the three report buckets rather than real ABI
layout.
"""

import contextlib
import importlib.util
import io
import json
import os
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parents[1]
FIXTURES = Path(__file__).resolve().parent / "fixtures"
RUST_SRC = FIXTURES / "mini-rust-src"
CLEAN = FIXTURES / "manifest-clean.json"
GAPS = FIXTURES / "manifest-gaps.json"


def load_tool():
    path = TOOLS / "std-abi-surface.py"
    spec = importlib.util.spec_from_file_location("std_abi_surface", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


TOOL = load_tool()


def run(*args):
    """Run the tool's main() and capture stdout."""
    output = io.StringIO()
    with contextlib.redirect_stdout(output), contextlib.redirect_stderr(io.StringIO()):
        code = TOOL.main(list(args))
    return code, output.getvalue()


def run_json(*args):
    code, output = run("--json", *args)
    return code, json.loads(output)


class CleanSurfaceTests(unittest.TestCase):
    def test_clean_manifest_reports_nothing_missing(self):
        code, output = run("--rust-src", str(RUST_SRC), "--manifest", str(CLEAN))
        self.assertEqual(code, 0)
        self.assertIn("MISSING (referenced by std, not promised for v1): 0", output)
        self.assertIn("UNCLASSIFIED (referenced by std, absent from the manifest): 0", output)

    def test_use_groups_and_types_are_collected(self):
        code, report = run_json("--rust-src", str(RUST_SRC), "--manifest", str(CLEAN))
        self.assertEqual(code, 0)
        # Only getentropy is promised and unused.  c_int, AT_FDCWD and openat
        # must have been found through `use libc::{...}`, and c_char through a
        # path in another module.
        self.assertEqual(report["unused"], ["getentropy"])

    def test_cfg_select_picks_the_native_arm(self):
        code, report = run_json("--rust-src", str(RUST_SRC), "--manifest", str(CLEAN))
        self.assertEqual(code, 0)
        names = {item["name"] for item in report["missing"]}
        self.assertNotIn("statx", names)
        self.assertNotIn("futimens", names)  # it is in the manifest, so not missing
        # The Linux-only item in lib.rs and the cfg_select arm must both be
        # excluded from every bucket.
        all_reported = {
            item["name"]
            for key in ("missing", "unclassified")
            for item in report[key]
        }
        self.assertNotIn("statx", all_reported)
        self.assertNotIn("linux_only", all_reported)
        self.assertNotIn("pthread_getname_np", all_reported)

    def test_json_is_deterministic(self):
        first = run("--json", "--rust-src", str(RUST_SRC), "--manifest", str(CLEAN))
        second = run("--json", "--rust-src", str(RUST_SRC), "--manifest", str(CLEAN))
        self.assertEqual(first, second)


class GapTests(unittest.TestCase):
    def test_gaps_are_reported_in_warning_mode(self):
        code, report = run_json("--rust-src", str(RUST_SRC), "--manifest", str(GAPS))
        self.assertEqual(code, 0)
        missing = {item["name"] for item in report["missing"]}
        self.assertEqual(missing, {"AT_FDCWD", "c_char", "futimens", "openat"})
        unclassified = {item["name"] for item in report["unclassified"]}
        self.assertEqual(unclassified, {"AT_FDCWD", "c_char", "futimens", "openat"})

    def test_gaps_fail_in_strict_mode(self):
        code, _ = run("--strict", "--rust-src", str(RUST_SRC), "--manifest", str(GAPS))
        self.assertEqual(code, 1)

    def test_clean_passes_strict_mode(self):
        code, _ = run("--strict", "--rust-src", str(RUST_SRC), "--manifest", str(CLEAN))
        self.assertEqual(code, 0)

    def test_missing_entry_with_v1_false_is_not_unclassified(self):
        manifest = {
            "schema": "irix7-native-libc-symbols",
            "version": "0.1",
            "entries": [
                {"name": name, "kind": "function", "class": "unsupported", "area": "files", "v1": False}
                for name in ("read", "c_int", "c_char", "openat", "AT_FDCWD", "futimens")
            ],
        }
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as handle:
            json.dump(manifest, handle)
            path = handle.name
        try:
            code, report = run_json("--rust-src", str(RUST_SRC), "--manifest", path)
        finally:
            os.unlink(path)
        self.assertEqual(code, 0)
        self.assertEqual(report["unclassified"], [])
        self.assertEqual(len(report["missing"]), 6)
        self.assertEqual(report["missing"][0]["entry_class"], "unsupported")

    def test_missing_manifest_is_a_usage_error(self):
        code, _ = run("--rust-src", str(RUST_SRC), "--manifest", str(FIXTURES / "nope.json"))
        self.assertEqual(code, 2)


class ManifestValidationTests(unittest.TestCase):
    def _load(self, data):
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as handle:
            json.dump(data, handle)
            path = handle.name
        try:
            return TOOL.load_manifest(path)
        finally:
            os.unlink(path)

    def test_duplicate_name_is_rejected(self):
        data = {
            "schema": "irix7-native-libc-symbols",
            "version": "0.1",
            "entries": [
                {"name": "open", "kind": "function", "class": "posix-2024", "area": "files", "v1": True},
                {"name": "open", "kind": "function", "class": "posix-2024", "area": "files", "v1": True},
            ],
        }
        with self.assertRaises(SystemExit):
            self._load(data)

    def test_bad_class_is_rejected(self):
        data = {
            "schema": "irix7-native-libc-symbols",
            "version": "0.1",
            "entries": [
                {"name": "open", "kind": "function", "class": "linux", "area": "files", "v1": True}
            ],
        }
        with self.assertRaises(SystemExit):
            self._load(data)

    def test_missing_v1_flag_is_rejected(self):
        data = {
            "schema": "irix7-native-libc-symbols",
            "version": "0.1",
            "entries": [
                {"name": "open", "kind": "function", "class": "posix-2024", "area": "files"}
            ],
        }
        with self.assertRaises(SystemExit):
            self._load(data)


if __name__ == "__main__":
    unittest.main()
