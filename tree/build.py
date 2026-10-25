#!/usr/bin/env python3
"""Generate The Tree of IRIX 7 from tree/roadmap.json.

The roadmap is the source of truth. This script computes the layout, draws the
tree as SVG, and writes a self-contained static page into the output directory.

    python3 tree/build.py                 # writes tree/dist
    python3 tree/build.py --out book/tree # for the GitHub Pages artifact
    python3 tree/build.py --check         # validate only
"""

from __future__ import annotations

import argparse
import html
import json
import math
import random
import shutil
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent
ASSETS = ROOT / "assets"
DATA = ROOT / "roadmap.json"

W, H = 3000, 2300
SOIL = 2000
SUN = (1500, 380)
SPLIT = (1500, 1480)
SEED = (1500, 1990)

# Milestone anchors are explicit; features are grown along their parent's bough.
LEAF_PATHS = {
    "m3": [SEED, (1500, 1790), (1500, 1600)],
    "m4": [SPLIT, (720, 1420), (520, 780), (430, 560)],
    "m5": [SPLIT, (1180, 1340), (1080, 700), (990, 470)],
    "m6": [SPLIT, (1820, 1340), (1820, 1030)],
    "m7": [SPLIT, (1180, 1340), (1080, 700), (990, 470)],
    "m8": [SPLIT, (720, 1420), (520, 780), (430, 560)],
    "m9": [SPLIT, (2280, 1420), (2350, 1080)],
}
LEAF_SEGMENT = {"m3": 0, "m4": 1, "m5": 1, "m6": 1, "m7": 2, "m8": 2, "m9": 1}
LEAF_T = [0.20, 0.38, 0.56, 0.74]

ICONS = {
    "chip": '<rect x="7" y="7" width="10" height="10" rx="1.5"/><path d="M9 3v2M12 3v2M15 3v2M9 19v2M12 19v2M15 19v2M3 9h2M3 12h2M3 15h2M19 9h2M19 12h2M19 15h2"/><rect x="10" y="10" width="4" height="4" rx=".5"/>',
    "wrench": '<path d="M14.7 6.3a4 4 0 0 0-5.4 5.3L4 17l3 3 5.4-5.4a4 4 0 0 0 5.3-5.4l-2.3 2.3-2.9-.6-.6-2.9 2.3-2.3Z"/>',
    "network": '<circle cx="12" cy="5" r="2.2"/><circle cx="5" cy="18" r="2.2"/><circle cx="19" cy="18" r="2.2"/><path d="M12 7.2v4.2M12 11.4 6.4 16M12 11.4l5.6 4.6"/>',
    "cpu": '<rect x="6" y="6" width="12" height="12" rx="2"/><rect x="9" y="9" width="6" height="6" rx="1"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M5 4l2 2M19 4l-2 2M5 20l2-2M19 20l-2-2"/>',
    "terminal": '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="m7 9 3 3-3 3M13 15h4"/>',
    "disk": '<ellipse cx="12" cy="6" rx="7.5" ry="3"/><path d="M4.5 6v6c0 1.7 3.4 3 7.5 3s7.5-1.3 7.5-3V6"/><path d="M4.5 12v6c0 1.7 3.4 3 7.5 3s7.5-1.3 7.5-3v-6"/>',
    "file": '<path d="M6 3h8l4 4v14H6z"/><path d="M14 3v5h5M9 12h6M9 16h6"/>',
    "monitor": '<rect x="3" y="4" width="18" height="13" rx="2"/><path d="M9 21h6M12 17v4"/><path d="m8 9 2 2-2 2M13 13h3"/>',
    "x11": '<path d="m5 5 14 14M19 5 5 19"/><circle cx="12" cy="12" r="9" opacity=".35"/>',
    "layers": '<path d="m12 3 9 5-9 5-9-5 9-5Z"/><path d="m3 12 9 5 9-5M3 16l9 5 9-5"/>',
    "cube": '<path d="m12 3 8 4.5v9L12 21l-8-4.5v-9L12 3Z"/><path d="m4 7.5 8 4.5 8-4.5M12 12v9"/>',
    "flag": '<path d="M6 21V4M6 4h11l-2 4 2 4H6"/>',
    "bolt": '<path d="M13 2 4 14h6l-1 8 9-12h-6l1-8Z"/>',
    "book": '<path d="M4 5a3 3 0 0 1 3-3h13v18H7a3 3 0 0 0-3 3V5Z"/><path d="M4 20a3 3 0 0 1 3-3h13"/>',
    "shield": '<path d="M12 3 20 6v6c0 5-3.4 8-8 9-4.6-1-8-4-8-9V6l8-3Z"/><path d="m9 12 2 2 4-4"/>',
    "spark": '<path d="M12 3v4M12 17v4M3 12h4M17 12h4M6 6l2.5 2.5M15.5 15.5 18 18M18 6l-2.5 2.5M8.5 15.5 6 18"/>',
    "branch": '<path d="M6 21V8a5 5 0 0 1 5-5h7"/><path d="M14 7h4V3M14 13h6v-4"/>',
    "mushroom": '<path d="M4 12a8 8 0 0 1 16 0H4Z"/><path d="M9 12v5a3 3 0 0 0 6 0v-5"/>',
}


# --------------------------------------------------------------------------
# small geometry helpers
# --------------------------------------------------------------------------


@dataclass
class Segment:
    p0: tuple[float, float]
    c1: tuple[float, float]
    c2: tuple[float, float]
    p3: tuple[float, float]

    def point(self, t: float) -> tuple[float, float]:
        mt = 1 - t
        x = (
            mt**3 * self.p0[0]
            + 3 * mt**2 * t * self.c1[0]
            + 3 * mt * t**2 * self.c2[0]
            + t**3 * self.p3[0]
        )
        y = (
            mt**3 * self.p0[1]
            + 3 * mt**2 * t * self.c1[1]
            + 3 * mt * t**2 * self.c2[1]
            + t**3 * self.p3[1]
        )
        return x, y

    def tangent(self, t: float) -> tuple[float, float]:
        mt = 1 - t
        x = (
            3 * mt**2 * (self.c1[0] - self.p0[0])
            + 6 * mt * t * (self.c2[0] - self.c1[0])
            + 3 * t**2 * (self.p3[0] - self.c2[0])
        )
        y = (
            3 * mt**2 * (self.c1[1] - self.p0[1])
            + 6 * mt * t * (self.c2[1] - self.c1[1])
            + 3 * t**2 * (self.p3[1] - self.c2[1])
        )
        length = math.hypot(x, y) or 1
        return x / length, y / length

    def svg(self) -> str:
        return (
            f"M {self.p0[0]:.2f} {self.p0[1]:.2f} "
            f"C {self.c1[0]:.2f} {self.c1[1]:.2f} "
            f"{self.c2[0]:.2f} {self.c2[1]:.2f} "
            f"{self.p3[0]:.2f} {self.p3[1]:.2f}"
        )


