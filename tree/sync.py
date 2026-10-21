#!/usr/bin/env python3
"""Sync The Tree of IRIX 7 with the roadmap's actual sources of truth.

Order of evidence:

1. release tags in git (strongest: a tagged release happened);
2. the IRIX 7 — Roadmap Project (Phase + Workflow fields), when a
   PROJECT_TOKEN with project scope is configured;
3. closed epic issues (a safe fallback when the Project token is absent).

The sync is monotonic. It can promote a milestone from planned/exploring to
active, or to done; it never silently moves work backwards. If the Project
board appears to be behind the tree, that is reported as a warning instead of
being applied.

This script is deliberately dependency-free and import-safe so it can be
unit-tested and run from GitHub Actions.
"""

from __future__ import annotations

import argparse
import calendar
import datetime as dt
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any
from zoneinfo import ZoneInfo

ROOT = Path(__file__).resolve().parent
DATA = ROOT / "roadmap.json"
DEFAULT_PROJECT_URL = "https://github.com/orgs/irix7/projects/2"
DEFAULT_REPOSITORY = "irix7/os"

# A release tag is the strongest possible evidence that a milestone shipped.
TAG_TO_RELEASE = {
    "v0.1": ("v0.1", "m3"),
    "v0.2": ("v0.2", "m5"),
    "v0.3": ("v0.3", "m6"),
    "v0.4": ("v0.4", "m7"),
    "v0.5": ("v0.5", "m8"),
    "v0.6": ("v0.6", "m9"),
    "v1.0": ("v1.0", "m10"),
}

# planned and exploring are peer states: both mean "not started". Only promote.
RANK = {"planned": 0, "exploring": 0, "active": 1, "done": 2}
ACTIVE_WORKFLOWS = {"In Progress", "In Review", "Blocked"}
QUIET_WORKFLOWS = {"Backlog", "Ready"}

PHASE_RE = re.compile(r"^M(\d+)\b")
URL_RE = re.compile(r"/(?:orgs|users)/([^/]+)/projects/(\d+)")


def today() -> str:
    try:
        now = dt.datetime.now(dt.timezone.utc).astimezone(ZoneInfo("Australia/Sydney"))
    except Exception:
        now = dt.datetime.now(dt.timezone.utc)
    return f"{now.day} {calendar.month_name[now.month]} {now.year}"


def rank(status: str) -> int:
    return RANK.get(status, 0)


def can_promote(current: str, proposed: str) -> bool:
    return rank(proposed) > rank(current)


def parse_project_url(url: str) -> tuple[str, int] | None:
    match = URL_RE.search(url or "")
    if not match:
        return None
    return match.group(1), int(match.group(2))


def phase_to_milestone(phase: str | None) -> str | None:
    if not phase:
        return None
    match = PHASE_RE.match(phase.strip())
    if not match:
        return None
    return f"m{int(match.group(1))}"


def normalise_item(raw: dict[str, Any]) -> dict[str, Any]:
    """Reduce a ProjectV2 item to the fields the sync actually needs."""
    content = raw.get("content") or {}
    fields: dict[str, str] = {}
    for value in (raw.get("fieldValues") or {}).get("nodes", []):
        name = (value.get("field") or {}).get("name")
        if name and value.get("name") is not None:
            fields[name] = value["name"]
    labels = [
        node.get("name", "")
        for node in ((content.get("labels") or {}).get("nodes") or [])
    ]
    return {
        "number": content.get("number"),
        "title": content.get("title") or "",
        "state": (content.get("state") or "").upper(),
        "url": content.get("url") or "",
        "labels": labels,
        "phase": fields.get("Phase"),
        "workflow": fields.get("Workflow"),
        "is_epic": "type/epic" in labels or bool(PHASE_RE.match(content.get("title") or "")),
    }


