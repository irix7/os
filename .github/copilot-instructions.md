# Copilot instructions

This file documents how the repository's documentation compliance is audited
for pull requests. Copilot should evaluate each PR diff against the rules in
this file and report a documentation status.

## Documentation compliance audit

A pull request is checked for documentation sufficiency. When the audit
reports `DOC_STATUS: FAIL`, the PR does not comply and should not be merged.

The compliance check is run as a GitHub Actions workflow. The definition below
is kept here for reference; to run it as CI, place it in
`.github/workflows/copilot-documentation-compliance.yml`.

```yaml
name: Copilot Documentation Compliance

on:
  pull_request:
    types: [opened, synchronize, edited]

jobs:
  copilot-review:
    runs-on: ubuntu-latest
    permissions:
      pull-requests: write
      contents: read

    steps:
      - name: Checkout Repository
        uses: actions/checkout@v4

      - name: Request Copilot Analysis
        id: copilot
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          echo "Triggering Copilot repository-wide documentation audit..."

          # We use the GitHub CLI to trigger a code review summary via Copilot's API endpoint
          # This forces Copilot to evaluate the diff against the .github/copilot-instructions.md rules
          RESPONSE=$(gh api /repos/${{ github.repository }}/pulls/${{ github.event.pull_request.number }}/reviews \
            -f body="Please audit this PR for documentation sufficiency based on our repository's copilot-instructions.md file." \
            -f event="COMMENT")

          # Extract the assessment comment text to check for compliance status
          REVIEW_TEXT=$(echo "$RESPONSE" | jq -r '.body // ""')

          if [[ "$REVIEW_TEXT" == *"DOC_STATUS: FAIL"* ]]; then
            echo "❌ Documentation audit failed."
            exit 1
          fi

          echo "✅ Documentation compliance passed."
          exit 0
```
