# M.O.O.N.

> Strategic Memory Augmentation and Context Distillation for OpenClaw.

Moon is a Rust CLI plus an OpenClaw context-engine plugin for long-session
context control. It keeps the active context window under Moon control while
leaving the final provider request envelope to OpenClaw.

## What Moon Does

Moon has two runtime lanes:

1. Active lane: `moon context-engine`
   - records the current session checkpoint
   - refreshes the hot projection
   - runs `cleanse` only when pressure says compaction is needed
   - assembles the next active context packet
2. Maintenance lane: `moon watch`
   - projects library docs
   - embeds searchable collections
   - runs distillation and daily memory synthesis

Current active control flow:

1. `record`
2. routine hot `project`
3. conditional `cleanse`
4. operator `assemble`
5. active context packet generation

## Prompt Boundary

Moon does not own the final provider payload. The boundary is:

1. Moon owns:
   - checkpointing
   - hot projection refresh
   - conditional `cleanse`
   - operator assembly artifacts
   - active context packet generation
   - transcript compaction entries
2. OpenClaw owns:
   - final system prompt
   - message replay
   - tool definitions
   - provider dispatch
3. Routine Moon `systemPromptAddition` stays unused.
4. Moon injects model-facing active context through the `messages` lane.
5. Moon `cleanse` summaries still travel through transcript `compaction` entries
   and replay as `compactionSummary`.

## Current Model-Facing Design

Moon now uses three distinct surfaces:

1. Operator artifact
   - written under `$MOON_HOME/mce/`
   - rich debugging/control artifact
   - not shipped directly to the model
2. Active context packet
   - written under `$MOON_HOME/mcp/`
   - deterministic, model-facing packet
   - injected by the plugin into the OpenClaw `messages` lane
3. `cleanse` summary
   - written under `$MOON_HOME/cleanse/`
   - appended to transcript compaction when compaction runs
   - replayed later as `compactionSummary`

The plugin can also run a gated Moon-owned curator subagent over the packet when
configured, but deterministic local retrieval stays the first stage.

## Install

Source install:

```bash
export MOON_HOME="${MOON_HOME:-$HOME/.moon}"
mkdir -p "$MOON_HOME"
cp .env.example "$MOON_HOME/.env"
cp moon.toml.example "$MOON_HOME/moon.toml"

cargo install --path . --force
moon install
moon verify --strict
moon status
moon health
```

`moon install` does the following:

1. installs the OpenClaw Moon plugin into the OpenClaw extensions dir
2. writes Moon runtime docs into `$MOON_HOME`
3. writes Moon runtime skills into the OpenClaw state dir
4. patches OpenClaw config to:
   - enable the Moon plugin
   - set `plugins.slots.contextEngine = "moon"`
   - set `plugins.slots.memory = "none"`
   - set `agents.defaults.memorySearch.enabled = false`
5. wires Moon runtime paths into `plugins.entries.moon.config.*`
6. configures macOS launchd autostart when running from an installed binary
7. repairs secret-bearing Moon runtime paths to owner-only permissions:
   - `$MOON_HOME/.env`
   - `$MOON_HOME/auth/`
   - `$MOON_HOME/auth/openai-codex.json`
   - `$MOON_HOME/logs/`
   - `$MOON_HOME/logs/audit.log`
   - `$MOON_HOME/logs/distill.audit.log`

## Upgrade

Recommended upgrade path:

```bash
moon update
```

`moon update`:

1. upgrades to latest stable tag by default
2. supports `--channel main`
3. supports `--check` and `--dry-run`
4. reruns `moon install`
5. reruns `moon verify --strict`
6. preserves existing `$MOON_HOME/.env` and `$MOON_HOME/moon.toml`
7. repairs owner-only permissions for Moon-managed secret/runtime log paths

## Login

Moon supports OpenClaw-style OpenAI Codex OAuth for its remote lanes.

```bash
moon login --provider openai-codex
```

Moon stores managed auth in:

1. `$MOON_HOME/auth/openai-codex.json`

Moon keeps the auth store owner-only on Unix:

1. auth dir: `0700`
2. auth file: `0600`

Moon uses that auth for:

1. `cleanse` with `MOON_CLEANSE_PROVIDER=openai-codex`
2. remote `distill`
3. remote wisdom synthesis

## Security

Moon treats these runtime paths as secret-bearing or privacy-sensitive:

1. `$MOON_HOME/.env`
2. `$MOON_HOME/auth/`
3. `$MOON_HOME/auth/openai-codex.json`
4. `$MOON_HOME/logs/`
5. `$MOON_HOME/logs/audit.log`
6. `$MOON_HOME/logs/distill.audit.log`

