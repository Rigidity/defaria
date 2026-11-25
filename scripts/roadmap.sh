#!/bin/sh
# Keep the roadmap project (https://github.com/users/Rigidity/projects/3) in
# step with the issues: add new ones in their place in the order, and move
# them along as work starts. Closing an issue or merging its PR marks it Done
# on its own.
#
# Usage:
#   scripts/roadmap.sh list                    the issues, in order, with status
#   scripts/roadmap.sh add <issue> <order>     add an issue at that place
#   scripts/roadmap.sh order <issue> <order>   move an issue to another place
#   scripts/roadmap.sh status <issue> <todo|doing|done>
#   scripts/roadmap.sh check                   what's out of step; fails if anything is
#
# <order> is the Order field, a number: work goes in increasing order. To put
# an issue between 7 and 8, give it 7.5.
#
# Needs the `project` scope: gh auth refresh -h github.com -s project
set -eu

owner=Rigidity
number=3
project=PVT_kwHOAhvc6s4BtNmp
order_field=PVTF_lAHOAhvc6s4BtNmpzhrYIVs
status_field=PVTSSF_lAHOAhvc6s4BtNmpzhrYIRQ

usage() {
    sed -n 's/^#   /  /p' "$0" >&2
    exit 1
}

# The project item for issue $1 of this repository.
item() {
    id=$(gh project item-list "$number" --owner "$owner" --limit 1000 \
        --format json \
        --jq ".items[] | select(.content.number == $1) | .id")
    if [ -z "$id" ]; then
        echo "issue #$1 isn't in the project; add it first" >&2
        exit 1
    fi
    echo "$id"
}

set_order() {
    gh project item-edit --id "$1" --project-id "$project" \
        --field-id "$order_field" --number "$2" >/dev/null
}

[ $# -ge 1 ] || usage
case "$1" in
list)
    gh project item-list "$number" --owner "$owner" --limit 1000 \
        --format json \
        --jq '.items | sort_by(.order) | .[]
            | "\(.order)\t#\(.content.number)\t\(.status)\t\(.content.title)"'
    ;;
add)
    [ $# -eq 3 ] || usage
    url=$(gh issue view "$2" --json url --jq .url)
    id=$(gh project item-add "$number" --owner "$owner" --url "$url" \
        --format json --jq .id)
    set_order "$id" "$3"
    ;;
order)
    [ $# -eq 3 ] || usage
    id=$(item "$2")
    set_order "$id" "$3"
    ;;
check)
    # Open issues that aren't epics belong in the project, each with an
    # Order; epics stay out.
    items=$(gh project item-list "$number" --owner "$owner" --limit 1000 \
        --format json)
    issues=$(gh issue list --state open --limit 1000 \
        --json number,title,labels)
    problems=$(jq -rn --argjson items "$items" --argjson issues "$issues" '
        ($items.items | map(select(.content.type == "Issue"))) as $items
        | ($items | map(.content.number)) as $in
        | ($issues | map(select(any(.labels[]; .name == "epic")))
            | map(.number)) as $epics
        | ($issues[]
            | select((.number | IN($epics[])) | not)
            | select((.number | IN($in[])) | not)
            | "#\(.number) isn'"'"'t in the project: \(.title)"),
          ($items[] | select(.order == null)
            | "#\(.content.number) has no Order: \(.content.title)"),
          ($items[] | select(.content.number | IN($epics[]))
            | "#\(.content.number) is an epic; epics stay out: \(.content.title)"),
          ($items | map(select(.order != null)) | group_by(.order)[]
            | select(length > 1)
            | "Order \(.[0].order) is shared by "
                + (map("#\(.content.number)") | join(", ")))
    ')
    # Open epics whose work is all done should be closed; the
    # close-finished-epics workflow normally does it.
    for epic in $(printf '%s' "$issues" |
        jq -r '.[] | select(any(.labels[]; .name == "epic")) | .number'); do
        open=$(gh api --paginate \
            "repos/$owner/defaria/issues/$epic/sub_issues?per_page=100" \
            --jq '.[] | select(.state == "open") | .number' | wc -l)
        total=$(gh api "repos/$owner/defaria/issues/$epic" \
            --jq .sub_issues_summary.total)
        if [ "$open" -eq 0 ] && [ "$total" -gt 0 ]; then
            problems="$problems${problems:+
}#$epic is an epic with every sub-issue closed; close it"
        fi
    done
    if [ -n "$problems" ]; then
        echo "$problems" >&2
        exit 1
    fi
    echo "The roadmap is in step with the issues."
    ;;
status)
    [ $# -eq 3 ] || usage
    case "$3" in
    todo) option=f75ad846 ;;
    doing) option=47fc9ee4 ;;
    done) option=98236657 ;;
    *) usage ;;
    esac
    id=$(item "$2")
    gh project item-edit --id "$id" --project-id "$project" \
        --field-id "$status_field" --single-select-option-id "$option" \
        >/dev/null
    ;;
*)
    usage
    ;;
esac
