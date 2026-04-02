# MIP: Moon-Owned Memory And OpenClaw Setup (Completed)

## Status

1. Completed on `2026-04-02`.
2. Scope delivered for the primary Moon-owned flow only.
3. OpenClaw fallback memory behavior remains intentionally out of scope.

## Problem (Resolved)

Moon already owned context assembly through `moon context-engine`, but OpenClaw
memory ownership was previously left implicit. If `plugins.slots.memory` was
unset, OpenClaw could still resolve to `memory-core` and keep legacy memory
behavior active, which conflicted with Moon-owned runtime expectations.

## Delivered Changes

### Install and Repair

1. `moon install` now enforces:
   - `plugins.slots.memory = "none"`
   - `agents.defaults.memorySearch.enabled = false`
2. Existing stale memory settings are repaired on subsequent installs.
3. Existing Moon runtime/plugin wiring behavior remains in place:
   `contextEngine`, plugin runtime paths, and provenance records.

### Verification and Status

1. `moon verify --strict` now fails when memory contract keys are stale or
   missing, with exact key-level issue text.
2. `moon status` and `status` diagnostics now surface:
   - raw memory slot value
   - resolved memory slot value
   - legacy memory search enabled/disabled state
3. Drift is reported explicitly for:
   - missing `plugins.slots.memory`
   - non-`none` `plugins.slots.memory`
   - missing `agents.defaults.memorySearch.enabled`
   - non-`false` `agents.defaults.memorySearch.enabled`

### Documentation

1. Updated `README.md` to document Moon-owned memory contract behavior.
2. Updated `docs/runbook.md` with operator guidance for memory contract drift.

## Test Coverage Added

1. Install patch regression: stale memory contract is repaired.
2. Strict verify regressions:
   - stale memory contract fails
   - missing memory contract fails
3. Status regression:
   - memory drift is surfaced with exact key diagnostics
4. Clean path regression:
   - install + strict verify succeeds with correct Moon-owned contract

## Acceptance Criteria

1. [x] `moon install` writes and repairs a Moon-owned OpenClaw memory contract.
2. [x] `moon verify --strict` catches legacy/missing OpenClaw memory settings.
3. [x] `moon status` reports exact bad keys for memory contract drift.
4. [x] Moon-owned installs no longer rely on implicit OpenClaw memory defaults.

## Notes

1. This completion intentionally keeps primary and fallback behavior separated.
2. Any future fallback policy should be specified in a separate MIP and release.