def aggregate_project_items(
    items: list[dict[str, Any]],
) -> tuple[dict[str, dict[str, str]], list[str]]:
    """Return proposals keyed by milestone id, plus warnings."""
    grouped: dict[str, list[dict[str, Any]]] = {}
    for item in items:
        milestone = phase_to_milestone(item.get("phase"))
        if milestone:
            grouped.setdefault(milestone, []).append(item)

    proposals: dict[str, dict[str, str]] = {}
    warnings: list[str] = []

    for milestone, group in sorted(grouped.items()):
        workflows = [i.get("workflow") for i in group if i.get("workflow")]
        epics = [i for i in group if i.get("is_epic")]
        closed_epics = [i for i in epics if i.get("state") == "CLOSED"]
        open_epics = [i for i in epics if i.get("state") == "OPEN"]

        for epic in closed_epics:
            if epic.get("workflow") != "Done":
                warnings.append(
                    f"Project #{epic.get('number')} ({epic.get('title')}) is closed but "
                    f"Workflow={epic.get('workflow') or 'unset'}; the board is stale."
                )
        for epic in open_epics:
            if epic.get("workflow") == "Done":
                warnings.append(
                    f"Project #{epic.get('number')} ({epic.get('title')}) is open but "
                    "Workflow=Done; the board is inconsistent."
                )

        if closed_epics:
            proposals[milestone] = {
                "status": "done",
                "reason": "the milestone epic is closed",
            }
            continue

        if open_epics and any(i.get("workflow") == "Done" for i in open_epics):
            # Do not mark done while the epic itself is open.
            continue

        if workflows and all(w == "Done" for w in workflows):
            proposals[milestone] = {
                "status": "done",
                "reason": "every Project item for this phase is Done",
            }
        elif any(w in ACTIVE_WORKFLOWS for w in workflows):
            proposals[milestone] = {
                "status": "active",
                "reason": "the Project has work in progress, review or blocked",
            }
        elif workflows and all(w in QUIET_WORKFLOWS for w in workflows):
            proposals[milestone] = {
                "status": "planned",
                "reason": "every Project item is still in Backlog or Ready",
            }
        elif workflows:
            warnings.append(
                f"{milestone}: Project items are mixed ({', '.join(sorted(set(workflows)))}); "
                "no status change proposed."
            )

    return proposals, warnings