def catmull_segments(points: list[tuple[float, float]]) -> list[Segment]:
    """Catmull-Rom through the points, converted to cubic Béziers."""
    pts = [tuple(map(float, p)) for p in points]
    segs: list[Segment] = []
    for i in range(len(pts) - 1):
        p0 = pts[i - 1] if i > 0 else pts[i]
        p1 = pts[i]
        p2 = pts[i + 1]
        p3 = pts[i + 2] if i + 2 < len(pts) else p2
        c1 = (p1[0] + (p2[0] - p0[0]) / 6, p1[1] + (p2[1] - p0[1]) / 6)
        c2 = (p2[0] - (p3[0] - p1[0]) / 6, p2[1] - (p3[1] - p1[1]) / 6)
        segs.append(Segment(p1, c1, c2, p2))
    return segs


def catmull_path(points: list[tuple[float, float]]) -> str:
    return " ".join(seg.svg() for seg in catmull_segments(points))


def esc(text: str) -> str:
    return html.escape(str(text), quote=True)


def clamp(value: float, low: float, high: float) -> float:
    return max(low, min(high, value))


# --------------------------------------------------------------------------
# data + layout
# --------------------------------------------------------------------------


class Roadmap:
    def __init__(self, data: dict):
        self.data = data
        self.meta = data["meta"]
        self.statuses = data["statuses"]
        self.branches = data["branches"]
        self.nodes = data["nodes"]
        self.by_id = {n["id"]: n for n in self.nodes}
        self.children: dict[str, list[dict]] = {}
        for node in self.nodes:
            if node.get("parent"):
                self.children.setdefault(node["parent"], []).append(node)
        self.validate()

    def validate(self) -> None:
        assert len(self.by_id) == len(self.nodes), "duplicate node ids"
        for node in self.nodes:
            for key in ("kind", "branch", "status", "title", "summary"):
                assert key in node, f"{node['id']} is missing {key}"
            assert node["status"] in self.statuses, f"{node['id']}: bad status"
            assert node["branch"] in self.branches, f"{node['id']}: bad branch"
            if node.get("parent"):
                assert node["parent"] in self.by_id, f"{node['id']}: missing parent"
            if node["kind"] in ("milestone", "release"):
                assert "anchor" in node, f"{node['id']} needs an anchor"
            if node["kind"] == "feature":
                assert node["parent"] in self.by_id, f"{node['id']} has no parent"
                assert self.by_id[node["parent"]]["kind"] == "milestone"

    def milestones(self) -> list[dict]:
        return [n for n in self.nodes if n["kind"] == "milestone"]

    def counts(self) -> dict[str, int]:
        result = {s: 0 for s in self.statuses}
        for node in self.nodes:
            if node["status"] == "pruned":
                result["pruned"] += 1
            elif node["kind"] == "milestone":
                result[node["status"]] += 1
        return result

    def children_of(self, node_id: str, kind: str | None = None) -> list[dict]:
        kids = self.children.get(node_id, [])
        if kind:
            kids = [k for k in kids if k["kind"] == kind]
        return kids

    def ancestors(self, node_id: str) -> list[str]:
        out = []
        node = self.by_id.get(node_id)
        while node and node.get("parent"):
            out.append(node["parent"])
            node = self.by_id.get(node["parent"])
        return out

    def place_features(self) -> None:
        for milestone in self.milestones():
            kids = self.children_of(milestone["id"], "feature")
            if not kids:
                continue
            points = LEAF_PATHS[milestone["id"]]
            seg = catmull_segments(points)[LEAF_SEGMENT[milestone["id"]]]
            for i, child in enumerate(kids):
                t = LEAF_T[i % len(LEAF_T)]
                px, py = seg.point(t)
                tx, ty = seg.tangent(t)
                side = -1 if i % 2 == 0 else 1
                offset = 102 + 14 * (i % 2)
                nx, ny = -ty, tx
                x, y = px + nx * offset * side, py + ny * offset * side
                # keep labels on the canvas and off the trunk
                child["_x"], child["_y"] = round(x, 1), round(y, 1)
                child["_side"] = side
                child["_twig"] = (round(px, 1), round(py, 1))

    def place_lists(self) -> None:
        workstreams = [n for n in self.nodes if n["kind"] == "workstream"]
        for i, node in enumerate(workstreams):
            node["_x"], node["_y"] = 2604, 2044 + i * 33
        pruned = [n for n in self.nodes if n["kind"] == "pruned"]
        for i, node in enumerate(pruned):
            node["_x"] = 168 + (i // 5) * 236
            node["_y"] = 2036 + (i % 5) * 37

    def decorate(self) -> None:
        for node in self.nodes:
            node["_status_colour"] = node.get("accent") or self.statuses[node["status"]]["colour"]
            node["_status_label"] = self.statuses[node["status"]]["label"]
        for node in self.nodes:
            if "anchor" in node:
                node["_x"], node["_y"] = node["anchor"]
        for node in self.nodes:
            if node.get("parent") and node["parent"] in self.by_id:
                parent = self.by_id[node["parent"]]
                if "anchor" in parent:
                    node["_parent_pos"] = parent["anchor"]

    def layout(self) -> None:
        self.decorate()
        self.place_features()
        self.place_lists()


# --------------------------------------------------------------------------
# SVG pieces
# --------------------------------------------------------------------------


def starfield(rng: random.Random) -> str:
    out = []
    for _ in range(340):
        x = rng.uniform(-500, W + 500)
        y = rng.uniform(-80, 1960)
        if (x - SUN[0]) ** 2 + (y - SUN[1]) ** 2 < 300**2:
            continue
        r = rng.choice([0.5, 0.6, 0.8, 0.9, 1.1, 1.4, 1.8])
        o = rng.uniform(0.18, 0.8)
        delay = rng.uniform(0, 9)
        cls = "star twinkle" if rng.random() < 0.22 else "star"
        out.append(
            f'<circle class="{cls}" cx="{x:.0f}" cy="{y:.0f}" r="{r}" '
            f'opacity="{o:.2f}" style="animation-delay:-{delay:.1f}s"/>'
        )
    for x, y, s in [(420, 310, 10), (2580, 250, 8), (2380, 690, 6), (760, 890, 6)]:
        out.append(
            f'<path class="star-cross" d="M{x - s} {y}H{x + s}M{x} {y - s}V{y + s}"/>'
        )
    return "\n".join(out)


def ranges(rng: random.Random) -> str:
    def ridge(base: int, amp: int, step: int, seed_shift: int) -> str:
        r = random.Random(seed_shift)
        pts = []
        x = -900
        while x < W + 900:
            y = base + math.sin(x / 210) * amp * 0.35 + r.uniform(-amp * 0.35, amp * 0.35)
            pts.append((x, y))
            x += step
        d = f"M -900 {SOIL + 10} L -900 {pts[0][1]:.1f} "
        for i in range(1, len(pts)):
            px, py = pts[i - 1]
            x, y = pts[i]
            d += f"Q {px + (x - px) / 2:.1f} {min(py, y) - r.uniform(0, 18):.1f} {x:.1f} {y:.1f} "
        d += f"L {W + 900} {SOIL + 10} Z"
        return d

    return (
        f'<path class="ridge far" d="{ridge(1880, 70, 120, 41)}"/>'
        f'<path class="ridge near" d="{ridge(1940, 45, 90, 77)}"/>'
    )


def grass(rng: random.Random) -> str:
    out = []
    for _ in range(160):
        x = rng.uniform(-500, W + 500)
        h = rng.uniform(5, 22)
        lean = rng.uniform(-7, 7)
        o = rng.uniform(0.12, 0.4)
        colour = rng.choice(["#63d8c6", "#ff9a72", "#ffd37a"])
        out.append(
            f'<path d="M{x:.0f} {SOIL} q {lean / 2:.1f} {-h / 2:.1f} {lean:.1f} {-h:.1f}" '
            f'stroke="{colour}" stroke-opacity="{o:.2f}" stroke-width="1.2" fill="none"/>'
        )
    return "\n".join(out)


def root_hairs(rng: random.Random, path: list[tuple[float, float]], count: int) -> str:
    segs = catmull_segments(path)
    out = []
    for i in range(count):
        seg = segs[min(i % len(segs), len(segs) - 1)]
        t = rng.uniform(0.15, 0.9)
        px, py = seg.point(t)
        tx, ty = seg.tangent(t)
        nx, ny = -ty, tx
        side = -1 if i % 2 == 0 else 1
        length = rng.uniform(12, 38)
        bend = rng.uniform(-12, 12)
        x1 = px + nx * length * side
        y1 = py + ny * length * side + rng.uniform(4, 16)
        out.append(
            f'<path d="M{px:.0f} {py:.0f} Q {px + nx * length * side * 0.4:.0f} '
            f'{py + ny * length * side * 0.4 + bend:.0f} {x1:.0f} {y1:.0f}" '
            f'class="root-hair"/>'
        )
    return "\n".join(out)


def mushrooms() -> str:
    out = []
    for x, y, s in [(322, 2113, 1.0), (678, 2152, 0.8), (704, 2072, 0.6)]:
        out.append(
            f'<g class="mushroom" transform="translate({x},{y}) scale({s})">'
            '<path d="M-10 0a10 8 0 0 1 20 0Z" class="cap"/>'
            '<path d="M-2 0v8a2 2 0 0 0 4 0V0" class="stem"/>'
            '<circle cx="-4" cy="-3" r="1.1" class="spot"/>'
            '<circle cx="4" cy="-4" r=".8" class="spot"/>'
            "</g>"
        )
    return "\n".join(out)


def indy_easter_egg() -> str:
    return """
<g class="indy-egg" transform="translate(2130,2172)" aria-hidden="true">
  <rect x="0" y="28" width="62" height="13" rx="2" class="indy-case"/>
  <path d="M6 31h9M52 31h4" class="indy-detail"/>
  <rect x="12" y="0" width="42" height="30" rx="2" class="indy-case"/>
  <rect x="17" y="5" width="32" height="20" rx="1" class="indy-screen"/>
  <text x="33" y="19" class="indy-text">6.5</text>
  <path d="M28 41v5h8v-5" class="indy-detail"/>
</g>"""


def icon(name: str) -> str:
    return ICONS[name]


MILESTONE_ICONS = {
    "m0": "wrench",
    "m1": "bolt",
    "m2": "cpu",
    "m3": "terminal",
    "m4": "disk",
    "m5": "file",
    "m6": "monitor",
    "m7": "x11",
    "m8": "layers",
    "m9": "cube",
    "m10": "flag",
}


def draw_background(rng: random.Random) -> str:
    return f"""
<g id="background" aria-hidden="true">
  <rect x="-3000" y="-1800" width="9000" height="6200" fill="url(#sky)"/>
  <g id="astrolabe">
    <circle cx="{SUN[0]}" cy="{SUN[1]}" r="332" class="orbit"/>
    <circle cx="{SUN[0]}" cy="{SUN[1]}" r="430" class="orbit faint"/>
    <circle cx="{SUN[0]}" cy="{SUN[1]}" r="570" class="orbit fainter"/>
    <circle cx="{SUN[0]}" cy="{SUN[1]}" r="730" class="orbit fainter"/>
    <path d="M{SUN[0] - 760} {SUN[1]}H{SUN[0] + 760}M{SUN[0]} {SUN[1] - 760}V{SOIL - 20}" class="axis"/>
  </g>
  <g id="stars">{starfield(rng)}</g>
  <g id="sun">
    <ellipse cx="{SUN[0]}" cy="{SUN[1]}" rx="500" ry="112" transform="rotate(-11 {SUN[0]} {SUN[1]})" class="planet-ring"/>
    <ellipse cx="{SUN[0]}" cy="{SUN[1]}" rx="556" ry="132" transform="rotate(-11 {SUN[0]} {SUN[1]})" class="planet-ring faint"/>
    <circle cx="{SUN[0]}" cy="{SUN[1]}" r="330" fill="url(#sunHalo)"/>
    <circle cx="{SUN[0]}" cy="{SUN[1]}" r="228" fill="url(#sunCore)"/>
    <circle cx="{SUN[0]}" cy="{SUN[1]}" r="316" class="sun-ring"/>
    <circle cx="{SUN[0]}" cy="{SUN[1]}" r="398" class="sun-ring faint"/>
  </g>
  <g id="ridges">{ranges(rng)}</g>
  <rect x="-3000" y="{SOIL}" width="9000" height="4200" fill="url(#soil)"/>
  <ellipse cx="1500" cy="2070" rx="820" ry="240" fill="url(#soilGlow)"/>
  <g id="grass">{grass(rng)}</g>
  <path d="M-2000 {SOIL}H{W + 2000}" class="horizon"/>
  <path d="M-2000 {SOIL + 2}H{W + 2000}" class="horizon-glow"/>
</g>"""


def draw_roots(rng: random.Random) -> str:
    roots = [
        ([SEED, (1280, 2038), (1050, 2090)], 30),
        ([SEED, (1500, 2080), (1500, 2180)], 36),
        ([SEED, (1710, 2038), (1950, 2090)], 30),
    ]
    out = ['<g id="roots" aria-hidden="true">']
    for path, hairs in roots:
        d = catmull_path(path)
        out.append(f'<path d="{d}" class="root-underlay"/>')
        out.append(f'<path d="{d}" class="root-main"/>')
        out.append(f'<path d="{d}" class="root-light"/>')
        out.append(root_hairs(rng, path, hairs))
    out.append("</g>")
    return "\n".join(out)


def draw_boughs() -> str:
    boughs = [
        ("storage", [SPLIT, (720, 1420), (520, 780), (430, 560)], [6.8, 5.0, 3.2]),
        ("compat", [SPLIT, (1180, 1340), (1080, 700), (990, 470)], [7.4, 5.6, 3.6]),
        ("graphics", [SPLIT, (1820, 1340), (1820, 1030)], [6.4, 4.2]),
        ("native", [SPLIT, (2280, 1420), (2350, 1080)], [6.6, 4.4]),
    ]
    out = []
    for name, points, widths in boughs:
        segs = catmull_segments(points)
        for i, seg in enumerate(segs):
            width = widths[min(i, len(widths) - 1)]
            out.append(
                f'<path d="{seg.svg()}" class="bough-shadow" stroke-width="{width + 7:.1f}"/>'
            )
            out.append(
                f'<path d="{seg.svg()}" class="bough" data-branch="{name}" '
                f'stroke-width="{width:.1f}"/>'
            )
            out.append(
                f'<path d="{seg.svg()}" class="bough-light" stroke-width="{max(.8, width * .15):.1f}"/>'
            )
    return "\n".join(out)


def draw_leaves(rng: random.Random) -> str:
    """Small leaves and spores that give the boughs texture without labels."""
    paths = [
        ([SPLIT, (720, 1420), (520, 780), (430, 560)], "storage"),
        ([SPLIT, (1180, 1340), (1080, 700), (990, 470)], "compat"),
        ([SPLIT, (1820, 1340), (1820, 1030)], "graphics"),
        ([SPLIT, (2280, 1420), (2350, 1080)], "native"),
        ([SEED, (1500, 1810), (1500, 1600)], "control"),
    ]
    out = ['<g id="leaves" aria-hidden="true">']
    for points, branch in paths:
        colour = {"storage": "#63d8c6", "compat": "#8ab8ff", "graphics": "#f49ad4", "native": "#c9f08a", "control": "#ffd37a"}[branch]
        for seg in catmull_segments(points):
            for _ in range(16):
                t = rng.uniform(0.08, 0.94)
                px, py = seg.point(t)
                tx, ty = seg.tangent(t)
                nx, ny = -ty, tx
                side = -1 if rng.random() < 0.5 else 1
                off = rng.uniform(6, 23)
                x, y = px + nx * off * side, py + ny * off * side
                angle = math.degrees(math.atan2(ty, tx)) + rng.uniform(-18, 18)
                length = rng.uniform(4.5, 9.5)
                width = rng.uniform(1.1, 2.2)
                o = rng.uniform(0.12, 0.34)
                out.append(
                    f'<ellipse cx="{x:.1f}" cy="{y:.1f}" rx="{length:.1f}" ry="{width:.1f}" '
                    f'fill="{colour}" opacity="{o:.2f}" transform="rotate({angle:.1f} {x:.1f} {y:.1f})"/>'
                )
    # drifting spores
    for _ in range(34):
        x = rng.uniform(140, W - 140)
        y = rng.uniform(420, 1980)
        if (x - SUN[0]) ** 2 + (y - SUN[1]) ** 2 < 240**2:
            continue
        r = rng.uniform(0.7, 2.0)
        colour = rng.choice(["#ffd37a", "#63d8c6", "#ff9a72"])
        out.append(
            f'<circle cx="{x:.0f}" cy="{y:.0f}" r="{r:.1f}" fill="{colour}" opacity="{rng.uniform(.10, .35):.2f}"/>'
        )
    out.append("</g>")
    return "\n".join(out)


def draw_trunk() -> str:
    trunk = [SEED, (1500, 1810), (1500, 1600), SPLIT]
    segs = catmull_segments(trunk)
    out = []
    for i, seg in enumerate(segs):
        width = [14.5, 12.5, 9.5][min(i, 2)]
        out.append(f'<path d="{seg.svg()}" class="bough-shadow" stroke-width="{width + 10}"/>')
        out.append(f'<path d="{seg.svg()}" class="trunk" stroke-width="{width}"/>')
        out.append(
            f'<path d="{seg.svg()}" class="trunk-light" stroke-width="{max(1.4, width * .12):.1f}"/>'
        )
    out.append('<circle cx="1500" cy="1480" r="5.5" class="split-hub"/>')
    return "\n".join(out)


def draw_dependencies(roadmap: Roadmap) -> str:
    out = ['<g id="dependencies" aria-hidden="true">']
    for dep in roadmap.data["dependencies"]:
        a = roadmap.by_id[dep["from"]]
        b = roadmap.by_id[dep["to"]]
        ax, ay = a["anchor"]
        bx, by = b["anchor"]
        if b["id"] == "m10":
            # canopy convergence: rise up and inward to the sun
            d = (
                f"M {ax} {ay} C {ax + (bx - ax) * .2} {ay - 190}, "
                f"{bx + (ax - bx) * .32} {by + 210}, {bx} {by + 30}"
            )
            out.append(f'<path d="{d}" class="converge"/>')
        else:
            dx, dy = bx - ax, by - ay
            cx = (ax + bx) / 2 - dy * 0.16
            cy = (ay + by) / 2 + dx * 0.16
            d = f"M {ax} {ay} Q {cx:.1f} {cy:.1f} {bx} {by}"
            out.append(f'<path d="{d}" class="cross-link"/>')
    out.append("</g>")
    return "\n".join(out)


def node_base(node: dict, extra_class: str = "") -> str:
    status = node["status"]
    colour = node["_status_colour"]
    label = f'{node.get("code", "")} {node["title"]}'.strip()
    aria = f'{label}, {node["_status_label"]}'
    return (
        f'<g class="node node-{node["kind"]} st-{status} {extra_class}" '
        f'data-id="{node["id"]}" data-kind="{node["kind"]}" '
        f'data-status="{status}" data-branch="{node["branch"]}" '
        f'data-parent="{node.get("parent", "")}" tabindex="0" role="button" '
        f'aria-label="{esc(aria)}" style="--s:{colour}" '
        f'transform="translate({node["_x"]},{node["_y"]})">'
        f"<title>{esc(node.get('fullTitle') or node['title'])} — {esc(node['_status_label'])}</title>"
    )


def draw_milestone(node: dict) -> str:
    x, y = node.get("anchor", (node["_x"], node["_y"]))
    node["_x"], node["_y"] = x, y
    root = node["id"] in ("m0", "m1", "m2")
    r = 38 if node["id"] == "m10" else (24 if root else 29)
    icon_size = 26 if node["id"] == "m10" else 22
    cls = "is-root" if root else ""
    out = [node_base(node, cls)]
    out.append(f'<circle class="node-halo" r="{r + 18}"/>')
    out.append(f'<circle class="node-ring" r="{r}"/>')
    out.append(f'<circle class="node-disc" r="{r - 5}"/>')
    out.append(
        f'<g class="node-icon" transform="translate({-icon_size / 2:.1f},{-icon_size / 2:.1f}) '
        f'scale({icon_size / 24:.3f})">{icon(MILESTONE_ICONS[node["id"]])}</g>'
    )
    if root:
        out.append(f'<text class="m-code" y="{r + 20}">{esc(node["code"])}</text>')
        out.append(f'<text class="m-title-sm" y="{r + 38}">{esc(node["title"])}</text>')
        out.append(f'<text class="m-status" y="{r + 52}">{esc(node["_status_label"])}</text>')
    elif node["id"] == "m3":
        out.append(f'<text class="m-code" x="-48" y="-7" text-anchor="end">{esc(node["code"])}</text>')
        out.append(
            f'<text class="m-title" x="-48" y="15" text-anchor="end">{esc(node["title"])}</text>'
        )
        out.append(
            f'<text class="m-status" x="-48" y="31" text-anchor="end">{esc(node["_status_label"])}</text>'
        )
    else:
        out.append(f'<text class="m-code" y="{r + 20}">{esc(node["code"])}</text>')
        out.append(f'<text class="m-title" y="{r + 42}">{esc(node["title"])}</text>')
        out.append(f'<text class="m-status" y="{r + 58}">{esc(node["_status_label"])}</text>')
    out.append("</g>")
    return "\n".join(out)


def draw_seed(node: dict) -> str:
    node["_x"], node["_y"] = SEED
    out = [node_base(node, "is-seed")]
    out.append('<circle class="seed-halo" r="44"/>')
    out.append('<circle class="seed-ring" r="25"/>')
    out.append('<circle class="seed-disc" r="20"/>')
    out.append(
        '<g class="node-icon" transform="translate(-11,-11) scale(.92)">'
        + icon("chip")
        + "</g>"
    )
    out.append('<text class="seed-title" x="38" y="0">IRIX 7</text>')
    out.append('<text class="seed-sub" x="38" y="17">IP24 · MIPS R4400</text>')
    out.append("</g>")
    return "\n".join(out)


def draw_feature(node: dict) -> str:
    x, y = node["_x"], node["_y"]
    side = node["_side"]
    anchor = "end" if side < 0 else "start"
    lx = -14 if side < 0 else 14
    out = [node_base(node)]
    out.append('<circle class="feature-halo" r="13"/>')
    out.append('<circle class="feature-ring" r="6"/>')
    out.append('<circle class="feature-core" r="2.6"/>')
    out.append(
        f'<text class="f-label" x="{lx}" y="4" text-anchor="{anchor}">{esc(node["title"])}</text>'
    )
    out.append("</g>")
    return "\n".join(out)


def draw_release(node: dict) -> str:
    x, y = node["anchor"]
    node["_x"], node["_y"] = x, y
    parent = node["parent"]
    out = [node_base(node)]
    if parent in ("m3", "m5", "m6", "m7", "m8", "m9", "m10"):
        # a fine stem back to the branch, in the group's local coordinates
        parent_node = node["_parent_pos"]
        px, py = parent_node
        d = f"M {px - x} {py - y} Q {(px - x) * .55} {(py - y) * .55 + 14} 0 0"
        out.append(f'<path class="release-stem" d="{d}"/>')
    out.append('<path class="fruit" d="M0 -10 L8 0 L0 10 L-8 0 Z"/>')
    out.append('<circle class="fruit-core" r="2.4"/>')
    out.append(f'<text class="r-label" x="16" y="0">{esc(node["version"])}</text>')
    if node["id"] == "v1.0":
        out.append(f'<text class="r-sub" x="16" y="14">{esc(node["title"])}</text>')
    out.append("</g>")
    return "\n".join(out)


def draw_workstream(node: dict) -> str:
    out = [node_base(node, "is-workstream")]
    out.append('<circle class="ws-halo" r="10"/>')
    out.append('<circle class="ws-dot" r="3.6"/>')
    out.append(f'<text class="ws-label" x="14" y="4">{esc(node["title"])}</text>')
    out.append("</g>")
    return "\n".join(out)


def draw_pruned(node: dict) -> str:
    out = [node_base(node, "is-pruned")]
    out.append('<circle class="pruned-halo" r="14"/>')
    out.append('<path class="pruned-x" d="M-9 -4 l6 6 m0 -6 l-6 6" transform="translate(-2,0)"/>')
    out.append(f'<text class="p-label" x="5" y="4">{esc(node["title"])}</text>')
    out.append("</g>")
    return "\n".join(out)


def draw_workstreams(roadmap: Roadmap) -> str:
    nodes = [n for n in roadmap.nodes if n["kind"] == "workstream"]
    out = ['<g id="mycelium">']
    out.append(
        '<text class="list-head" x="2564" y="1988">ALWAYS GROWING</text>'
        '<text class="list-sub" x="2564" y="2004">cross-cutting workstreams</text>'
    )
    out.append(
        '<path class="mycelium-thread" d="M1500 1990 C 1900 2010, 2260 1998, 2564 2044 '
        'L 2564 2246"/>'
    )
    for node in nodes:
        out.append(draw_workstream(node))
    out.append("</g>")
    return "\n".join(out)


def draw_compost(roadmap: Roadmap) -> str:
    nodes = [n for n in roadmap.nodes if n["kind"] == "pruned"]
    out = ['<g id="compost">']
    out.append(
        '<path class="compost-mound" d="M120 2262 C 190 2072, 470 1988, 700 2004 '
        'C 790 2012, 810 2110, 828 2262 Z"/>'
    )
    out.append(
        '<path class="compost-contour" d="M180 2180 C 300 2080, 560 2040, 760 2072"/>'
    )
    out.append(
        '<path class="compost-contour" d="M150 2232 C 320 2140, 600 2120, 800 2160"/>'
    )
    out.append(
        '<text class="list-head" x="168" y="1962">THE COMPOST</text>'
        '<text class="list-sub" x="168" y="1978">pruned from v1.0</text>'
    )
    out.append(mushrooms())
    for node in nodes:
        out.append(draw_pruned(node))
    out.append("</g>")
    return "\n".join(out)


def draw_canopy() -> str:
    return f"""
<g id="canopy" aria-hidden="true">
  <text class="canopy-label" x="{SUN[0]}" y="{SUN[1] + 210}">THE CANOPY</text>
</g>"""


# --------------------------------------------------------------------------
# page
# --------------------------------------------------------------------------


def svg_page(roadmap: Roadmap) -> str:
    rng = random.Random(7)

    milestones = roadmap.milestones()
    features = [n for n in roadmap.nodes if n["kind"] == "feature"]
    releases = [n for n in roadmap.nodes if n["kind"] == "release"]

    body = []
    body.append(draw_background(rng))
    body.append(draw_roots(rng))
    body.append('<g id="tree">')
    body.append(draw_dependencies(roadmap))
    body.append(draw_trunk())
    body.append(draw_boughs())
    body.append(draw_leaves(rng))
    body.append('<g id="twigs">')
    for f in features:
        x, y = f["_x"], f["_y"]
        tx, ty = f["_twig"]
        body.append(
            f'<path class="twig" d="M{tx:.1f} {ty:.1f} Q {(tx + x) / 2:.1f} '
            f'{(ty + y) / 2 + 8:.1f} {x:.1f} {y:.1f}"/>'
        )
    body.append("</g>")
    body.append("</g>")

    body.append('<g id="nodes">')
    body.append('<g id="nodes">')
    body.append(draw_seed(roadmap.by_id["seed"]))
    for node in milestones:
        body.append(draw_milestone(node))
    for node in features:
        body.append(draw_feature(node))
    for node in releases:
        body.append(draw_release(node))
    body.append("</g>")

    body.append(draw_workstreams(roadmap))
    body.append(draw_compost(roadmap))
    body.append(draw_canopy())
    body.append(indy_easter_egg())

    # A simplified copy for the minimap.
    mini = ['<g id="minimap-content">']
    for seg in catmull_segments([SEED, (1500, 1810), (1500, 1600), SPLIT]):
        mini.append(f'<path d="{seg.svg()}" class="mini-bough"/>')
    for points in [
        [SPLIT, (720, 1420), (520, 780), (430, 560)],
        [SPLIT, (1180, 1340), (1080, 700), (990, 470)],
        [SPLIT, (1820, 1340), (1820, 1030)],
        [SPLIT, (2280, 1420), (2350, 1080)],
    ]:
        for seg in catmull_segments(points):
            mini.append(f'<path d="{seg.svg()}" class="mini-bough"/>')
    for node in milestones:
        mini.append(
            f'<circle cx="{node["_x"]}" cy="{node["_y"]}" r="34" class="mini-node" '
            f'style="fill:{node["_status_colour"]}"/>'
        )
    mini.append("</g>")

    return f"""
<svg id="tree" viewBox="0 0 {W} {H}" preserveAspectRatio="xMidYMid meet"
     role="group" aria-labelledby="tree-title" aria-describedby="tree-desc">
  <title id="tree-title">The Tree of IRIX 7</title>
  <desc id="tree-desc">An upward-growing technology tree of the IRIX 7 roadmap. Milestones M0 to M10 grow from the roots through the control plane, storage, compatibility, graphics and native ABI branches to the v1.0 release in the canopy.</desc>
  <defs>
    <radialGradient id="sky" gradientUnits="userSpaceOnUse" cx="1500" cy="620" r="2500">
      <stop offset="0" stop-color="#141d3a"/>
      <stop offset=".42" stop-color="#0a1022"/>
      <stop offset="1" stop-color="#05070f"/>
    </radialGradient>
    <radialGradient id="sunHalo" cx="50%" cy="50%" r="50%">
      <stop offset="0" stop-color="#ffcf77" stop-opacity=".78"/>
      <stop offset=".38" stop-color="#ff9a72" stop-opacity=".3"/>
      <stop offset="1" stop-color="#ff7a64" stop-opacity="0"/>
    </radialGradient>
    <radialGradient id="sunCore" cx="50%" cy="50%" r="50%">
      <stop offset="0" stop-color="#ffe3a2" stop-opacity=".55"/>
      <stop offset=".5" stop-color="#ffb074" stop-opacity=".2"/>
      <stop offset="1" stop-color="#ff7a64" stop-opacity="0"/>
    </radialGradient>
    <radialGradient id="soilGlow" cx="50%" cy="50%" r="50%">
      <stop offset="0" stop-color="#ff8f6b" stop-opacity=".14"/>
      <stop offset=".55" stop-color="#b85f4a" stop-opacity=".05"/>
      <stop offset="1" stop-color="#000000" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="soil" gradientUnits="userSpaceOnUse" x1="0" y1="{SOIL}" x2="0" y2="{H}">
      <stop offset="0" stop-color="#080d1b"/>
      <stop offset="1" stop-color="#03050b"/>
    </linearGradient>
    <linearGradient id="boughGrad" gradientUnits="userSpaceOnUse" x1="0" y1="{SOIL}" x2="0" y2="360">
      <stop offset="0" stop-color="#ff8a63"/>
      <stop offset=".55" stop-color="#ffb27c"/>
      <stop offset="1" stop-color="#ffe09a"/>
    </linearGradient>
    <linearGradient id="trunkGrad" gradientUnits="userSpaceOnUse" x1="0" y1="{SOIL}" x2="0" y2="1450">
      <stop offset="0" stop-color="#e87958"/>
      <stop offset=".6" stop-color="#ff9a72"/>
      <stop offset="1" stop-color="#ffc084"/>
    </linearGradient>
    <filter id="softGlow" x="-80%" y="-80%" width="260%" height="260%">
      <feGaussianBlur stdDeviation="7"/>
    </filter>
    <filter id="tinyGlow" x="-120%" y="-120%" width="340%" height="340%">
      <feGaussianBlur stdDeviation="2.2"/>
    </filter>
  </defs>
  <g id="world">
    {''.join(body)}
  </g>
  <defs>{''.join(mini)}</defs>
</svg>"""


def build_page(roadmap: Roadmap) -> str:
    data = roadmap.data
    counts = roadmap.counts()
    active = [m for m in roadmap.milestones() if m["status"] == "active"]
    now = active[-1] if active else roadmap.milestones()[0]
    total_milestones = len(roadmap.milestones())
    releases = len([n for n in roadmap.nodes if n["kind"] == "release"])

    status_chips = []
    for key, status in data["statuses"].items():
        if key == "pruned":
            continue
        status_chips.append(
            f'<button class="lg-chip" data-filter="{key}" style="--s:{status["colour"]}">'
            f'<i></i>{esc(status["label"])}<span>{counts[key]}</span></button>'
        )

    rail = []
    for m in roadmap.milestones():
        rail.append(
            f'<button class="rail-item" data-node="{m["id"]}" style="--s:{m["_status_colour"]}">'
            f'<i></i><span class="code">{esc(m["code"])}</span>'
            f'<span class="name">{esc(m["title"])}</span></button>'
        )

    sr_nodes = []
    for n in data["nodes"]:
        if n["kind"] in ("milestone", "release", "workstream", "pruned"):
            sr_nodes.append(f'<li><a href="#{n["id"]}">{esc(n["title"])}</a></li>')

    json_blob = json.dumps(data, ensure_ascii=False, separators=(",", ":")).replace("</", "<\\/")

    return f"""<!doctype html>
<html lang="en-AU">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<title>The Tree of IRIX 7 — a living roadmap</title>
<meta name="description" content="{esc(data["meta"]["description"])}">
<meta name="theme-color" content="#060a16">
<meta name="color-scheme" content="dark">
<link rel="canonical" href="{esc(data["meta"]["site"])}">
<meta property="og:type" content="website">
<meta property="og:site_name" content="IRIX 7">
<meta property="og:title" content="The Tree of IRIX 7">
<meta property="og:description" content="{esc(data["meta"]["description"])}">
<meta property="og:url" content="{esc(data["meta"]["site"])}">
<meta property="og:image" content="{esc(data["meta"]["site"])}assets/og.png">
<meta property="og:image:type" content="image/png">
<meta property="og:image:width" content="1200">
<meta property="og:image:height" content="630">
<meta property="og:image:alt" content="The Tree of IRIX 7 — a living roadmap">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="The Tree of IRIX 7">
<meta name="twitter:description" content="{esc(data["meta"]["description"])}">
<meta name="twitter:image" content="{esc(data["meta"]["site"])}assets/og.png">
<link rel="icon" href="assets/favicon.svg" type="image/svg+xml">
<link rel="preload" href="assets/fonts/fraunces.woff2" as="font" type="font/woff2" crossorigin>
<link rel="preload" href="assets/fonts/space-grotesk.woff2" as="font" type="font/woff2" crossorigin>
<link rel="stylesheet" href="assets/tree.css">
</head>
<body>
<noscript>
  <style>
    .intro, .search-layer, .panel, .hud, .rail, .now-growing, .grain {{ display: none !important; }}
    .search-trigger, [data-open-about] {{ display: none !important; }}
    .node {{ cursor: default; }}
  </style>
</noscript>
<a class="skip" href="#node-list">Skip to the roadmap list</a>
<div id="app">
  <div id="stage" aria-label="Interactive roadmap tree. Drag to pan, scroll to zoom." tabindex="-1">
    {svg_page(roadmap)}
  </div>

  <header class="topbar">
    <a class="brand" href="./" aria-label="The Tree of IRIX 7">
      <svg class="prism" viewBox="0 0 32 32" aria-hidden="true">
        <path d="M16 3 29 10.5v11L16 29 3 21.5v-11L16 3Z" class="prism-face"/>
        <path d="M16 3v26M3 10.5l13 7.5 13-7.5" class="prism-edge"/>
        <path d="M16 18v11" class="prism-cut"/>
      </svg>
      <span class="brand-text">
        <span class="brand-name">IRIX 7</span>
        <span class="brand-sub">The Tree</span>
      </span>
    </a>
    <div class="top-actions">
      <button class="search-trigger" data-open-search aria-label="Find a node" aria-keyshortcuts="Control+K Meta+K">
        <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="6.5"/><path d="m16 16 4.5 4.5"/></svg>
        <span>Find a node</span>
        <kbd>⌘K</kbd>
      </button>
      <a class="ghost-btn" href="{esc(data["meta"]["docs"])}">Docs</a>
      <a class="icon-btn" href="{esc(data["meta"]["repo"])}" aria-label="Source on GitHub" title="Source on GitHub">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m8 8-4 4 4 4M16 8l4 4-4 4M13 5l-2 14"/></svg>
      </a>
      <button class="icon-btn" data-open-about aria-label="How to read this tree" title="How to read this tree">
        <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="M9.5 9a2.6 2.6 0 1 1 3.7 2.4c-.8.4-1.2 1-1.2 1.9M12 17.2v.1"/></svg>
      </button>
    </div>
  </header>

  <div class="hud hud-left">
    <div class="legend" aria-label="Filter by status">
      <span class="hud-label">Status</span>
      {''.join(status_chips)}
      <button class="lg-chip lg-pruned" data-filter="pruned" style="--s:{data["statuses"]["pruned"]["colour"]}">
        <i></i>Pruned<span>{counts["pruned"]}</span>
      </button>
    </div>
  </div>

  <div class="hud hud-right">
    <div class="minimap-wrap" aria-hidden="true">
      <svg id="minimap" viewBox="0 0 {W} {H}">
        <use href="#minimap-content"/>
        <rect id="mini-view" x="0" y="0" width="100" height="100"/>
      </svg>
    </div>
    <div class="zoom" role="group" aria-label="Zoom controls">
      <button data-zoom="in" aria-label="Zoom in">+</button>
      <button data-zoom="out" aria-label="Zoom out">−</button>
      <button data-zoom="fit" aria-label="Fit the whole tree">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/></svg>
      </button>
    </div>
  </div>

  <nav class="rail" aria-label="Milestones">{''.join(rail)}</nav>

  <div class="now-growing" id="now-growing">
    <span class="ng-label">Now growing</span>
    <button class="ng-link" data-node="{now["id"]}">
      <i style="--s:{now["_status_colour"]}"></i>
      <span>{esc(now["code"])} · {esc(now["title"])}</span>
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12h14M13 6l6 6-6 6"/></svg>
    </button>
  </div>

  <aside class="panel" id="panel" aria-hidden="true" aria-labelledby="panel-title">
    <div class="panel-grip" aria-hidden="true"></div>
    <div class="panel-head">
      <span class="panel-eyebrow" id="panel-eyebrow"></span>
      <button class="panel-close" data-close-panel aria-label="Close details">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 6 12 12M18 6 6 18"/></svg>
      </button>
    </div>
    <div class="panel-scroll">
      <div class="panel-status" id="panel-status"></div>
      <h2 class="panel-title" id="panel-title"></h2>
      <p class="panel-summary" id="panel-summary"></p>
      <div class="panel-body" id="panel-body"></div>
      <a class="panel-doc" id="panel-doc" href="#" hidden>
        <span>Read the full milestone</span>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12h14M13 6l6 6-6 6"/></svg>
      </a>
      <p class="panel-foot">IRIX 7 · living roadmap · statuses {esc(data["meta"]["updated"])}</p>
    </div>
  </aside>

  <section class="intro" id="intro" aria-labelledby="intro-title">
    <div class="intro-main">
      <p class="eyebrow"><span class="pulse"></span> A living roadmap · IP24</p>
      <h1 id="intro-title">The Tree of<br><em>IRIX 7</em></h1>
      <p class="intro-lead">{esc(data["meta"]["intro"][0])}</p>
      <p class="intro-copy">{esc(data["meta"]["intro"][1])}</p>
      <p class="intro-copy muted">{esc(data["meta"]["intro"][2])}</p>
      <div class="intro-actions">
        <button class="enter-btn" data-enter>
          <span>Enter the tree</span>
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12h14M13 6l6 6-6 6"/></svg>
        </button>
        <span class="intro-hint">Drag to explore · scroll to zoom</span>
      </div>
      <div class="intro-stats">
        <div><strong>{total_milestones}</strong><span>milestones</span></div>
        <div><strong>{releases}</strong><span>releases</span></div>
        <div><strong>{counts["done"]}</strong><span>done</span></div>
        <div><strong>{counts["active"]}</strong><span>growing</span></div>
      </div>
    </div>
    <div class="intro-aside">
      <p class="aside-label">Now growing</p>
      <button class="aside-card" data-node="{now["id"]}">
        <span class="aside-code">{esc(now["code"])}</span>
        <span class="aside-title">{esc(now["fullTitle"])}</span>
        <span class="aside-copy">{esc(now["summary"])}</span>
        <span class="aside-cta">See what is left <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12h14M13 6l6 6-6 6"/></svg></span>
      </button>
      <p class="aside-note">{esc(data["meta"]["note"])}</p>
    </div>
  </section>

  <div class="search-layer" id="search-layer" hidden>
    <div class="search-box" role="dialog" aria-modal="true" aria-label="Find a node">
      <div class="search-field">
        <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="6.5"/><path d="m16 16 4.5 4.5"/></svg>
        <input id="search-input" type="search" placeholder="Search milestones, features, releases…" aria-label="Search the tree" autocomplete="off" spellcheck="false">
        <kbd>esc</kbd>
      </div>
      <ul class="search-results" id="search-results" role="listbox" aria-label="Results"></ul>
      <p class="search-hint"><span>↑↓ to move</span><span>↵ to open</span><span>esc to close</span></p>
    </div>
  </div>

  <dialog class="about" id="about" aria-labelledby="about-title">
    <div class="about-head">
      <p class="eyebrow">Colophon</p>
      <button class="panel-close" data-close-about aria-label="Close">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 6 12 12M18 6 6 18"/></svg>
      </button>
    </div>
    <h2 id="about-title">How to read the tree</h2>
    <p class="about-lead">This is not a Gantt chart. It is a dependency map: a branch can only grow when the wood beneath it is strong enough.</p>
    <div class="about-grid">
      <div>
        <h3>Branches</h3>
        <p>Solid boughs are the plan's structure. Dotted arcs are cross-links — compatibility needs storage, X11 needs a display, every branch meets at v1.0.</p>
      </div>
      <div>
        <h3>Status</h3>
        <p>Colour is the state of the work, not its size. Hollow, dashed rings are still exploratory. The compost holds what v1.0 deliberately leaves out.</p>
      </div>
      <div>
        <h3>The roots</h3>
        <p>M0–M2 are the foundations: toolchain, ROM and kernel. Without them nothing above can be true, which is why they are underground.</p>
      </div>
      <div>
        <h3>The source</h3>
        <p>Everything here is generated from <a href="roadmap.json"><code>roadmap.json</code></a> by <a href="https://github.com/irix7/os/blob/main/tree/build.py">build.py</a>. It follows <a href="../plan/milestones.html">the milestone plan</a>; milestone statuses sync from <a href="{esc(data["meta"]["project"])}">IRIX 7 — Roadmap</a> on a schedule, statuses updated {esc(data["meta"]["updated"])}.</p>
      </div>
    </div>
    <div class="about-statuses" id="about-statuses"></div>
    <p class="about-foot">Set in Fraunces, Space Grotesk and IBM Plex Mono. Built with Python and SVG, no tracking, no dependencies. IRIX 7 is GPL-3.0. <a href="{esc(data["meta"]["repo"])}">Source</a> · <a href="{esc(data["meta"]["docs"])}">Documentation</a></p>
  </dialog>

  <div class="sr-only" id="node-list" tabindex="-1"><ul>{''.join(sr_nodes)}</ul></div>
  <div class="grain" aria-hidden="true"></div>
</div>
<script type="application/json" id="roadmap-data">{json_blob}</script>
<script src="assets/tree.js"></script>
</body>
</html>"""


def build(out: Path) -> None:
    data = json.loads(DATA.read_text(encoding="utf-8"))
    roadmap = Roadmap(data)
    roadmap.layout()
    page = build_page(roadmap)
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    (out / "index.html").write_text(page, encoding="utf-8")
    shutil.copytree(ASSETS, out / "assets")
    shutil.copy2(DATA, out / "roadmap.json")
    size = (out / "index.html").stat().st_size
    print(
        f"tree: {len(roadmap.nodes)} nodes, {len(roadmap.milestones())} milestones, "
        f"{len([n for n in roadmap.nodes if n['kind'] == 'feature'])} features → {out} "
        f"({size // 1024} KiB HTML)"
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=ROOT / "dist", help="output directory")
    parser.add_argument("--check", action="store_true", help="validate the roadmap only")
    args = parser.parse_args()
    data = json.loads(DATA.read_text(encoding="utf-8"))
    roadmap = Roadmap(data)
    roadmap.layout()
    if args.check:
        print(
            f"roadmap ok: {len(roadmap.nodes)} nodes, "
            f"{len(roadmap.milestones())} milestones, "
            f"{len([n for n in roadmap.nodes if n['kind'] == 'feature'])} features"
        )
    else:
        build(args.out)


if __name__ == "__main__":
    main()
