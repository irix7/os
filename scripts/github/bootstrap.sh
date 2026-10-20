#!/usr/bin/env bash
# Bootstrap the IRIX 7 GitHub organisation/repository/project.
#
# Idempotent: safe to re-run. It never deletes labels, issues, milestones or
# projects. It creates/updates labels, milestones, the Project, fields, views
# and the M0-M10 epics.
#
# Requirements:
#   gh auth refresh -s project,repo,workflow
#
# Usage:
#   scripts/github/bootstrap.sh
set -uo pipefail

ORG="${ORG:-irix7}"
REPO="${REPO:-$ORG/os}"
PROJECT_TITLE="${PROJECT_TITLE:-IRIX 7 — Roadmap}"

say()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

command -v gh >/dev/null || die "gh is not installed"
gh auth status >/dev/null 2>&1 || die "gh is not authenticated; run: gh auth login"

say "Organisation: $ORG   Repository: $REPO"

# ---------------------------------------------------------------------------
# Repository features
# ---------------------------------------------------------------------------
say "Enabling Discussions and Wiki"
gh api -X PATCH "repos/$REPO" -F has_discussions=true -F has_wiki=true >/dev/null \
  || warn "could not enable Discussions/Wiki (admin permission?)"

# ---------------------------------------------------------------------------
# Labels
# ---------------------------------------------------------------------------
say "Creating/updating labels"
while IFS='|' read -r name colour description; do
  [ -z "${name// }" ] && continue
  if gh label list -R "$REPO" --limit 200 --json name --jq '.[].name' </dev/null | grep -qx "$name"; then
    gh label edit "$name" -R "$REPO" --color "$colour" --description "$description" </dev/null >/dev/null
  else
    gh label create "$name" -R "$REPO" --color "$colour" --description "$description" </dev/null >/dev/null
  fi
done <<'LABELS'
area/kernel|1d76db|Traps, TLB, scheduler, VM, IPC, capabilities
area/boot|0e8a16|ROM, boot protocol, boot bundle, image loading
area/compat|5319e7|IRIX syscall ABI, loader, libc, libraries
area/drivers|0e8a16|SCSI, Seeq, HAL2, Newport, Vino, serial, RTC
area/display|d93f0b|Display server, desktop, X11, graphics
area/storage|bfd4f2|EROFS, XFS, I7FS, block cache, Git sync
area/network|006b75|smoltcp, /net, HTTP Range, TLS, MCP
area/toolchain|fbca04|rustc, GCC, LLVM, binutils, build-std
area/infra|c2e0c6|CI, Actions, runners, releases, R2
area/docs|0075ca|Documentation, specs, ADRs, website
area/security|b60205|Threat model, MCP security, supply chain
area/legal|e99695|Licence, provenance, trademark
area/clean-room|f9d0c4|Harness, similarity screen, provenance
area/hardware|f0e68c|Real Indy, rig, SCSI, serial, PROM, GIO
type/epic|3e4b9e|A milestone-sized body of work
type/feature|a2eeef|New capability
type/bug|d73a4a|Something is broken
type/research|d4c5f9|Question to answer, not code to write
type/adr|fef2c0|Architecture decision record
type/hardware|f0e68c|Real-Indy work
type/task|ededed|Small unit of work
type/debt|e6ccb3|Refactor or cleanup
priority/P0|b60205|Blocks the milestone or release
priority/P1|d93f0b|Must be in the milestone
priority/P2|fbca04|Should be in the milestone
priority/P3|0e8a16|Nice to have
size/XS|c2e0c6|Less than 1 hour
size/S|c2e0c6|1-4 hours
size/M|c2e0c6|1-3 days
size/L|c2e0c6|1-2 weeks
size/XL|c2e0c6|More than 2 weeks; split it
risk/low|0e8a16|Local, reversible
risk/medium|fbca04|Crosses a subsystem or the toolchain
risk/high|b60205|Clean-room, security, data loss, hardware damage
hardware/iris|1d76db|Reproducible on the emulator
hardware/indy|d93f0b|Requires a real Indy
hardware/both|5319e7|Must be validated on both
hardware/none|ededed|Tooling/docs only
status/blocked|b60205|Waiting on a dependency
status/needs-adr|fef2c0|Decision required before code
status/needs-design|d4c5f9|Interface or UX design required
status/needs-hardware|f0e68c|Blocked on a part or machine
status/clean-room-review|f9d0c4|Needs the similarity screen
status/security-review|b60205|Needs a security review
status/needs-triage|ededed|Not yet triaged
stale|ededed|No activity for 60 days
dependencies|0366d6|Dependency update
skip-changelog|ededed|Exclude from release notes
release/major|b60205|Major version change
release/minor|d93f0b|Minor version change
release/patch|0e8a16|Patch version change
LABELS

