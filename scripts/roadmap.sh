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
