#!/bin/sh
# Upload screenshots for a pull request and print Markdown to paste into its
# description. GitHub has no API for attaching images to a PR, so the images
# are committed to a separate branch that holds only screenshots, and linked
# by commit, so later uploads never change an existing link.
#
# Usage: scripts/pr-screenshots.sh <pr-number> <image.png> [<image.png>...]
#
# Images land in pr-<number>/<file name> on the screenshots branch. Name the
# files after what they show (close-up.png, far-view.png); the name becomes
# the image's alt text. Doesn't touch your working tree, index or branch.
# The links only render because the repository is public.
set -eu

branch=rigidity/pr-screenshots
remote=origin

if [ $# -lt 2 ]; then
    echo "usage: $0 <pr-number> <image.png> [<image.png>...]" >&2
    exit 1
fi
pr=$1
shift
case $pr in
    *[!0-9]* | '') echo "pr-screenshots: '$pr' isn't a PR number" >&2; exit 1 ;;
esac
for file in "$@"; do
    [ -f "$file" ] || { echo "pr-screenshots: no such file: $file" >&2; exit 1; }
done

# Build the new commit in a scratch index, on top of the branch if it exists.
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
export GIT_INDEX_FILE="$scratch/index"
if git fetch -q "$remote" "refs/heads/$branch" 2>/dev/null; then
    parent=$(git rev-parse FETCH_HEAD)
    git read-tree "$parent"
else
    parent=
    git read-tree --empty
fi
for file in "$@"; do
    blob=$(git hash-object -w "$file")
    git update-index --add --cacheinfo "100644,$blob,pr-$pr/$(basename "$file")"
done
tree=$(git write-tree)
commit=$(git commit-tree "$tree" ${parent:+-p "$parent"} -m "Screenshots for PR #$pr")
git push -q "$remote" "$commit:refs/heads/$branch"

repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
for file in "$@"; do
    name=$(basename "$file")
    echo "![${name%.*}](https://raw.githubusercontent.com/$repo/$commit/pr-$pr/$name)"
done