# ---------------------------------------------------------------------------
# Milestones
# ---------------------------------------------------------------------------
say "Creating milestones"
create_milestone() {
  local title="$1" description="$2"
  if gh api "repos/$REPO/milestones" --paginate --jq '.[].title' | grep -qx "$title"; then
    return
  fi
  gh api "repos/$REPO/milestones" \
    -f title="$title" \
    -f state=open \
    -f description="$description" >/dev/null
}
create_milestone "v0.1" "Serial, network, MCP, shell, reset/watchdog"
create_milestone "v0.2" "First IRIX o32 binary, minimal libc"
create_milestone "v0.3" "Newport display, native desktop shell"
create_milestone "v0.4" "X11 lease, xterm/twm, Xlib subset"
create_milestone "v0.5" "XFS read-only, IRIX disk access"
create_milestone "v0.6" "Native cells, capability ABI, native libc"
create_milestone "v1.0" "Real-Indy daily driver, updater, docs, signed release"

# ---------------------------------------------------------------------------
# Project
# ---------------------------------------------------------------------------
say "Finding or creating the project: $PROJECT_TITLE"
PROJECT_NUMBER=$(gh project list --owner "$ORG" --limit 50 --format json \
  --jq ".projects[] | select(.title == \"$PROJECT_TITLE\") | .number" | head -1)

if [ -z "$PROJECT_NUMBER" ]; then
  PROJECT_JSON=$(gh project create --owner "$ORG" --title "$PROJECT_TITLE" --format json)
  PROJECT_NUMBER=$(printf '%s' "$PROJECT_JSON" | jq -r '.number')
fi
[ -n "$PROJECT_NUMBER" ] || die "could not determine the project number"
say "Project number: $PROJECT_NUMBER"

gh project link "$PROJECT_NUMBER" --owner "$ORG" --repo "$REPO" >/dev/null 2>&1 \
  || gh project link "$PROJECT_NUMBER" --owner "$ORG" --repo "$(basename "$REPO")" >/dev/null 2>&1 \
  || warn "could not link the project to $REPO"

PROJECT_ID=$(gh api graphql \
  -f query='query($org:String!,$num:Int!){organization(login:$org){projectV2(number:$num){id}}}' \
  -f org="$ORG" -F num="$PROJECT_NUMBER" --jq '.data.organization.projectV2.id' 2>/dev/null)

# ---------------------------------------------------------------------------
# Fields
# ---------------------------------------------------------------------------
say "Creating project fields"
field_exists() {
  gh project field-list "$PROJECT_NUMBER" --owner "$ORG" --format json \
    --jq '.fields[].name' 2>/dev/null | grep -qx "$1"
}
ensure_select() {
  local name="$1" options="$2"
  if field_exists "$name"; then
    say "  field exists: $name"
    return
  fi
  gh project field-create "$PROJECT_NUMBER" --owner "$ORG" \
    --name "$name" --data-type SINGLE_SELECT \
    --single-select-options "$options" >/dev/null \
    || warn "could not create field: $name"
}
ensure_number() {
  local name="$1"
  if field_exists "$name"; then return; fi
  gh project field-create "$PROJECT_NUMBER" --owner "$ORG" \
    --name "$name" --data-type NUMBER >/dev/null \
    || warn "could not create field: $name"
}
ensure_date() {
  local name="$1"
  if field_exists "$name"; then return; fi
  gh project field-create "$PROJECT_NUMBER" --owner "$ORG" \
    --name "$name" --data-type DATE >/dev/null \
    || warn "could not create field: $name"
}

ensure_select "Workflow" "Backlog,Ready,In Progress,Blocked,In Review,Done"
ensure_select "Phase" "M0 Toolchain,M1 ROM,M2 Kernel,M3 Network+MCP,M4 Storage,M5 Compat,M6 Display,M7 X11,M8 XFS,M9 Native,M10 Release,Cross-cutting"
ensure_select "Area" "Kernel,Boot,Compat,Drivers,Display,Storage,Network,Toolchain,Infra,Docs,Security,Legal,Clean-room,Hardware"
ensure_select "Priority" "P0,P1,P2,P3"
ensure_select "Size" "XS,S,M,L,XL"
ensure_select "Risk" "Low,Medium,High"
ensure_select "Hardware" "iris,Indy,Both,None"
ensure_select "Target" "Backlog,v0.1,v0.2,v0.3,v0.4,v0.5,v0.6,v1.0"
ensure_select "Clean-room" "Not needed,Review,Approved"
ensure_number "Estimate"
ensure_date "Start"
ensure_date "Target date"

