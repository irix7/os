#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Audit the native IRIX 7 libc surface that Rust `std` references.

This is a **name-level** audit.  It walks the pinned Rust `std` sources, follows
the module tree that would be compiled for a native `target_os = "irix7"` target
(`target_family = "unix"`, n32, big-endian) and collects every `libc::`
reference it finds.  It then compares those names with the machine-readable
manifest in `docs/spec/native-libc-symbols.json`.

It is not an ABI checker.  It does not compile anything, does not expand macros
beyond `cfg_select!`, does not evaluate `cfg!()` in expressions, and does not
know whether a symbol has the right signature, layout or semantics.  A clean
run means only that every name `std` mentions has a written decision.

Standard library only, read-only, deterministic.  No network access.
"""

from __future__ import annotations

import argparse
import bisect
import json
import os
import re
import sys

SCHEMA = "irix7-native-libc-symbols"
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(SCRIPT_DIR)
DEFAULT_MANIFEST = os.path.join(REPO_ROOT, "docs", "spec", "native-libc-symbols.json")

# The cfg model of the native target, from the std research report section 3.2.
# These are the only values the audit claims to know.  Anything else is
# "unknown" and is treated as possibly active (conservative).
TARGET = {
    "target_os": "irix7",
    "target_family": "unix",
    "target_vendor": "sgi",
    "target_env": "",
    "target_abi": "n32",
    "target_arch": "mips64",
    "target_endian": "big",
    "target_pointer_width": "32",
    "target_has_atomic": {"64"},
    "target_thread_local": False,
    "target_has_threads": True,
}

# Bare cfg names with a known truth value.
BARE = {
    "unix": True,
    "windows": False,
    "target_thread_local": False,
    "target_has_threads": True,
}

# Keyword cfgs set by the build, not by the target.  The audit models a normal
# release build of the library (not `cfg(test)`), so the harness-only ones are
# false.  `debug_assertions` stays unknown because it does not change the libc
# surface in a way the audit should hide.
BUILD_CFG = {
    "test": False,
    "miri": False,
    "bootstrap": False,
    "proc_macro": False,
    "fuzzing": False,
    "rustfmt": False,
    "clippy": False,
    "coverage": False,
    "docsrs": False,
    "doc": False,
    "doctest": False,
    "sanitize": False,
    "no_global_oom_handling": False,
}

# `std` helper macros that expand to a `#[cfg]` gate the audit cannot see
# through.  Each entry is known to be inactive for the native target.  If the
# pinned `std` changes, the audit over-reports rather than under-reports.
INACTIVE_MACROS = ("cfg_has_statx",)


class CfgError(ValueError):
    pass


def _split_top(text: str) -> list[str]:
    """Split on commas at bracket depth zero."""
    parts: list[str] = []
    depth = 0
    current: list[str] = []
    for ch in text:
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        if ch == "," and depth == 0:
            parts.append("".join(current).strip())
            current = []
        else:
            current.append(ch)
    tail = "".join(current).strip()
    if tail:
        parts.append(tail)
    return parts


def _strip_outer(expr: str) -> str:
    expr = expr.strip()
    while expr.startswith("(") and expr.endswith(")"):
        depth = 0
        balanced = True
        for index, ch in enumerate(expr):
            if ch == "(":
                depth += 1
            elif ch == ")":
                depth -= 1
                if depth == 0 and index != len(expr) - 1:
                    balanced = False
                    break
        if not balanced:
            break
        expr = expr[1:-1].strip()
    return expr


def evaluate_cfg(expr: str):
    """Return True, False or None (unknown) for a cfg predicate."""
    expr = _strip_outer(expr)
    if not expr:
        return None
    if expr == "true":
        return True
    if expr == "false":
        return False

    match = re.match(r"^(all|any)\s*\((.*)\)$", expr, re.S)
    if match:
        values = [evaluate_cfg(part) for part in _split_top(match.group(2))]
        if match.group(1) == "all":
            if any(value is False for value in values):
                return False
            if all(value is True for value in values):
                return True
            return None
        if any(value is True for value in values):
            return True
        if all(value is False for value in values):
            return False
        return None

    match = re.match(r"^not\s*\((.*)\)$", expr, re.S)
    if match:
        value = _evaluate(match.group(1))
        return None if value is None else not value

    match = re.match(r'^([a-z_]+)\s*=\s*"([^"]*)"$', expr)
    if match:
        key, value = match.groups()
        if key in TARGET:
            expected = TARGET[key]
            if isinstance(expected, set):
                return value in expected
            return value == expected
        if key in ("target_feature", "feature", "panic"):
            return None
        if key == "target_has_atomic":
            return None
        return None

    if expr in BARE:
        return BARE[expr]
    if expr in BUILD_CFG:
        return BUILD_CFG[expr]
    # An unrecognised bare predicate is a build-script or compiler cfg.  Being
    # conservative keeps both arms visible, which is safer than hiding one.
    return None


def _evaluate(expr: str):
    """Conservative wrapper: a parse failure is an unknown cfg, not a crash."""
    try:
        return evaluate_cfg(expr)
    except (CfgError, RecursionError, ValueError):
        return None


def _mask(text: str) -> str:
    """Blank comments and string/char literal contents, preserving offsets.

    Newlines and delimiters are kept so offsets and line numbers stay valid.
    Everything the scanner treats as structure (branches, brackets, semicolons)
    survives; everything it treats as data does not.
    """
    out: list[str] = []
    index = 0
    length = len(text)
    block_depth = 0
    while index < length:
        ch = text[index]
        nxt = text[index + 1] if index + 1 < length else ""
        if block_depth:
            if ch == "/" and nxt == "*":
                block_depth += 1
                out.extend("  ")
                index += 2
                continue
            if ch == "*" and nxt == "/":
                block_depth -= 1
                out.extend("  ")
                index += 2
                continue
            out.append("\n" if ch == "\n" else " ")
            index += 1
            continue
        if ch == "/" and nxt == "/":
            while index < length and text[index] != "\n":
                out.append(" ")
                index += 1
            continue
        if ch == "/" and nxt == "*":
            block_depth = 1
            out.extend("  ")
            index += 2
            continue
        if ch == "r" and nxt in "#\"":
            # Raw string: r"..." or r#"..."#
            hashes = 0
            probe = index + 1
            while probe < length and text[probe] == "#":
                hashes += 1
                probe += 1
            if probe < length and text[probe] == '"':
                terminator = '"' + "#" * hashes
                out.append(ch)
                out.append("#" * hashes)
                out.append('"')
                index = probe + 1
                end = text.find(terminator, index)
                if end == -1:
                    end = length
                for raw in text[index:end]:
                    out.append("\n" if raw == "\n" else " ")
                index = min(end + len(terminator), length)
                continue
        if ch == '"':
            out.append('"')
            index += 1
            while index < length:
                inner = text[index]
                if inner == "\\" and index + 1 < length:
                    out.extend("  ")
                    index += 2
                    continue
                if inner == '"':
                    out.append('"')
                    index += 1
                    break
                out.append("\n" if inner == "\n" else " ")
                index += 1
            continue
        if ch == "'":
            # Char literal or lifetime.  A char literal has a closing quote
            # within a few characters and is not an identifier prefix.
            end = index + 1
            if end < length and text[end] == "\\":
                end += 2
            else:
                end += 1
            if end < length and text[end] == "'":
                out.append("'")
                for _ in text[index + 1 : end]:
                    out.append(" ")
                out.append("'")
                index = end + 1
                continue
        out.append(ch)
        index += 1
    return "".join(out)


class Source:
    """A source file with its masked text and line index."""

    def __init__(self, path: str, rel: str, text: str):
        self.path = path
        self.rel = rel
        self.text = text
        self.mask = _mask(text)
        self.line_starts = [0]
        for match in re.finditer("\n", text):
            self.line_starts.append(match.end())
        self.line_count = len(self.line_starts)
        self.active = [True] * (self.line_count + 1)

    def line_of(self, offset: int) -> int:
        return max(0, bisect.bisect_right(self.line_starts, offset) - 1)

    def offset_of(self, line: int) -> int:
        if line >= len(self.line_starts):
            return len(self.text)
        return self.line_starts[line]

    def line_text(self, line: int) -> str:
        start = self.line_starts[line]
        end = self.line_starts[line + 1] if line + 1 < len(self.line_starts) else len(self.text)
        return self.text[start:end]

    def line_mask(self, line: int) -> str:
        start = self.line_starts[line]
        end = self.line_starts[line + 1] if line + 1 < len(self.line_starts) else len(self.mask)
        return self.mask[start:end]

    def deactivate(self, lo: int, hi: int) -> None:
        lo = max(0, lo)
        hi = min(hi, self.line_count - 1)
        for line in range(lo, hi + 1):
            self.active[line] = False

    def active_text(self) -> str:
        """The masked text with inactive lines blanked, offsets preserved."""
        chars = list(self.mask)
        for line in range(self.line_count):
            if self.active[line]:
                continue
            start = self.offset_of(line)
            end = self.offset_of(line + 1)
            for index in range(start, min(end, len(chars))):
                if chars[index] != "\n":
                    chars[index] = " "
        return "".join(chars)


def _match_close(text: str, open_offset: int) -> int:
    """Given the offset of an opening bracket, return the matching close."""
    pairs = {"(": ")", "[": "]", "{": "}"}
    opener = text[open_offset]
    closer = pairs[opener]
    depth = 0
    for index in range(open_offset, len(text)):
        ch = text[index]
        if ch == opener:
            depth += 1
        elif ch == closer:
            depth -= 1
            if depth == 0:
                return index
    raise CfgError("unbalanced bracket at offset %d" % open_offset)


_ARM_RE = re.compile(r"=>")


def _parse_cfg_select(src: Source, offset: int):
    """Parse a `cfg_select!{ ... }` at `offset`.

    Returns (arms, block_end).  Each arm is (predicate, start_offset,
    end_offset).
    """
    match = re.search(r"cfg_select!\s*", src.mask[offset:])
    if not match:
        raise CfgError("not a cfg_select! invocation")
    body_open = offset + match.end()
    if body_open >= len(src.mask) or src.mask[body_open] not in "{(":
        raise CfgError("cfg_select! without a body")
    body_close = _match_close(src.mask, body_open)
    inner_start = body_open + 1
    arms = []
    pos = inner_start
    while pos < body_close:
        while pos < body_close and src.mask[pos].isspace():
            pos += 1
        if pos >= body_close:
            break
        arrow = _ARM_RE.search(src.mask, pos, body_close)
        if not arrow:
            break
        predicate = src.text[pos : arrow.start()].strip()
        body_start = arrow.end()
        while body_start < body_close and src.mask[body_start].isspace():
            body_start += 1
        if body_start < body_close and src.mask[body_start] in "{(":
            body_end = _match_close(src.mask, body_start)
        else:
            # A bare expression arm: read to the next comma at depth zero.
            depth = 0
            cursor = body_start
            while cursor < body_close:
                ch = src.mask[cursor]
                if ch in "([{":
                    depth += 1
                elif ch in ")]}":
                    depth -= 1
                elif ch == "," and depth == 0:
                    break
                cursor += 1
            body_end = cursor - 1
        arms.append((predicate, pos, body_end))
        pos = body_end + 1
        while pos < body_close and src.mask[pos] in " \t\r\n,":
            pos += 1
    return arms, body_close


def _find_item_end(src: Source, line: int) -> int:
    """Find the last line of the item starting at `line`."""
    depth = 0
    opened = False
    cursor = line
    while cursor < src.line_count:
        code = src.line_mask(cursor)
        for ch in code:
            if ch in "([{":
                depth += 1
                opened = True
            elif ch in ")]}":
                depth -= 1
        stripped = code.strip()
        if depth <= 0 and opened and stripped.endswith(("}", ";")):
            # `} else {` on the next line belongs to the same item.
            probe = cursor + 1
            while probe < src.line_count and not src.line_mask(probe).strip():
                probe += 1
            if stripped.endswith("}") and probe < src.line_count:
                if src.line_mask(probe).strip().startswith("else"):
                    cursor = probe
                    continue
            return cursor
        if depth <= 0 and not opened and stripped.endswith((";", ",")):
            return cursor
        if depth <= 0 and opened and stripped.endswith(","):
            return cursor
        cursor += 1
    return src.line_count - 1


_ATTR_RE = re.compile(r"^\s*#\[(cfg|cfg_attr)\s*\(")


def _mark_conditional(src: Source, lo: int, hi: int) -> None:
    """Deactivate cfg-selected code for the native target."""
    line = lo
    while line < hi:
        code = src.line_mask(line)
        stripped = code.lstrip()
        if not stripped:
            line += 1
            continue

        inner = re.match(r"^\s*#!\[cfg\((.*)\)\]\s*$", code)
        if inner:
            if _evaluate(inner.group(1)) is False:
                src.deactivate(0, src.line_count)
                return
            line += 1
            continue

        if _ATTR_RE.match(code):
            attr_start = line
            attr_end = line
            # Gather multi-line attributes.
            while attr_end + 1 < src.line_count:
                joined = src.mask[src.offset_of(attr_start) : src.offset_of(attr_end + 1)]
                if joined.count("[") <= joined.count("]"):
                    break
                attr_end += 1
            joined_raw = src.text[src.offset_of(attr_start) : src.offset_of(attr_end + 1)]
            attr_text = joined_raw.strip()
            match = re.match(r"^#\[cfg\((.*)\)\]\s*$", attr_text, re.S)
            if match:
                value = _evaluate(match.group(1))
                if value is False:
                    end = _find_item_end(src, attr_end + 1)
                    src.deactivate(attr_start, end)
                    line = end + 1
                    continue
            line = attr_end + 1
            continue

        for macro in INACTIVE_MACROS:
            macro_at = src.mask.find(macro + "!", src.offset_of(line), src.offset_of(line + 1))
            if macro_at == -1:
                continue
            brace = src.mask.find("{", macro_at)
            if brace == -1 or brace > src.offset_of(line + 1):
                continue
            close = _match_close(src.mask, brace)
            src.deactivate(line, src.line_of(close))
            line = src.line_of(close) + 1
            break
        else:
            macro_at = -1
        if macro_at != -1:
            continue

        select_at = src.mask.find("cfg_select!", src.offset_of(line), src.offset_of(line + 1))
        if select_at != -1:
            try:
                arms, block_close = _parse_cfg_select(src, select_at)
            except CfgError:
                line += 1
                continue
            chosen = None
            for predicate, start, end in arms:
                if predicate == "_":
                    if chosen is None:
                        chosen = (predicate, start, end)
                    break
                value = _evaluate(predicate)
                if value is True or value is None:
                    chosen = (predicate, start, end)
                    break
            if chosen is not None:
                for predicate, start, end in arms:
                    if (start, end) != (chosen[1], chosen[2]):
                        src.deactivate(src.line_of(start), src.line_of(end))
                _mark_conditional(src, src.line_of(chosen[1]), src.line_of(chosen[2]) + 1)
            block_end_line = src.line_of(block_close)
            line = max(line + 1, block_end_line + 1)
            continue

        line += 1


_USE_GROUP = re.compile(r"\buse\s+libc::\{([^}]*)\}", re.S)
_USE_SINGLE = re.compile(r"\buse\s+libc::([A-Za-z_][A-Za-z0-9_]*)\s*(?:as\s+[A-Za-z_][A-Za-z0-9_]*)?\s*;")
_MOD_DECL = re.compile(
    r"^\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:unsafe\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;"
)
_PATH_ATTR = re.compile(r'^\s*#\[path\s*=\s*"([^"]+)"\]\s*$')


def _collect_names(src: Source, aliases: tuple[str, ...]) -> dict[str, list[int]]:
    text = src.active_text()
    found: dict[str, list[int]] = {}
    prefixes = "|".join(["libc"] + [re.escape(alias) for alias in aliases])
    path_re = re.compile(r"\b(?:%s)::([A-Za-z_][A-Za-z0-9_]*)" % prefixes)

    def add(name: str, offset: int) -> None:
        found.setdefault(name, []).append(src.line_of(offset))

    for match in path_re.finditer(text):
        add(match.group(1), match.start())
    for match in _USE_GROUP.finditer(text):
        for part in _split_top(match.group(1)):
            part = part.strip()
            if not part or part in ("self", "*"):
                continue
            part = part.split(" as ")[0].strip()
            if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", part):
                add(part, match.start())
    for match in _USE_SINGLE.finditer(text):
        add(match.group(1), match.start())
    return found


class Auditor:
    def __init__(self, rust_src: str):
        self.rust_src = os.path.abspath(rust_src)
        self.std_src = self._find_std_src()
        self.files: dict[str, Source] = {}
        self.names: dict[str, list[tuple[str, int]]] = {}
        self.external_modules: list[str] = []

    def _find_std_src(self) -> str:
        candidates = [
            self.rust_src,
            os.path.join(self.rust_src, "library", "std", "src"),
            os.path.join(self.rust_src, "std", "src"),
        ]
        for candidate in candidates:
            if os.path.isfile(os.path.join(candidate, "lib.rs")):
                return candidate
        raise SystemExit(
            "error: %s does not look like a Rust source tree "
            "(expected lib.rs, library/std/src/lib.rs or std/src/lib.rs)" % self.rust_src
        )

    def _load(self, path: str) -> Source | None:
        rel = os.path.relpath(path, self.std_src)
        if rel in self.files:
            return self.files[rel]
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as handle:
                text = handle.read()
        except OSError as error:
            print("warning: cannot read %s: %s" % (path, error), file=sys.stderr)
            return None
        src = Source(path, rel, text)
        self.files[rel] = src
        _mark_conditional(src, 0, src.line_count)
        return src

    def _module_paths(self, src: Source) -> list[str]:
        paths: list[str] = []
        if os.path.basename(src.path) in ("mod.rs", "lib.rs"):
            base_root = os.path.dirname(src.path)
        else:
            base_root = os.path.join(
                os.path.dirname(src.path), os.path.splitext(os.path.basename(src.path))[0]
            )
        pending_path = None
        stack: list[tuple[int, list[str]]] = []
        depth = 0
        for line in range(src.line_count):
            code = src.line_mask(line)
            stripped = code.strip()
            if src.active[line]:
                match = _PATH_ATTR.match(src.line_text(line))
                if match:
                    pending_path = match.group(1)
                else:
                    inline = re.match(
                        r"^\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:unsafe\s+)?mod\s+"
                        r"([A-Za-z_][A-Za-z0-9_]*)\s*\{",
                        code,
                    )
                    if inline:
                        prefix = (stack[-1][1] if stack else []) + [inline.group(1)]
                        stack.append((depth + 1, prefix))
                        pending_path = None
                    else:
                        match = _MOD_DECL.match(code)
                        if match:
                            name = match.group(1)
                            prefix = stack[-1][1] if stack else []
                            base = os.path.join(base_root, *prefix)
                            if pending_path:
                                candidate = os.path.normpath(
                                    os.path.join(base, pending_path)
                                )
                            else:
                                candidate = os.path.join(base, name + ".rs")
                                if not os.path.isfile(candidate):
                                    candidate = os.path.join(base, name, "mod.rs")
                            candidate = os.path.normpath(candidate)
                            if os.path.isfile(candidate):
                                if candidate.startswith(self.std_src + os.sep):
                                    paths.append(candidate)
                                else:
                                    self.external_modules.append(
                                        os.path.relpath(candidate, self.std_src)
                                    )
                            pending_path = None
                        elif stripped and not stripped.startswith("#"):
                            pending_path = None
            for ch in code:
                if ch in "([{":
                    depth += 1
                elif ch in ")]}":
                    depth -= 1
            while stack and depth < stack[-1][0]:
                stack.pop()
        return paths

    def _aliases(self) -> tuple[str, ...]:
        aliases: set[str] = set()
        for src in self.files.values():
            text = src.active_text()
            for match in re.finditer(r"\buse\s+libc\s+as\s+([A-Za-z_][A-Za-z0-9_]*)", text):
                aliases.add(match.group(1))
        changed = True
        while changed:
            changed = False
            for src in self.files.values():
                text = src.active_text()
                for match in re.finditer(
                    r"\buse\s+([A-Za-z_][A-Za-z0-9_]*)\s+as\s+([A-Za-z_][A-Za-z0-9_]*)",
                    text,
                ):
                    if match.group(1) in aliases and match.group(2) not in aliases:
                        aliases.add(match.group(2))
                        changed = True
        return tuple(sorted(aliases))

    def run(self) -> None:
        root = os.path.join(self.std_src, "lib.rs")
        queue = [root]
        seen: set[str] = set()
        while queue:
            path = queue.pop()
            if path in seen:
                continue
            seen.add(path)
            src = self._load(path)
            if src is None:
                continue
            queue.extend(reversed(self._module_paths(src)))
        aliases = self._aliases()
        for rel, src in self.files.items():
            for name, lines in _collect_names(src, aliases).items():
                for line in lines:
                    self.names.setdefault(name, []).append((rel, line + 1))


def load_manifest(path: str) -> dict:
    with open(path, "r", encoding="utf-8") as handle:
        data = json.load(handle)
    if data.get("schema") != SCHEMA:
        raise SystemExit("error: %s: schema must be %r" % (path, SCHEMA))
    entries = data.get("entries")
    if not isinstance(entries, list):
        raise SystemExit("error: %s: missing entries array" % path)
    seen: dict[str, dict] = {}
    for entry in entries:
        name = entry.get("name")
        if not isinstance(name, str) or not name:
            raise SystemExit("error: %s: entry without a name" % path)
        if name in seen:
            raise SystemExit("error: %s: duplicate entry %s" % (path, name))
        for field in ("kind", "class", "area"):
            if not isinstance(entry.get(field), str) or not entry[field]:
                raise SystemExit("error: %s: %s is missing %s" % (path, name, field))
        if entry["class"] not in ("posix-2024", "xsi", "extension", "compat-only", "unsupported"):
            raise SystemExit("error: %s: %s has invalid class %r" % (path, name, entry["class"]))
        if not isinstance(entry.get("v1"), bool):
            raise SystemExit("error: %s: %s must have a boolean v1" % (path, name))
        seen[name] = entry
    data["_by_name"] = seen
    return data


def build_report(auditor: Auditor, manifest: dict) -> dict:
    entries = manifest["_by_name"]
    referenced = auditor.names
    missing = []
    unclassified = []
    for name in sorted(referenced):
        entry = entries.get(name)
        occurrences = sorted(set(referenced[name]))
        record = {
            "name": name,
            "occurrences": [{"file": rel, "line": line} for rel, line in occurrences[:5]],
            "occurrence_count": len(occurrences),
        }
        if entry is None:
            unclassified.append(record)
            missing.append(dict(record, reason="not in manifest"))
        elif not entry["v1"]:
            missing.append(
                dict(
                    record,
                    reason="manifest marks it %s, not part of v1" % entry["class"],
                    entry_class=entry["class"],
                )
            )
    unused = sorted(
        name for name, entry in entries.items() if entry["v1"] and name not in referenced
    )
    return {
        "manifest": {
            "version": manifest.get("version"),
            "entries": len(entries),
            "promised": sum(1 for entry in entries.values() if entry["v1"]),
        },
        "rust_src": auditor.std_src,
        "files_scanned": len(auditor.files),
        "references": sum(len(value) for value in referenced.values()),
        "names_referenced": len(referenced),
        "missing": missing,
        "unclassified": unclassified,
        "unused": unused,
        "external_modules": sorted(set(auditor.external_modules)),
    }


def print_text(report: dict) -> None:
    manifest = report["manifest"]
    print("std ABI surface audit (name level)")
    print("rust-src: %s" % report["rust_src"])
    print(
        "manifest: %d entries, %d promised for v1, version %s"
        % (manifest["entries"], manifest["promised"], manifest["version"] or "unknown")
    )
    print(
        "scanned: %d files, %d libc references, %d distinct names"
        % (report["files_scanned"], report["references"], report["names_referenced"])
    )
    print()
    _print_section("MISSING (referenced by std, not promised for v1)", report["missing"])
    _print_section(
        "UNCLASSIFIED (referenced by std, absent from the manifest)",
        report["unclassified"],
    )
    print("UNUSED (manifest promises for v1, std does not reference)")
    print("  Usually a target cfg arm not yet listing irix7, or a crate-facing entry.")
    if report["unused"]:
        for name in report["unused"]:
            print("  %s" % name)
    else:
        print("  (none)")
    print()
    if report["external_modules"]:
        print("note: skipped modules outside the std source tree:")
        for name in report["external_modules"]:
            print("  %s" % name)
        print()
    print(
        "Note: name-level audit only.  It does not check signatures, layouts, errno\n"
        "values or runtime semantics.  See docs/spec/native-libc.md section 9."
    )


def _print_section(title: str, records: list[dict]) -> None:
    print("%s: %d" % (title, len(records)))
    for record in records:
        where = ", ".join(
            "%s:%d" % (item["file"], item["line"]) for item in record["occurrences"][:3]
        )
        suffix = (
            " (+%d more)" % (record["occurrence_count"] - 3)
            if record["occurrence_count"] > 3
            else ""
        )
        reason = record.get("reason")
        print("  %-40s %s%s" % (record["name"], where, suffix))
        if reason:
            print("      %s" % reason)
    if not records:
        print("  (none)")
    print()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="std-abi-surface.py",
        description=(
            "Audit the native IRIX 7 libc names referenced by Rust std against "
            "docs/spec/native-libc-symbols.json.  Read-only and name-level."
        ),
        epilog=(
            "Exit status is 0 unless --strict is given and a name referenced by std "
            "is missing from the v1 profile or unclassified.  In warning mode the "
            "audit always reports success so CI can gate on it later."
        ),
    )
    parser.add_argument(
        "--rust-src",
        default=os.environ.get("RUST_SRC"),
        help="Rust source tree (the directory holding library/std/src or std/src); "
        "defaults to $RUST_SRC, then `rustc --print sysroot`",
    )
    parser.add_argument(
        "--manifest",
        default=DEFAULT_MANIFEST,
        help="machine-readable symbol manifest (default: %(default)s)",
    )
    parser.add_argument("--json", action="store_true", help="emit a JSON report instead of text")
    parser.add_argument(
        "--strict",
        action="store_true",
        help="exit non-zero when missing or unclassified names remain",
    )
    args = parser.parse_args(argv)

    rust_src = args.rust_src
    if not rust_src:
        import subprocess

        try:
            sysroot = subprocess.check_output(
                ["rustc", "--print", "sysroot"], text=True, stderr=subprocess.DEVNULL
            ).strip()
        except (OSError, subprocess.CalledProcessError):
            print(
                "error: --rust-src is required; rustc is not available and RUST_SRC is unset",
                file=sys.stderr,
            )
            return 2
        rust_src = os.path.join(sysroot, "lib", "rustlib", "src", "rust")

    if not os.path.isdir(rust_src):
        print("error: rust source tree not found: %s" % rust_src, file=sys.stderr)
        return 2
    if not os.path.isfile(args.manifest):
        print("error: manifest not found: %s" % args.manifest, file=sys.stderr)
        return 2

    manifest = load_manifest(args.manifest)
    auditor = Auditor(rust_src)
    auditor.run()
    report = build_report(auditor, manifest)

    if args.json:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        print_text(report)

    if args.strict and (report["missing"] or report["unclassified"]):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