def fetch_graphql(token: str, query: str, variables: dict[str, Any]) -> dict[str, Any]:
    request = urllib.request.Request(
        "https://api.github.com/graphql",
        data=json.dumps({"query": query, "variables": variables}).encode(),
        headers={
            "Authorization": f"bearer {token}",
            "Accept": "application/vnd.github+json",
            "Content-Type": "application/json",
            "User-Agent": "irix7-roadmap-sync",
        },
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.loads(response.read().decode())


PROJECT_QUERY = """
query($owner: String!, $number: Int!, $cursor: String) {
  organization(login: $owner) {
    projectV2(number: $number) {
      title
      url
      items(first: 100, after: $cursor) {
        pageInfo { hasNextPage endCursor }
        nodes {
          content {
            __typename
            ... on Issue {
              number title state url
              labels(first: 20) { nodes { name } }
            }
            ... on PullRequest { number title state url }
            ... on DraftIssue { title }
          }
          fieldValues(first: 30) {
            nodes {
              __typename
              ... on ProjectV2ItemFieldSingleSelectValue {
                name
                field { ... on ProjectV2SingleSelectField { name } }
              }
            }
          }
        }
      }
    }
  }
}
"""


def fetch_project(
    token: str, project_url: str
) -> tuple[list[dict[str, Any]] | None, str | None, list[str]]:
    parsed = parse_project_url(project_url)
    if not parsed:
        return None, None, [f"Could not parse PROJECT_URL={project_url!r}."]
    owner, number = parsed
    items: list[dict[str, Any]] = []
    warnings: list[str] = []
    cursor: str | None = None
    title = ""
    url = project_url
    try:
        while True:
            payload = fetch_graphql(
                token,
                PROJECT_QUERY,
                {"owner": owner, "number": number, "cursor": cursor},
            )
            if payload.get("errors"):
                return None, None, [
                    "Project GraphQL error: "
                    + "; ".join(err.get("message", "unknown") for err in payload["errors"])
                ]
            project = ((payload.get("data") or {}).get("organization") or {}).get("projectV2")
            if not project:
                return None, None, [
                    f"Project {owner}/{number} was not found or is not visible to PROJECT_TOKEN."
                ]
            title = project.get("title") or title
            url = project.get("url") or url
            page = project.get("items") or {}
            for raw in page.get("nodes") or []:
                items.append(normalise_item(raw))
            page_info = page.get("pageInfo") or {}
            if not page_info.get("hasNextPage"):
                break
            cursor = page_info.get("endCursor")
    except (urllib.error.URLError, TimeoutError, ValueError) as error:
        return None, None, [f"Could not read the Project: {error}"]
    return items, url, warnings


def fetch_epic_issues(
    token: str, repository: str
) -> tuple[list[dict[str, Any]] | None, list[str]]:
    url = (
        f"https://api.github.com/repos/{repository}/issues"
        "?labels=type/epic&state=all&per_page=100"
    )
    request = urllib.request.Request(
        url,
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "irix7-roadmap-sync",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            issues = json.loads(response.read().decode())
    except (urllib.error.URLError, TimeoutError, ValueError) as error:
        return None, [f"Could not read epic issues: {error}"]
    items = []
    for issue in issues:
        if issue.get("pull_request"):
            continue
        labels = [label.get("name", "") for label in issue.get("labels") or []]
        items.append(
            {
                "number": issue.get("number"),
                "title": issue.get("title") or "",
                "state": (issue.get("state") or "").upper(),
                "url": issue.get("html_url") or "",
                "labels": labels,
                "phase": None,
                "workflow": None,
                "is_epic": "type/epic" in labels,
            }
        )
    return items, []


def proposals_from_epics(
    items: list[dict[str, Any]],
) -> tuple[dict[str, dict[str, str]], list[str]]:
    proposals: dict[str, dict[str, str]] = {}
    for item in items:
        if not item.get("is_epic"):
            continue
        match = PHASE_RE.match(item.get("title") or "")
        if not match:
            continue
        milestone = f"m{int(match.group(1))}"
        if item.get("state") == "CLOSED":
            proposals[milestone] = {
                "status": "done",
                "reason": "the milestone epic is closed",
            }
    return proposals, []


def git_tags() -> set[str]:
    try:
        result = subprocess.run(
            ["git", "-C", str(ROOT.parent), "tag", "--list"],
            capture_output=True,
            text=True,
            check=True,
            timeout=15,
        )
        return {line.strip() for line in result.stdout.splitlines() if line.strip()}
    except (OSError, subprocess.SubprocessError):
        return set()


def tag_proposals(tags: set[str]) -> tuple[dict[str, dict[str, str]], list[str]]:
    proposals: dict[str, dict[str, str]] = {}
    warnings: list[str] = []
    for tag, (release, milestone) in TAG_TO_RELEASE.items():
        if tag in tags:
            reason = f"release tag {tag} exists"
            proposals[release] = {"status": "done", "reason": reason}
            proposals[milestone] = {"status": "done", "reason": reason}
    return proposals, warnings


def merge_proposals(
    target: dict[str, dict[str, str]], source: dict[str, dict[str, str]]
) -> None:
    """Keep the strongest proposal when several sources disagree."""
    for node_id, proposal in source.items():
        current = target.get(node_id)
        if not current or rank(proposal["status"]) > rank(current["status"]):
            target[node_id] = proposal


def apply_proposals(
    data: dict[str, Any], proposals: dict[str, dict[str, str]], warnings: list[str]
) -> list[str]:
    """Apply only forward transitions; return human-readable changes."""
    changes: list[str] = []
    nodes = data.get("nodes", [])
    by_id = {node["id"]: node for node in nodes}
    milestones_to_complete: set[str] = set()

    for node in nodes:
        proposal = proposals.get(node["id"])
        if not proposal:
            continue
        current = node.get("status", "planned")
        proposed = proposal["status"]
        if can_promote(current, proposed):
            node["status"] = proposed
            changes.append(
                f"{node.get('code') or node['id']} {node['title']}: {current} → {proposed} "
                f"({proposal['reason']})"
            )
            if node.get("kind") == "milestone" and proposed == "done":
                milestones_to_complete.add(node["id"])
        elif rank(proposed) < rank(current):
            warnings.append(
                f"{node.get('code') or node['id']}: Project suggests {proposed}, "
                f"but the tree is {current}; keeping {current} (sync is monotonic)."
            )

    # A completed milestone completes its feature leaves. A feature cannot be
    # unfinished if the milestone that contains it is done.
    for node in nodes:
        if node.get("kind") != "feature":
            continue
        if node.get("parent") in milestones_to_complete and node.get("status") != "done":
            changes.append(
                f"{node.get('code') or node['id']} {node['title']}: "
                f"{node.get('status')} → done (parent milestone is done)"
            )
            node["status"] = "done"

    return changes


def report_markdown(
    *,
    changes: list[str],
    warnings: list[str],
    source: str,
    statuses: dict[str, str],
) -> str:
    lines = ["## Roadmap sync", "", f"Source: {source}", ""]
    if changes:
        lines += ["### Changes", ""] + [f"- {change}" for change in changes] + [""]
    else:
        lines += ["No status changes.", ""]
    if warnings:
        lines += ["### Warnings", ""] + [f"- {warning}" for warning in warnings] + [""]
    lines += ["### Milestone status", ""]
    for milestone, status in statuses.items():
        lines.append(f"- {milestone}: **{status}**")
    return "\n".join(lines) + "\n"


def write_output(changed: bool) -> None:
    path = os.environ.get("GITHUB_OUTPUT")
    if path:
        with open(path, "a", encoding="utf-8") as handle:
            handle.write(f"changed={'true' if changed else 'false'}\n")


def write_summary(markdown: str) -> None:
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if path:
        with open(path, "a", encoding="utf-8") as handle:
            handle.write(markdown + "\n")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dry-run", action="store_true", help="do not write roadmap.json")
    parser.add_argument("--project-url", default=os.environ.get("PROJECT_URL") or DEFAULT_PROJECT_URL)
    parser.add_argument("--repository", default=os.environ.get("GITHUB_REPOSITORY") or DEFAULT_REPOSITORY)
    parser.add_argument("--report", type=Path, help="write a Markdown report to this path")
    args = parser.parse_args()

    data = json.loads(DATA.read_text(encoding="utf-8"))
    project_token = os.environ.get("PROJECT_TOKEN") or ""
    github_token = os.environ.get("GITHUB_TOKEN") or ""

    proposals: dict[str, dict[str, str]] = {}
    warnings: list[str] = []
    sources: list[str] = []

    tags = git_tags()
    tag_props, tag_warnings = tag_proposals(tags)
    merge_proposals(proposals, tag_props)
    warnings += tag_warnings
    if tag_props:
        sources.append("release tags")

    project_items: list[dict[str, Any]] | None = None
    project_url = args.project_url
    if project_token:
        project_items, project_url, project_warnings = fetch_project(
            project_token, args.project_url
        )
        warnings += project_warnings
        if project_items is not None:
            project_props, project_warnings = aggregate_project_items(project_items)
            merge_proposals(proposals, project_props)
            warnings += project_warnings
            sources.append(f"Project {project_url}")
    else:
        warnings.append(
            "PROJECT_TOKEN is not set; skipping Project #2 and using closed epics only."
        )

    if (project_items is None or not project_items) and github_token:
        epic_items, epic_warnings = fetch_epic_issues(github_token, args.repository)
        warnings += epic_warnings
        if epic_items is not None:
            epic_props, epic_warnings = proposals_from_epics(epic_items)
            merge_proposals(proposals, epic_props)
            warnings += epic_warnings
            sources.append("closed epic issues")
    if not project_token and not github_token:
        warnings.append("No GitHub token is available; only local release tags were checked.")

    changes = apply_proposals(data, proposals, warnings)
    if changes:
        data.setdefault("meta", {})["updated"] = today()
        if not args.dry_run:
            DATA.write_text(
                json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
            )

    statuses = {
        f"{node.get('code') or node['id']} {node['title']}": node["status"]
        for node in data.get("nodes", [])
        if node.get("kind") == "milestone"
    }
    source = " + ".join(sources) if sources else "none configured"
    markdown = report_markdown(
        changes=changes,
        warnings=warnings,
        source=source,
        statuses=statuses,
    )
    print(markdown)
    write_summary(markdown)
    write_output(bool(changes and not args.dry_run))
    if args.report:
        args.report.write_text(markdown, encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