# ---------------------------------------------------------------------------
# Views (GraphQL; gh project has no view-create)
# ---------------------------------------------------------------------------
say "Creating project views"
if [ -z "${PROJECT_ID:-}" ]; then
  warn "no project ID; skipping views"
else
  EXISTING_VIEWS=$(gh api graphql \
    -f query='query($id:ID!){node(id:$id){... on ProjectV2 {views(first:50){nodes{name}}}}}' \
    -f id="$PROJECT_ID" --jq '.data.node.views.nodes[].name' 2>/dev/null || true)
  create_view() {
    local name="$1" layout="$2"
    if printf '%s\n' "$EXISTING_VIEWS" | grep -qx "$name"; then
      say "  view exists: $name"
      return
    fi
    gh api graphql \
      -f query='mutation($projectId:ID!,$name:String!,$layout:ProjectV2ViewLayout!){createProjectV2View(input:{projectId:$projectId,name:$name,layout:$layout}){projectV2View{id name}}}' \
      -f projectId="$PROJECT_ID" -f name="$name" -f layout="$layout" >/dev/null \
      && say "  view created: $name" \
      || warn "could not create view: $name (create it in the UI)"
  }
  create_view "Roadmap" "ROADMAP_LAYOUT"
  create_view "Board" "BOARD_LAYOUT"
  create_view "Table" "TABLE_LAYOUT"
  create_view "Blocked" "TABLE_LAYOUT"
  create_view "My work" "TABLE_LAYOUT"
  create_view "By area" "BOARD_LAYOUT"
  create_view "Hardware" "TABLE_LAYOUT"
  create_view "Clean-room" "TABLE_LAYOUT"
  create_view "Backlog" "TABLE_LAYOUT"
  create_view "Releases" "ROADMAP_LAYOUT"
  create_view "Epics" "TABLE_LAYOUT"
fi

# ---------------------------------------------------------------------------
# Epics
# ---------------------------------------------------------------------------
say "Creating epics"
while IFS='|' read -r title area phase priority size risk hardware target goal demo; do
  [ -z "${title// }" ] && continue

  existing=$(gh issue list -R "$REPO" --state all --limit 200 --json number,title \
    --jq ".[] | select(.title == \"$title\") | .number" </dev/null | head -1)

  if [ -n "$existing" ]; then
    url="https://github.com/$REPO/issues/$existing"
    say "  epic exists: $title (#$existing)"
  else
    body=$(cat <<EOF
## Goal
$goal

## Demo
$demo

## Acceptance criteria
- [ ] The demo runs on iris.
- [ ] The demo runs on the target real Indy, or the limitation is recorded.
- [ ] Tests, documentation and clean-room checks are complete.
- [ ] The next milestone's dependencies are unblocked.

## Links
- Plan: https://github.com/$REPO/blob/main/docs/plan/milestones.md
- Architecture: https://github.com/$REPO/blob/main/docs/spec/architecture.md

## Out of scope
See the milestone entry in the plan.
EOF
)
    url=$(gh issue create -R "$REPO" --title "$title" --body "$body" \
      --label "type/epic" --label "$area" </dev/null)
    say "  epic created: $title ($url)"
  fi

  gh project item-add "$PROJECT_NUMBER" --owner "$ORG" --url "$url" </dev/null >/dev/null 2>&1 || true

  # Set the custom fields. Failures are non-fatal: the item may already be set
  # or the option may not exist yet.
  field() {
    local name="$1" value="$2"
    gh project item-edit "$PROJECT_NUMBER" --owner "$ORG" --url "$url" \
      --field "$name" --value "$value" </dev/null >/dev/null 2>&1 || true
  }
  field "Workflow" "Backlog"
  field "Phase" "$phase"
  field "Area" "$(case "$area" in
    area/kernel) echo Kernel;; area/boot) echo Boot;; area/compat) echo Compat;;
    area/drivers) echo Drivers;; area/display) echo Display;; area/storage) echo Storage;;
    area/network) echo Network;; area/toolchain) echo Toolchain;; area/infra) echo Infra;;
    area/docs) echo Docs;; area/security) echo Security;; area/legal) echo Legal;;
    area/clean-room) echo Clean-room;; area/hardware) echo Hardware;; *) echo Infra;; esac)"
  field "Priority" "$priority"
  field "Size" "$size"
  field "Risk" "$risk"
  field "Hardware" "$hardware"
  field "Target" "$target"
  if [ "$area" = "area/clean-room" ] || [ "$area" = "area/security" ]; then
    field "Clean-room" "Review"
  else
    field "Clean-room" "Not needed"
  fi
