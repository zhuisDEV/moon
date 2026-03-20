# M.O.O.N.

> Strategic Memory Augmentation and Context Distillation for OpenClaw.

Moon v1 is a Rust CLI and OpenClaw plugin that helps agents keep context under
control across long sessions. It records raw session checkpoints, compacts when
needed, projects searchable docs, and maintains durable memory.

## Overview

Moon has two runtime lanes:

1. Active lane (short-lived): `moon context-engine` controls normal turn-time
   context preparation (`record` -> conditional `cleanse` -> `assemble`).
2. Maintenance lane (background/transitional): `moon watch` handles library
   projection/embed/distill maintenance cycles.

Moon v1 command surface includes:

1. Admin/bootstrap: `install`, `update`, `verify`, `repair`, `status`, `config`, `health`
2. Runtime stages: `record`, `project`, `cleanse`, `assemble`, `context-engine`
3. Search/memory: `recall`, `embed`, `distill`
4. Watcher control: `watch`, `stop`, `restart`

## Read This First

For first-time setup from repo source, read [BOOTSTRAP.md](./BOOTSTRAP.md)
first.

After `moon install`, Moon exports runtime docs into `$MOON_HOME`:

1. `$MOON_HOME/README.md`
2. `$MOON_HOME/.env.example`
3. `$MOON_HOME/moon.toml.example`
4. `$MOON_HOME/docs/troubleshooting.md`

`BOOTSTRAP.md` remains repo-only pre-install guidance and is not copied into
`$MOON_HOME`.

## Quick Start (Source Build)

```bash
export MOON_HOME="${MOON_HOME:-$HOME/.moon}"
mkdir -p "$MOON_HOME"
cp .env.example "$MOON_HOME/.env"
cp moon.toml.example "$MOON_HOME/moon.toml"
$EDITOR "$MOON_HOME/.env"
$EDITOR "$MOON_HOME/moon.toml"

cargo install --path . --force
moon install
moon verify --strict
moon status
moon health
moon config --show
```

## Upgrade

Recommended upgrade path:

```bash
moon update
```

`moon update`:

1. Upgrades to latest stable tag by default (`--channel stable`).
2. Supports `--channel main` for main-branch builds.
3. Supports `--check` and `--dry-run`.
4. Runs `moon install` and `moon verify --strict` after install.
5. Preserves existing `$MOON_HOME/.env` and `$MOON_HOME/moon.toml`.

Manual source reinstall path:

```bash
cargo install --path . --force
moon install
moon verify --strict
```

## Runtime Assumptions

1. Runtime root is `MOON_HOME`.
2. If `MOON_HOME` is unset, Moon defaults to `$HOME/.moon`.
3. Moon loads env only from `$MOON_HOME/.env`.
4. If `$MOON_HOME/.env` is missing/unreadable, Moon exits with error.

## Workspace Boundary Safety

Mutating commands enforce CWD boundaries by default.

1. Mutating commands validate CWD against daemon-recorded workspace (or explicit
   `MOON_HOME` when no daemon lock exists).
2. Diagnostics are exempt: `status`, `health`, `verify`, `config`, `update`.
3. Bypass with `--allow-out-of-bounds`.
4. Or set `MOON_ALLOW_OUT_OF_BOUNDS=1`.

## Minimal `.env` Baseline

```bash
# Required runtime root
MOON_HOME=$HOME/.moon

# External binaries
OPENCLAW_BIN=<optional-path-to-openclaw>
QMD_BIN=<path-to-qmd>

# Optional explicit runtime paths (defaults derive from MOON_HOME)
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

# OpenClaw state defaults
OPENCLAW_STATE_DIR=$HOME/.openclaw
OPENCLAW_CONFIG_PATH=$OPENCLAW_STATE_DIR/openclaw.json
OPENCLAW_SESSIONS_DIR=$OPENCLAW_STATE_DIR/agents/main/sessions
```

## Recommended `moon.toml` Baseline

