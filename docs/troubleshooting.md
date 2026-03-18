# M.O.O.N. Troubleshooting Guide

This guide is written for AI agents and operators fixing Moon/OpenClaw runtime
failures.

For first-time setup before installation, use repo `BOOTSTRAP.md`. This file is
for installed-runtime troubleshooting after `moon install`.

## 1. OpenClaw Cannot See Moon Memory Paths

Symptoms:

1. `moon status` or `moon verify --strict` reports missing
   `plugins.entries.moon.config.memoryDir`
2. `moon status` or `moon verify --strict` reports missing
   `plugins.entries.moon.config.memoryFile`

Fix:

1. Run `moon install`
2. Run `moon verify --strict`
3. Confirm these keys in `openclaw.json`:
   - `plugins.entries.moon.config.moonHome`
   - `plugins.entries.moon.config.memoryDir`
   - `plugins.entries.moon.config.memoryFile`

Expected values:

1. `moonHome = $MOON_HOME`
2. `memoryDir = $MOON_HOME/memory`
3. `memoryFile = $MOON_HOME/MEMORY.md`

## 2. Moon Context Engine Fails Before Dispatch

Symptoms:

1. plugin/context-engine call exits non-zero
2. `moon health` reports missing context-engine output

Fix:

1. Confirm `MOON_HOME` is set
2. Confirm `$MOON_HOME/.env` exists and is readable
3. Run `moon config --show`
4. Run `moon status`
5. Run `moon health`
6. Re-run `moon install` if `moonPath`, `moonHome`, `memoryDir`, or `memoryFile`
   drifted in `openclaw.json`

## 3. Search / Embed Maintenance Does Not Advance

Symptoms:

1. `pending_embed_collections` stays non-zero
2. `moon embed` reports missing capability or failed status

Fix:

1. Confirm `QMD_BIN` points to a working `qmd`
2. Run `moon embed --name history_lib --max-docs 25`
3. If hot-cache embedding is affected, run the current checkpoint path again so
   Moon recreates the short-term collection

## 4. Primary Flow Config Drift

Symptoms:

1. `moon status` reports context policy drift
2. `cleanse` triggers at the wrong token budget

Fix:

1. Open `$MOON_HOME/moon.toml`
2. Check `[context] window_tokens`
3. Check `[context] cleanse_trigger_ratio`
4. Check `[context] cleanse_emergency_ratio`
5. Run `moon config --show` to verify the resolved values

## 5. Minimum Recovery Sequence

If the runtime looks inconsistent and the root cause is unclear:

1. `moon install`
2. `moon verify --strict`
3. `moon status`
4. `moon health`
5. `moon watch --once`

## 6. Clean Restart / Lock Cleanup

Symptoms:

1. `launchd.stderr.log` accumulates repeated runtime errors
2. `moon health` reports daemon drift or stale lock symptoms
3. watcher/restart behavior looks inconsistent after binary upgrades

Fix:

1. `launchctl bootout "gui/$(id -u)" "$HOME/Library/LaunchAgents/com.moon.watch.plist" 2>/dev/null || true`
2. `pkill -f "moon watch --daemon" 2>/dev/null || true`
3. `pkill -f "moon restart" 2>/dev/null || true`
4. `rm -f "$MOON_HOME/logs/moon-watch.daemon.lock" "$MOON_HOME/logs/moon-embed.lock" "$MOON_HOME/logs/l1-normalisation.lock"`
5. `moon install`
6. `moon verify --strict`
7. `moon health`

Notes:

1. Installed Moon runtime operations should continue to work after the repo
   checkout is deleted.
2. On macOS, `moon install` should anchor launchd `WorkingDirectory` to
   `$MOON_HOME`, not the caller CWD.