done <<'EPICS'
M0 — Toolchain unblocked|area/toolchain|M0 Toolchain|P0|L|High|None|Backlog|Make build-std and the GCC backend work reliably|CI cross-compiles a no_std binary for n32 and o32
M1 — ROM netboot|area/boot|M1 ROM|P0|L|High|iris|Backlog|A Rust ROM that fetches the kernel over HTTP|The ROM prints DHCP, digests and jumps to a stub
M2 — Kernel core|area/kernel|M2 Kernel|P0|XL|High|iris|Backlog|Traps, TLB, timer, allocator, scheduler, capabilities|Timer ticks, page fault, panic screen
M3 — Network + MCP + shell|area/network|M3 Network+MCP|P0|L|High|iris|v0.1|The development control plane|An agent connects and runs a command
M4 — SCSI + block + EROFS|area/storage|M4 Storage|P0|XL|High|iris|Backlog|The R2 root image over HTTP Range|cat /etc/motd from EROFS
M5 — First IRIX binary|area/compat|M5 Compat|P0|XL|High|iris|v0.2|Run an o32 IRIX binary unchanged|hello prints and exits
M6 — Display + native desktop|area/display|M6 Display|P1|XL|Medium|iris|v0.3|Newport display server and desktop|A window with text, cursor, damage repair
M7 — X11|area/display|M7 X11|P0|XL|High|Both|v0.4|A clean-room Rust X11 server|xterm and twm under a display lease
M8 — XFS read-only|area/storage|M8 XFS|P1|L|Medium|Both|v0.5|Read IRIX 6.5 XFS v4 disks|ls an IRIX disk
M9 — Native cell + capability ABI|area/kernel|M9 Native|P1|L|High|iris|v0.6|Load and run a native Rust cell|A cell opens a file and prints
M10 — Real-Indy release|area/infra|M10 Release|P0|XL|High|Indy|v1.0|Boot, run and update on a real Indy|A real Indy boots from R2 and runs MCP/X11/XFS
Clean-room and provenance|area/clean-room|Cross-cutting|P0|M|High|None|Backlog|Harness, similarity screen, DCO, access control|A kernel PR passes the screen
CI and automation|area/infra|Cross-cutting|P0|M|Medium|None|Backlog|Build, test, release and project automation|CI is green and the Project updates itself
Documentation|area/docs|Cross-cutting|P1|M|Low|None|Backlog|User, admin, install and API docs|The mdBook site is published
Security|area/security|Cross-cutting|P0|M|High|None|Backlog|Threat model, MCP modes, supply chain|A security review checklist is in use
Hardware lab|area/hardware|Cross-cutting|P1|M|Medium|Both|Backlog|The two Indys, rig, spares and safety|The hardware runner completes a smoke test
Performance and benchmarks|area/infra|Cross-cutting|P2|S|Low|Both|Backlog|Tracked benchmarks, no gates|A baseline is recorded for each milestone
Community and governance|area/docs|Cross-cutting|P2|S|Low|None|Backlog|Contributing, governance, recognition|A second contributor lands a PR
EPICS

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------
say "Done."
echo
echo "Project:  https://github.com/orgs/$ORG/projects/$PROJECT_NUMBER"
echo "Repo:     https://github.com/$REPO"
echo
echo "Next manual steps (no stable CLI/API):"
echo "  1. Configure the views' filters/grouping in the UI."
echo "  2. Create the main ruleset: PR, approvals, required checks, no force-push."
echo "  3. Enable CodeQL default setup, secret scanning and push protection."
echo "  4. Enable Pages (GitHub Actions source) and run the Docs workflow."
echo "  5. Create the release/hardware environments and secrets."
echo "  6. Register the self-hosted indy runner."
echo "  7. Set the PROJECT_URL variable and PROJECT_TOKEN secret for project.yml."
