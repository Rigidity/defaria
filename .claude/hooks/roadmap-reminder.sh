#!/bin/sh
# A PostToolUse hook on Bash: after `gh issue create`, tell the agent to put
# the new issue in the roadmap project, in its place in the order. It doesn't
# add the issue itself, because choosing where it goes takes judgement (see
# "Issues and the roadmap" in AGENTS.md).
set -eu

input=$(cat)
command=$(printf '%s' "$input" | jq -r '.tool_input.command // ""')
case "$command" in
*"gh issue create"*) ;;
*) exit 0 ;;
esac

issues=$(printf '%s' "$input" | jq -r '.tool_response | tostring' |
    grep -oE 'github\.com/Rigidity/defaria/issues/[0-9]+' |
    sed 's#.*/##' | sort -un | sed 's/^/#/' | paste -sd ' ' -)
[ -n "$issues" ] || exit 0

message="You filed $issues. Unless it's an epic, add it to the roadmap \
project where it belongs by priority: read \`scripts/roadmap.sh list\`, then \
\`scripts/roadmap.sh add <issue> <order>\` (7.5 goes between 7 and 8). Put it \
under its epic if it has one. Finish with \`scripts/roadmap.sh check\`."

jq -n --arg message "$message" \
    '{hookSpecificOutput: {hookEventName: "PostToolUse", additionalContext: $message}}'
