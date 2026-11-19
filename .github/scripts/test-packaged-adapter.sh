#!/usr/bin/env bash
set -euo pipefail
shopt -s nullglob

archives=("${1:?Usage: bash test-packaged-adapter.sh <bundle-directory>}"/moon-*.tar.gz)
test "${#archives[@]}" -eq 1
candidate="$(mktemp -d)"
trap 'rm -rf "$candidate"' EXIT
tar -xzf "${archives[0]}" -C "$candidate"
sh tools/test-openclaw-adapter.sh "$candidate/moon-release/bin/moon"