```toml
[context]
window_mode = "fixed"
window_tokens = 200000
compaction_authority = "moon"
cleanse_trigger_ratio = 0.50
cleanse_emergency_ratio = 0.90

[watcher]
poll_interval_secs = 60
cooldown_secs = 60

[distill]
max_per_cycle = 3
residential_timezone = "UTC"
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

Binary: `moon`

```bash
moon <command> [flags]
```

Global flags:

1. `--json`
2. `--allow-out-of-bounds`

Commands:

1. `install [--force] [--dry-run] [--apply true|false]`
2. `update [--check] [--dry-run] [--channel stable|main]`
3. `verify [--strict]`
4. `repair [--force]`
5. `status`
6. `record [--source <path>] [--session-id <id>] [--dry-run]`
7. `project [--source <path>] [--session-id <id>] [--lane hot|library|lib] [--dry-run]`
8. `cleanse [--source <path>] [--session-id <id>] [--dry-run]`
9. `assemble [--source <path>] [--session-id <id>] [--dry-run]`
10. `context-engine [--source <path>] [--session-id <id>] [--used-tokens <N>] [--max-tokens <N>] [--force-cleanse]`
11. `watch [--once] [--daemon] [--dry-run]`
12. `stop`
13. `restart`
14. `recall --query <text> [--name <collection>] [--limit <N>]`
15. `embed [--name <collection>] [--max-docs <N>] [--dry-run] [--watcher-trigger]`
16. `distill [--mode norm|syns] [--archive <path>] [--file <path> ...] [--session-id <id>] [--dry-run]`
17. `config [--show]`
18. `health`

Exit codes:

1. `0`: command completed with `ok=true`
2. `2`: command completed with `ok=false`
3. `1`: runtime/process error

## Command Notes

1. `status` now includes daemon lock/runtime checks and can fail when lock is
   stale or autostart state is inconsistent.
2. `verify --strict` fails hard when runtime/plugin diagnostics are unhealthy.
3. `distill --mode norm` auto-selects a pending `$MOON_HOME/mlib/*.md` file if
   `--archive` is omitted.
4. `watch --daemon` is blocked from development binaries (`target/debug` or
   `target/release` path) for safety; use installed binary mode.

## Common Workflows

After OpenClaw upgrade:

```bash
moon install
moon verify --strict
```

One-command runtime upgrade:

```bash
moon update
```

Manual active-window run:

```bash
moon context-engine --used-tokens 65000 --max-tokens 200000
```

Run one maintenance cycle:

```bash
moon watch --once
```

Search and memory:

```bash
moon recall --name history_lib --query "your query"
moon embed --name history_lib --max-docs 25
moon distill --mode norm
moon distill --mode syns
```

## Provenance And Plugin Behavior

`moon install` normalizes plugin provenance and runtime wiring in OpenClaw
config, including:

1. `plugins.installs.moon.source|sourcePath|installPath`
2. `plugins.slots.contextEngine = "moon"`
3. `plugins.entries.moon.config.moonPath|moonHome|memoryDir|memoryFile`
4. plugin fallback keys (`fallbackMode`, `compactFallbackOnSkip`)
5. token/character defaults (`maxTokens`, `maxChars`, `maxRetainedBytes`,
   tool read limits)

`moon verify --strict` treats `openclaw plugins list --json` diagnostics as
authoritative.

## macOS Autostart Notes

With installed binary mode, `moon install` sets up launchd watcher autostart.

1. LaunchAgent label: `com.moon.watch`
2. WorkingDirectory is set to `MOON_HOME`
3. `moon restart` is the safe way to refresh running daemon state

`moon install` also ensures `MOON_HOME` export exists in `~/.zprofile` when
missing.

## Troubleshooting (Quick)

1. `required env file missing ... $MOON_HOME/.env`
   - Ensure `MOON_HOME` is what you expect.
   - Ensure `$MOON_HOME/.env` exists and is readable.
2. `moon status` reports stale daemon lock
   - Run `moon restart`.
   - If needed: `moon stop` then `moon restart`.
3. Verify/provenance failures
   - Run `moon install` then `moon verify --strict`.
4. qmd/embed issues
   - Confirm `QMD_BIN` is valid and executable.
   - Run `moon embed --name history_lib --max-docs 25`.

Safe recovery sequence:

```bash
moon install
moon verify --strict
moon status
moon health
moon watch --once
```

## Skills

Moon ships two role-scoped skill files:

1. `SKILL.md` (admin/operator)
2. `SKILL_SUBAGENT.md` (sub-agent least privilege)

Install target in Codex runtime:

1. `$CODEX_HOME/skills/moon-admin/SKILL.md`
2. `$CODEX_HOME/skills/moon-subagent/SKILL.md`

## Repository Map

1. `src/cli.rs`: CLI parse + dispatch
2. `src/commands/*.rs`: top-level command handlers
3. `src/moon/*.rs`: runtime subsystems (context, watch, distill, embed, state)
4. `src/openclaw/*.rs`: OpenClaw integration (config/plugin/gateway)
5. `assets/plugin/*`: plugin package assets
6. `tests/*.rs`: regression tests
7. `docs/*`: runbook/contracts/failure/security docs

## Additional Docs

1. [docs/runbook.md](./docs/runbook.md)
2. [docs/contracts.md](./docs/contracts.md)
3. [docs/failure_policy.md](./docs/failure_policy.md)
4. [docs/security_checklist.md](./docs/security_checklist.md)
5. [CHANGELOG.md](./CHANGELOG.md)
6. [RELEASE.md](./RELEASE.md)
7. [SUPPORT.md](./SUPPORT.md)
8. [GOVERNANCE.md](./GOVERNANCE.md)

## Uninstall (Quick)

Default uninstall removes Moon service/plugin/runtime artifacts and preserves
memory by default.

Preserved by default:

1. `$MOON_HOME/memory/`
2. `$MOON_HOME/MEMORY.md`
3. `$MOON_HOME/.env`
4. `$MOON_HOME/moon.toml`

```bash
set -euo pipefail

MOON_HOME="${MOON_HOME:-$HOME/.moon}"
OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR:-$HOME/.openclaw}"
OPENCLAW_CONFIG_PATH="${OPENCLAW_CONFIG_PATH:-$OPENCLAW_STATE_DIR/openclaw.json}"
LAUNCHD_LABEL="com.moon.watch"
LAUNCHD_PLIST="$HOME/Library/LaunchAgents/$LAUNCHD_LABEL.plist"

# Stop moon daemon if available
moon stop 2>/dev/null || true

# macOS launchd cleanup (safe no-op on non-macOS)
if command -v launchctl >/dev/null 2>&1; then
  launchctl bootout "gui/$(id -u)/$LAUNCHD_LABEL" 2>/dev/null || true
  launchctl bootout "gui/$(id -u)" "$LAUNCHD_PLIST" 2>/dev/null || true
  rm -f "$LAUNCHD_PLIST"
fi

# Remove OpenClaw plugin registration + extension payload
if command -v openclaw >/dev/null 2>&1; then
  openclaw plugins uninstall moon 2>/dev/null || true
fi
rm -rf "$OPENCLAW_STATE_DIR/extensions/moon"

# Remove moon-managed runtime artifacts (preserve memory + env/config)
rm -rf \
  "$MOON_HOME/raw" \
  "$MOON_HOME/mds" \
  "$MOON_HOME/mlib" \
  "$MOON_HOME/cleanse" \
  "$MOON_HOME/mce" \
  "$MOON_HOME/logs" \
  "$MOON_HOME/state" \
  "$MOON_HOME/qmd" \
  "$MOON_HOME/.env.example" \
  "$MOON_HOME/moon.toml.example" \
  "$MOON_HOME/docs"
```

Optional full wipe (including memory and local runtime config):

```bash
rm -rf "${MOON_HOME:-$HOME/.moon}"
```

Optional shell-profile cleanup:

1. `moon install` may have added `# Moon runtime home` and `export MOON_HOME=...`
   into `~/.zprofile`.
2. Remove that block manually if you no longer want Moon in shell startup.

Moon does not automatically revert existing OpenClaw config keys under
`plugins.entries.moon`, `plugins.installs.moon`, or `plugins.slots.contextEngine`.
If you need full rollback, edit `$OPENCLAW_CONFIG_PATH` manually.
