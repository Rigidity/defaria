#!/bin/sh
# Close the epic above an issue once every one of its sub-issues is closed,
# and the epic above that, and so on up. Run by
# .github/workflows/close-finished-epics.yml when an issue closes; GitHub
# doesn't start workflows for issues the workflow's own token closes, so the
# whole chain is handled here.
#
# Usage:
#   scripts/close-finished-epics.sh <issue>
#
# Set DRY_RUN=1 to print what it would close without closing anything.
# Uses GH_REPO if set, otherwise the current repository.
set -eu

[ $# -eq 1 ] || {
    sed -n 's/^#   /  /p' "$0" >&2
    exit 1
}
repo=${GH_REPO:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}
issue=$1

while :; do
    parent=$(gh api "repos/$repo/issues/$issue" \
        --jq '.parent_issue_url // "" | split("/") | last // ""')
    [ -n "$parent" ] || exit 0

    state=$(gh api "repos/$repo/issues/$parent" --jq .state)
    [ "$state" = open ] || exit 0

    # Every sub-issue closed, and there is at least one.
    open=$(gh api --paginate "repos/$repo/issues/$parent/sub_issues?per_page=100" \
        --jq '.[] | select(.state == "open") | .number' | wc -l | tr -d ' ')
    total=$(gh api "repos/$repo/issues/$parent" --jq .sub_issues_summary.total)
    if [ "$open" -ne 0 ] || [ "$total" -eq 0 ]; then
        exit 0
    fi

    if [ "${DRY_RUN:-}" = 1 ]; then
        echo "would close #$parent"
    else
        gh issue close "$parent" --repo "$repo" --reason completed \
            --comment "All of its sub-issues are closed, the last being #$issue."
        echo "closed #$parent"
    fi
    issue=$parent
done
