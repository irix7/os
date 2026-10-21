"""Unit tests for the roadmap sync. No network access required."""

from __future__ import annotations

import unittest

import sync


def project_item(
    phase: str,
    workflow: str,
    *,
    state: str = "OPEN",
    title: str | None = None,
    labels: list[str] | None = None,
    number: int = 1,
) -> dict:
    labels = labels or []
    return {
        "number": number,
        "title": title or f"{phase} — an epic",
        "state": state,
        "url": "",
        "labels": labels,
        "phase": phase,
        "workflow": workflow,
        "is_epic": "type/epic" in labels or bool(sync.PHASE_RE.match(title or "")),
    }


class ProjectUrlTests(unittest.TestCase):
    def test_org_project(self):
        self.assertEqual(
            sync.parse_project_url("https://github.com/orgs/irix7/projects/2"),
            ("irix7", 2),
        )

    def test_user_project(self):
        self.assertEqual(
            sync.parse_project_url("https://github.com/users/matt/projects/7"),
            ("matt", 7),
        )

    def test_bad_url(self):
        self.assertIsNone(sync.parse_project_url("https://github.com/irix7/os"))


class PhaseTests(unittest.TestCase):
    def test_phases(self):
        self.assertEqual(sync.phase_to_milestone("M0 Toolchain"), "m0")
        self.assertEqual(sync.phase_to_milestone("M10 Release"), "m10")
        self.assertEqual(sync.phase_to_milestone("M3 Network+MCP"), "m3")
        self.assertIsNone(sync.phase_to_milestone("Cross-cutting"))
        self.assertIsNone(sync.phase_to_milestone(None))


class MonotonicTests(unittest.TestCase):
    def test_promotions(self):
        self.assertTrue(sync.can_promote("planned", "active"))
        self.assertTrue(sync.can_promote("exploring", "active"))
        self.assertTrue(sync.can_promote("active", "done"))
        self.assertTrue(sync.can_promote("planned", "done"))

    def test_never_backwards(self):
        self.assertFalse(sync.can_promote("done", "active"))
        self.assertFalse(sync.can_promote("active", "planned"))
        self.assertFalse(sync.can_promote("exploring", "planned"))


class AggregateTests(unittest.TestCase):
    def test_all_backlog_is_planned(self):
        proposals, warnings = sync.aggregate_project_items(
            [project_item("M4 Storage", "Backlog"), project_item("M4 Storage", "Ready")]
        )
        self.assertEqual(proposals["m4"]["status"], "planned")
        self.assertEqual(warnings, [])

    def test_active_work(self):
        proposals, _ = sync.aggregate_project_items(
            [project_item("M3 Network+MCP", "In Progress")]
        )
        self.assertEqual(proposals["m3"]["status"], "active")

    def test_all_done(self):
        proposals, _ = sync.aggregate_project_items(
            [project_item("M5 Compat", "Done"), project_item("M5 Compat", "Done")]
        )
        self.assertEqual(proposals["m5"]["status"], "done")

    def test_closed_epic_wins_and_warns(self):
        proposals, warnings = sync.aggregate_project_items(
            [
                project_item(
                    "M0 Toolchain",
                    "Ready",
                    state="CLOSED",
                    title="M0 — Toolchain unblocked",
                    labels=["type/epic"],
                )
            ]
        )
        self.assertEqual(proposals["m0"]["status"], "done")
        self.assertTrue(any("board is stale" in warning for warning in warnings))

    def test_open_epic_done_is_not_done(self):
        proposals, warnings = sync.aggregate_project_items(
            [
                project_item(
                    "M1 ROM",
                    "Done",
                    state="OPEN",
                    title="M1 — ROM netboot",
                    labels=["type/epic"],
                )
            ]
        )
        self.assertNotIn("m1", proposals)
        self.assertTrue(any("inconsistent" in warning for warning in warnings))

    def test_mixed_workflow_is_ambiguous(self):
        proposals, warnings = sync.aggregate_project_items(
            [project_item("M6 Display", "Done"), project_item("M6 Display", "Backlog")]
        )
        self.assertNotIn("m6", proposals)
        self.assertTrue(any("mixed" in warning for warning in warnings))


class ApplyTests(unittest.TestCase):
    def data(self, milestone_status: str = "planned") -> dict:
        return {
            "nodes": [
                {
                    "id": "m3",
                    "kind": "milestone",
                    "code": "M3",
                    "title": "Control plane",
                    "status": milestone_status,
                },
                {
                    "id": "m3-network",
                    "kind": "feature",
                    "parent": "m3",
                    "title": "Network cell",
                    "status": "planned",
                },
            ]
        }

    def test_done_completes_features(self):
        data = self.data()
        warnings = []
        changes = sync.apply_proposals(
            data, {"m3": {"status": "done", "reason": "test"}}, warnings
        )
        self.assertEqual(data["nodes"][0]["status"], "done")
        self.assertEqual(data["nodes"][1]["status"], "done")
        self.assertTrue(any("parent milestone is done" in change for change in changes))
        self.assertEqual(warnings, [])

    def test_backwards_is_ignored_and_warned(self):
        data = self.data("active")
        warnings = []
        changes = sync.apply_proposals(
            data, {"m3": {"status": "planned", "reason": "test"}}, warnings
        )
        self.assertEqual(data["nodes"][0]["status"], "active")
        self.assertEqual(changes, [])
        self.assertTrue(any("keeping active" in warning for warning in warnings))

    def test_exploring_can_start(self):
        data = self.data("exploring")
        warnings = []
        sync.apply_proposals(data, {"m3": {"status": "active", "reason": "test"}}, warnings)
        self.assertEqual(data["nodes"][0]["status"], "active")


class TagTests(unittest.TestCase):
    def test_release_tag_promotes_release_and_milestone(self):
        proposals, warnings = sync.tag_proposals({"v0.4"})
        self.assertEqual(proposals["v0.4"]["status"], "done")
        self.assertEqual(proposals["m7"]["status"], "done")
        self.assertEqual(warnings, [])

    def test_unknown_tag_ignored(self):
        proposals, _ = sync.tag_proposals({"v9.9"})
        self.assertEqual(proposals, {})

    def test_strongest_proposal_wins(self):
        proposals = {}
        sync.merge_proposals(proposals, {"m7": {"status": "active", "reason": "board"}})
        sync.merge_proposals(proposals, {"m7": {"status": "done", "reason": "tag"}})
        self.assertEqual(proposals["m7"]["status"], "done")
        sync.merge_proposals(proposals, {"m7": {"status": "planned", "reason": "stale"}})
        self.assertEqual(proposals["m7"]["status"], "done")


if __name__ == "__main__":
    unittest.main()
