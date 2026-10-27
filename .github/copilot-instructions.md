# Copilot instructions

This file is read by GitHub Copilot when it reviews pull requests in this
repository. Use it to judge whether a PR's changes are documented
sufficiently: new behaviour, new flags, new files, and changed interfaces
should each be reflected in the appropriate docs under `docs/` and `book/`.

## Documentation review scope

When auditing a pull request, check that:

- User-facing behaviour changes are described in `docs/`.
- New or changed configuration options are documented.
- Public APIs, ABIs, and module interfaces have corresponding docs.
- The `book/` roadmap and `tree/` statuses stay consistent with the change.

If any of the above are missing, flag them in your review comments. There is
no machine-readable `DOC_STATUS:` token — report findings as ordinary review
comments.

## Triggering a Copilot review

A Copilot review is requested programmatically from CI. The workflow lives at
`.github/workflows/copilot-documentation-compliance.yml` and runs on pull
request `opened`, `synchronize`, and `edited` events. It requests
`copilot-pull-request-reviewer[bot]` as a reviewer via the GitHub REST API,
which causes Copilot to evaluate the PR diff and post review comments.

Copilot's review is free-text commentary, not a structured pass/fail verdict,
so it cannot be used as a hard merge gate on its own.
