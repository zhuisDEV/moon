#!/bin/sh
# Run the adapter against synthetic data only, with real-binary tests required.
set -eu

if [ "$#" -ne 1 ] || [ ! -x "$1" ]; then
  echo 'Usage: sh tools/test-openclaw-adapter.sh /path/to/moon' >&2
  exit 2
fi
moon_binary="$(cd "$(dirname "$1")" && pwd -P)/$(basename "$1")"
repo_root="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd -P)"
fixture_root="$(mktemp -d "${TMPDIR:-/tmp}/moon-adapter-test.XXXXXX")"
trap 'rm -rf "$fixture_root"' EXIT HUP INT TERM
mkdir -m 700 "$fixture_root/home" "$fixture_root/legacy" "$fixture_root/legacy/mlib"
printf '%s\n' 'A participant can reenter using roomKey and redemptionKey.' \
  > "$fixture_root/legacy/mlib/reentry.md"
env -u MOON_DATABASE -u MOON_HOME -u MOON_EMBEDDING_DIMENSIONS \
  "$moon_binary" --home "$fixture_root/home" \
  --database "$fixture_root/home/state/moon.sqlite" --dimensions 384 --json \
  import-legacy --source-home "$fixture_root/legacy" > /dev/null

# These deliberately hostile overrides must not escape the configured test home.
MOON_DATABASE="$fixture_root/ambient.sqlite" \
MOON_HOME="$fixture_root/unintended-home" \
MOON_EMBEDDING_DIMENSIONS=64 \
MOON_TEST_BINARY="$moon_binary" \
MOON_TEST_HOME="$fixture_root/home" \
MOON_TEST_MODE=lexical \
MOON_TEST_QUERY='roomKey redemptionKey participant reenter' \
MOON_TEST_EXPECTED='redemptionKey' \
MOON_REQUIRE_REAL_BINARY=1 \
deno test --node-modules-dir=none \
  --allow-read="$repo_root/assets/openclaw-plugin,$fixture_root" \
  --allow-env=MOON_TEST_BINARY,MOON_TEST_HOME,MOON_TEST_MODE,MOON_TEST_QUERY,MOON_TEST_EXPECTED,MOON_REQUIRE_REAL_BINARY \
  --allow-run="$moon_binary" \
  "$repo_root/assets/openclaw-plugin/index.test.ts" \
  "$repo_root/assets/openclaw-plugin/compaction-input.test.ts"

if [ -e "$fixture_root/ambient.sqlite" ] || [ -e "$fixture_root/unintended-home" ]; then
  echo 'Adapter test escaped its explicit Moon runtime.' >&2
  exit 1
fi
