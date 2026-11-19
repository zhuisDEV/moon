#!/usr/bin/env bash
set -euo pipefail

commit="${1:?Usage: bash require-main-ci.sh <commit>}"
if [[ ! "$commit" =~ ^[0-9a-f]{40}$ ]]; then
  echo 'Expected a full Git commit SHA.' >&2
  exit 2
fi

if ! response="$(gh api --method GET "repos/${GITHUB_REPOSITORY}/actions/workflows/ci.yml/runs" \
  -f branch=main -f event=push -f head_sha="$commit" -F per_page=100)"; then
  exit 2
fi

# Select the latest matching run before checking its result. A rerun must finish successfully too.
if ! run="$(jq -c --arg commit "$commit" --arg repo "$GITHUB_REPOSITORY_ID" '
  .workflow_runs
  | map(select(.head_sha == $commit and .head_branch == "main" and .event == "push"
    and .path == ".github/workflows/ci.yml" and (.head_repository.id | tostring) == $repo))
  | max_by(.run_number)
' <<< "$response")"; then
  exit 2
fi
if ! jq -e '.status == "completed" and .conclusion == "success"' <<< "$run" >/dev/null; then
  echo "Successful main CI is required for ${commit}. Wait for that commit’s CI to pass, then rerun." >&2
  exit 1
fi
jq -r '.html_url' <<< "$run"
