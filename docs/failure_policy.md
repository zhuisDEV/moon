# moon System Failure Policy

## Principles

1. `record` before any destructive reduction or context replacement.
2. Prefer explicit degraded behavior over silent corruption.
3. Always emit audit detail for failures and retries.
4. Do not silently mix fallback behavior into the normal MOON-owned path.
5. Background maintenance failures must not redefine the control plane.

## Warning Format

Emit AI-readable warning lines for actionable failures:

`MOON_WARN code=<CODE> stage=<STAGE> action=<ACTION> session=<SESSION_ID> source=<SOURCE_PATH> retry=<RETRY_POLICY> reason=<REASON> err=<ERR_SUMMARY>`

## Warning Codes

1. `STATE_CORRUPT`
2. `RECORD_FAILED`
3. `PROJECT_FAILED`
4. `CLEANSE_FAILED`
5. `ASSEMBLE_FAILED`
6. `RECALL_FAILED`
7. `EMBED_FAILED`
8. `EMBED_LOCKED`
9. `EMBED_CAPABILITY_MISSING`
10. `EMBED_STATUS_FAILED`
11. `DISTILL_FAILED`
12. `SYNS_FAILED`
13. `CWD_INVALID`

## Warning Triage

1. `STATE_CORRUPT`: inspect the state file, corrupt backup, and recent writes under `$MOON_HOME/state/`.
2. `RECORD_FAILED`: verify active session source visibility, read permissions, and `$MOON_HOME/raw/` write permissions.
3. `PROJECT_FAILED`: verify raw-session readability, parseability, and `$MOON_HOME/mds/` write permissions.
4. `CLEANSE_FAILED`: verify `MOON_CLEANSE_PROVIDER`, `MOON_CLEANSE_MODEL`, provider credentials, and remote model health.
5. `ASSEMBLE_FAILED`: verify the required control inputs exist and that the latest applicable `cleanse` summary is readable when compaction ran.
6. `RECALL_FAILED`: verify `QMD_BIN`, collection availability, and query execution.
7. `EMBED_FAILED` / `EMBED_STATUS_FAILED`: verify bounded `qmd embed` support, lock/state paths, and QMD command health.
8. `EMBED_LOCKED`: another embed worker is active; retry later.
9. `EMBED_CAPABILITY_MISSING`: installed QMD build lacks bounded embed support (`--max-docs`); upgrade QMD.
10. `DISTILL_FAILED`: verify projection markdown input and L1 output paths.
11. `SYNS_FAILED`: verify `MOON_WISDOM_PROVIDER`, `MOON_WISDOM_MODEL`, provider credentials, and selected source files.
12. `CWD_INVALID`: run from the expected workspace tree or use `--allow-out-of-bounds` intentionally.

## Stage Policies

## State / Startup

Failure:

1. state file read failure
2. state file JSON corruption

Policy:

1. preserve a best-effort corrupt backup when possible
2. emit `STATE_CORRUPT`
3. continue with a fresh default state instead of crashing the runtime

## `record`

Failure:

1. active session source missing
2. source unreadable
3. raw write failure

Policy:

1. hard stop downstream primary-path work for that checkpoint
2. do not run `project`, `cleanse`, or `assemble` against missing/unrecorded state
3. preserve the current active session unchanged
4. allow retry after operator fixes source/path issues

## `project`

Failure:

1. raw source parse failure
2. projection write failure

Policy:

1. preserve the recorded raw source as the canonical truth
2. do not fabricate projection output
3. degrade background memory/search maintenance only
4. allow later retry without blocking the already-recorded raw checkpoint

## `cleanse`

Failure:

1. provider unavailable or timeout
2. credentials/model misconfiguration
3. model output contract failure
4. cleanse write failure

Policy:

1. preserve raw state and current active session unchanged
2. do not overwrite prior valid `cleanse` output with partial/invalid content
3. surface the error clearly for operator retry
4. do not silently delegate normal-path compaction ownership to fallback logic

## `assemble`

Failure:

1. required control inputs missing
2. context composition fails validation
3. MOON cannot produce a coherent pre-dispatch payload

Policy:

1. fail the MOON-owned dispatch preparation explicitly
2. do not silently dispatch with an unknown or partial context
3. do not silently hand the normal path back to OpenClaw
4. require operator or explicit higher-level recovery logic to proceed

## `recall`

Failure:

1. QMD query/search execution failure
2. no matches

Policy:

1. return structured empty/no-hit behavior for no-match cases
2. surface command execution failure clearly when retrieval infrastructure is broken
3. never treat no-match as a hard control-plane failure by itself

## `embed`

Failure:

1. bounded embed capability missing
2. embed lock active
3. embed command failed
4. embed status reported failure

Policy:

1. watcher-triggered embed may degrade and continue the cycle
2. manual embed returns `ok=false` on lock/capability/command failures
3. never fall back to unbounded embed behavior
4. always append embed audit detail

## `distill --mode norm`

Failure:

1. projection markdown missing
2. L1 processing failure
3. summary/audit write failure

Policy:

1. preserve the source projection markdown unchanged
2. mark the item as still pending for later retry
3. do not block the primary control plane if raw capture succeeded

## `distill --mode syns`

Failure:

1. synthesis provider unavailable
2. provider/model misconfiguration
3. output contract failure

Policy:

1. skip the synthesis run and preserve existing durable memory
2. surface operator action clearly
3. do not let `syns` failure redefine active context control

## Transitional Watcher

Failure:

1. one-shot cycle failure
2. daemon loop failure
3. watcher-owned maintenance stages fail

Policy:

1. treat the watcher as transitional long-running maintenance infrastructure only
2. do not treat watcher failure as proof that the short-lived `moon-context-engine` control model is invalid
3. allow degraded maintenance behavior when safe
4. do not let watcher behavior redefine the target architecture or active-window ownership

## CLI Workspace Boundary

Failure:

1. mutating command is executed outside the expected workspace boundary

Policy:

1. return a structured boundary error
2. keep diagnostic commands runnable from any directory
3. allow explicit operator bypass with `--allow-out-of-bounds`
4. support env-gated bypass via `MOON_ALLOW_OUT_OF_BOUNDS` (truthy: `1`, `true`, `yes`, `on`)