Security contract:

1. `moon install` and `moon update` repair those paths to owner-only permissions
   on Unix (`0600` for files, `0700` for directories).
2. `moon verify --strict` fails if any of those paths are broader than
   owner-only.
3. `moon status` and `moon verify` continue masking configured API keys instead
   of printing them.
4. Remote provider failures now log only status plus request id when available;
   Moon no longer persists arbitrary remote error bodies into its audit paths.

## Runtime Root

Default runtime root:

1. `$HOME/.moon`

Key paths:

1. raw checkpoints: `$MOON_HOME/raw/`
2. hot projections: `$MOON_HOME/mds/`
3. library projections: `$MOON_HOME/mlib/`
4. cleanse summaries: `$MOON_HOME/cleanse/`
5. operator assembly artifacts: `$MOON_HOME/mce/`
6. active context packets: `$MOON_HOME/mcp/`
7. daily memory: `$MOON_HOME/memory/`
8. durable memory: `$MOON_HOME/MEMORY.md`
9. state: `$MOON_HOME/state/moon_state.json`
10. qmd runtime: `$MOON_HOME/qmd/`

## Minimal `.env`

```bash
MOON_HOME=$HOME/.moon

OPENCLAW_BIN=<optional-path-to-openclaw>
QMD_BIN=<path-to-qmd>

MOON_RAW_DIR=$MOON_HOME/raw
MOON_MDS_DIR=$MOON_HOME/mds
MOON_MLIB_DIR=$MOON_HOME/mlib
MOON_CLEANSE_DIR=$MOON_HOME/cleanse
MOON_MEMORY_DIR=$MOON_HOME/memory
MOON_MEMORY_FILE=$MOON_HOME/MEMORY.md
MOON_LOGS_DIR=$MOON_HOME/logs
MOON_CONFIG_PATH=$MOON_HOME/moon.toml
MOON_STATE_FILE=$MOON_HOME/state/moon_state.json
QMD_DB=$MOON_HOME/qmd/index.sqlite
QMD_CONFIG_DIR=$MOON_HOME/qmd/config

OPENCLAW_STATE_DIR=$HOME/.openclaw
OPENCLAW_CONFIG_PATH=$OPENCLAW_STATE_DIR/openclaw.json
OPENCLAW_SESSIONS_DIR=$OPENCLAW_STATE_DIR/agents/main/sessions
```

Provider examples:

```bash
# Gemini
MOON_CLEANSE_PROVIDER=gemini
MOON_CLEANSE_MODEL=gemini-3.1-flash-lite-preview
GEMINI_API_KEY=...

# OpenAI Responses
MOON_CLEANSE_PROVIDER=openai
MOON_CLEANSE_MODEL=gpt-4.1-mini
OPENAI_API_KEY=...

# OpenAI Codex OAuth
MOON_CLEANSE_PROVIDER=openai-codex
MOON_CLEANSE_MODEL=gpt-5.4
OPENAI_CODEX_BASE_URL=https://chatgpt.com/backend-api

# OpenAI-compatible
MOON_CLEANSE_PROVIDER=openai-compatible
MOON_CLEANSE_MODEL=deepseek-chat
AI_BASE_URL=https://api.deepseek.com
AI_API_KEY=...
```

## Recommended `moon.toml`

```toml
[context]
window_mode = "fixed"
window_tokens = 200000
compaction_authority = "moon"
cleanse_trigger_ratio = 0.50
cleanse_emergency_ratio = 0.90

[context_packet]
enabled = true
max_chars = 6000
max_candidates = 18
qmd_limit = 6
recent_memory_files = 4
recent_distill_docs = 4

[watcher]
poll_interval_secs = 60
cooldown_secs = 60

[distill]
max_per_cycle = 3
residential_timezone = "UTC"
syns_trigger_time_local = "00:00"
topic_discovery = true

[embed]
mode = "auto"
cooldown_secs = 60
max_docs_per_cycle = 3
min_pending_docs = 1
max_cycle_secs = 300

[hot_collection]
lifecycle_mode = "degrade"
lifecycle_command_mode = "primary"
```

## CLI

Global flags:

1. `--json`
2. `--allow-out-of-bounds`

Commands:

