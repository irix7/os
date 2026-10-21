# The Tree of IRIX 7

The interactive roadmap for IRIX 7. It is a static site generated from one
data file, so the tree grows when the plan does.

- **Live:** <https://irix7.github.io/os/tree/>
- **Source of truth:** [`roadmap.json`](roadmap.json)
- **Generator:** [`build.py`](build.py)
- **Assets:** [`assets/`](assets/)

## Build

The generator uses Python 3 and the standard library only.

```sh
python3 tree/build.py --check          # validate the roadmap
python3 tree/build.py                  # writes tree/dist/
python3 -m http.server -d tree/dist    # preview at http://localhost:8000
```

The GitHub Pages workflow builds the tree into `book/tree` after mdBook has
built the documentation, so it appears at `/os/tree/` alongside the book.

## Keeping it current

`tree/sync.py` promotes milestone statuses from evidence, strongest first:

1. a release tag (`v0.1` … `v1.0`) exists;
2. the **IRIX 7 — Roadmap** Project says a phase is in progress, in review or
   done (requires a `PROJECT_TOKEN` secret with `project` scope);
3. a milestone epic issue is closed (always available, via `GITHUB_TOKEN`).

The sync is monotonic: it can move planned/exploring → active → done, but it
never quietly moves work backwards. If the Project is behind the tree, the
workflow reports that in its step summary instead of applying it. A completed
milestone also completes its feature leaves. The date in `meta.updated` only
moves when a status actually changes.

The `Docs` workflow runs the sync on a daily schedule, on pushes to `main` and
on manual dispatch, then rebuilds and deploys. The scheduled run publishes the
synced data immediately; if the sync changes the JSON it also opens or updates
a `roadmap/sync` pull request (with the sync report as its body) so the
repository stays in step. Merge that PR when convenient. `main` is
PR-protected, so the workflow never pushes directly to it.

The PR is opened with `PROJECT_TOKEN` when available and otherwise with
`GITHUB_TOKEN`; the repository setting “Allow GitHub Actions to create and
approve pull requests” must be on for the fallback.

To preview the sync locally:

```sh
PROJECT_TOKEN="$(gh auth token)" python3 tree/sync.py --dry-run
python3 -m unittest discover -s tree -p 'test_*.py'
```

`PROJECT_TOKEN` is optional. Without it the sync still checks release tags and
closed epics, and warns that the Project was skipped. Configure
`vars.PROJECT_URL` only if the Project moves.

## Editing the roadmap

`roadmap.json` has five parts:

- `meta` — title, introduction, repository and documentation links.
- `statuses` — the five states: done, active, planned, exploring, pruned.
- `branches` — the colour and name of each part of the tree.
- `dependencies` — the cross-branch dotted arcs.
- `nodes` — milestones, features, releases, workstreams and pruned ideas.

Every node has an `id`, `kind`, `branch`, `status`, `title` and `summary`.
Milestones and releases also need an `anchor: [x, y]` in the 3000 × 2300 world.
Feature positions are generated along their parent milestone's bough, so adding
a feature only needs a `parent` and a short `summary`.

```json
{
  "id": "m4-cache",
  "kind": "feature",
  "parent": "m4",
  "branch": "storage",
  "status": "planned",
  "title": "Block cache",
  "summary": "An LRU cache with explicit DMA sync."
}
```

Statuses are synced from the Project and release tags, with manual editorial
control for exploring branches, features and pruned ideas. Update a status by
moving the work on the Project or cutting a release; the next sync carries it
through.

## Design

- The canvas is a single generated SVG: roots (M0–M2), trunk (M3), boughs
  (storage, compatibility, graphics, native), and the v1.0 sun in the canopy.
- Dotted arcs are cross-branch dependencies. The gold arcs are the v1.0
  convergence.
- The compost is deliberate: it records what v1.0 leaves out.
- The workstreams are the mycelium: they run under everything.

## Licences

The code and content are GPL-3.0, like the rest of IRIX 7.

The site bundles three SIL Open Font License typefaces. Their licences are in
[`assets/fonts/`](assets/fonts/):

- Fraunces — Copyright 2018 The Fraunces Project Authors.
- Space Grotesk — Copyright 2018 The Space Grotesk Project Authors.
- IBM Plex Mono — Copyright 2017 IBM Corp.