1. `install [--force] [--dry-run] [--apply true|false]`
2. `uninstall [--dry-run] [--purge] [--remove-binary]`
3. `login [--provider openai-codex] [--headless]`
4. `update [--check] [--dry-run] [--channel stable|main]`
5. `verify [--strict] [--verbose]`
6. `repair [--force]`
7. `status`
8. `health`
9. `record [--source <path>] [--session-id <id>] [--dry-run]`
10. `project [--source <path>] [--session-id <id>] [--lane hot|library|lib] [--dry-run]`
11. `cleanse [--source <path>] [--session-id <id>] [--dry-run]`
12. `assemble [--source <path>] [--session-id <id>] [--dry-run] [--replay-has-compaction-summary]`
13. `context-engine [--source <path>] [--session-id <id>] [--used-tokens <N>] [--max-tokens <N>] [--force-cleanse] [--replay-has-compaction-summary]`
14. `watch [--once] [--daemon] [--dry-run]`
15. `stop`
16. `restart`
17. `recall --query <text> [--name <collection>] [--limit <N>]`
18. `embed [--name <collection>] [--max-docs <N>] [--dry-run] [--watcher-trigger]`
19. `distill [--mode norm|syns] [--archive <path>] [--file <path> ...] [--session-id <id>] [--dry-run]`
20. `config [--show]`

Exit codes:

1. `0`: `ok=true`
2. `2`: `ok=false`
3. `1`: process/runtime error

## Common Workflows

One-command upgrade:

```bash
moon update
```

Strict integration verification:

```bash
moon verify --strict
```

Manual active-window checkpoint:

```bash
moon context-engine --used-tokens 65000 --max-tokens 200000
```

One maintenance cycle:

```bash
moon watch --once
```

Manual search and memory operations:

```bash
moon recall --name history_lib --query "recent decision about context packets"
moon embed --name history_lib --max-docs 25
moon distill --mode norm
moon distill --mode syns
```

## Operational Notes

1. `status` includes daemon lock and OpenClaw contract diagnostics.
2. `verify --strict` fails when plugin/runtime integration is unhealthy.
3. `verify --strict` also fails when Moon secret-bearing runtime files are not
   owner-only.
4. `watch --daemon` is blocked from development binaries for safety.
5. `moon install` may add a managed `MOON_HOME` block to `~/.zprofile`.
6. macOS installed-binary mode uses launchd label `com.moon.watch`.
7. Scheduled watcher `syns` runs once per local day at or after
   `distill.syns_trigger_time_local`; if Moon misses the exact window, it
   catches up later the same day.
8. Scheduled watcher `syns` uses the previous local day's
   `$MOON_HOME/memory/<day>.md` plus durable `MEMORY.md`; it does not fall back
   to the current day's daily-memory file.

## Uninstall

Moon now has a real uninstall command.

Safe default uninstall:

```bash
moon uninstall
```

Default uninstall removes:

1. OpenClaw Moon plugin assets
2. OpenClaw Moon install/config wiring
3. Moon runtime-generated artifacts:
   - `raw/`
   - `mds/`
   - `mlib/`
   - `cleanse/`
   - `mce/`
   - `mcp/`
   - `logs/`
   - `state/`
   - `qmd/`
   - generated runtime docs
4. Moon runtime skills under the OpenClaw state dir
5. the managed `MOON_HOME` shell block in `~/.zprofile`
6. launchd watcher plist on macOS

Default uninstall preserves:

1. `$MOON_HOME/memory/`
2. `$MOON_HOME/MEMORY.md`
3. `$MOON_HOME/.env`
4. `$MOON_HOME/moon.toml`
5. `$MOON_HOME/auth/`

Full wipe:

```bash
moon uninstall --purge
```

`--purge` removes the entire resolved `MOON_HOME`.

Optional binary removal:

```bash
moon uninstall --purge --remove-binary
```

`--remove-binary` attempts:

1. `cargo uninstall moon`

Use `--dry-run` first if you want the exact cleanup plan without mutating
anything.

## Repository Map

1. [src/cli.rs](./src/cli.rs)
2. [src/commands](./src/commands)
3. [src/moon](./src/moon)
4. [src/openclaw](./src/openclaw)
5. [assets/plugin](./assets/plugin)
6. [tests](./tests)
7. [docs](./docs)

## Additional Docs

1. [docs/runbook.md](./docs/runbook.md)
2. [docs/contracts.md](./docs/contracts.md)
3. [docs/failure_policy.md](./docs/failure_policy.md)
4. [docs/security_checklist.md](./docs/security_checklist.md)
5. [CHANGELOG.md](./CHANGELOG.md)
6. [RELEASE.md](./RELEASE.md)
7. [SUPPORT.md](./SUPPORT.md)
8. [GOVERNANCE.md](./GOVERNANCE.md)
