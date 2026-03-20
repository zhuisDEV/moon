# BOOTSTRAP.md

First-time Moon install guide.

This file is for operators/agents working from the repo before installation.
It is not exported into `$MOON_HOME` by `moon install`. After installation, use
`$MOON_HOME/README.md` for installed-runtime operations.

## 1. Prepare Runtime Root

MOON runtime env loading is strict from `$MOON_HOME/.env`.
Create `$MOON_HOME/.env` before running any `moon` command.

```bash
export MOON_HOME="${MOON_HOME:-$HOME/.moon}"
MOON_REPO="<path-to-moon-repo>"

mkdir -p "$MOON_HOME"
cp "$MOON_REPO/.env.example" "$MOON_HOME/.env"
cp "$MOON_REPO/moon.toml.example" "$MOON_HOME/moon.toml"
$EDITOR "$MOON_HOME/.env"
$EDITOR "$MOON_HOME/moon.toml"
```

Minimum dependency checks:

```bash
command -v openclaw
command -v qmd
```

Recommended `moon.toml` baseline:

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

Notes:

1. Hot collection lifecycle policy belongs in `moon.toml`, not `.env`.
2. If your real `qmd` only supports fallback lifecycle command shapes, set
   `lifecycle_command_mode = "fallback"`.
3. Current default cleanse thresholds with the example config are:
   `trigger=100000`, `emergency=180000`.
4. Hot session projection folders live directly under `$MOON_HOME/mds/<collection>/`.
5. Moon isolates qmd runtime state under `$MOON_HOME/qmd/` (`index.sqlite` and
   collection config) so it does not mutate your global qmd workspace.

## 2. Install Binary

Use installed binary mode for stable operations.

```bash
cd "$MOON_REPO"
cargo install --path . --force
```

If `moon` is not on `PATH`, use `$HOME/.cargo/bin/moon` explicitly.

## 3. Bootstrap Install Handshake

Run install/verify in this order:

```bash
moon install
moon verify --strict
moon status
moon health
moon config --show
```

`moon install` does the bootstrap handshake:

1. Wires OpenClaw plugin and provenance fields.
2. Sets `plugins.slots.contextEngine = "moon"`.
3. Writes managed plugin runtime config (`moonPath`, `moonHome`,
   `memoryDir`, `memoryFile`, fallback policy).
4. Provisions MOON runtime directories under `$MOON_HOME`.
5. On macOS (installed binary), wires launchd for transitional watcher daemon.
6. Exports installed-runtime operator docs into `$MOON_HOME` (`README.md`,
   `.env.example`, `moon.toml.example`, `docs/troubleshooting.md`).

## 4. Verify Core Runtime Signals

Check these after install:

1. `moon verify --strict` returns `ok=true`.
2. `moon status` reports expected `moon_home`, `qmd_bin`, and plugin slot.
3. `moon status` reports `plugin_config.memoryDir` and `plugin_config.memoryFile`
   under `$MOON_HOME`.
4. `moon health` reports writable state paths and no critical issues.
5. `moon config --show` confirms hot collection lifecycle policy values and
   computed cleanse token thresholds.
6. qmd runtime paths resolve under `$MOON_HOME/qmd/`, not `$HOME/.config/qmd` or a
   shared global qmd index.

## 5. Real OpenClaw Smoke Test

Use this order for the first real integration check:

```bash
moon install
moon status
moon health
moon config --show
```

Then validate the two lanes separately:

1. Hot lane: trigger one real OpenClaw turn and confirm `moon-context-engine`
   writes under `$MOON_HOME/raw`, `$MOON_HOME/mce`, and, when pressure is high
   enough, `$MOON_HOME/mds/<collection>/` and `$MOON_HOME/cleanse`.
2. Library lane: run `moon watch --once` and confirm watcher advances
   `raw -> mlib -> embed(history_lib) -> distill --mode norm`.

Expected ownership split:

1. MCE hot lane is immediate and not gated by watcher cooldown/cycle timing.
2. Watcher is maintenance-only and should not be treated as the active-window
   controller.
3. Fallback is off by default; finish primary-flow validation first.

## 6. First Search/Memory Smoke Tests

```bash
moon record --dry-run
moon project --lane library --dry-run
moon embed --name history_lib --max-docs 25 --dry-run
moon recall --name history_lib --query "bootstrap check"
```

Notes:

1. `recall` is the user-facing search command.
2. `embed` is the public search-maintenance command.
3. There is no separate public `index` command in Moon v1.
4. On qmd builds without bounded collection embed, Moon may use qmd global
   embed internally while still keeping collection/session intent in Moon state.

## 7. Transitional Watcher Operations

Watcher is maintenance-only and not the active-window controller.

```bash
moon watch --once --dry-run
moon watch --once
```

Daemon usage:

1. macOS: `moon install` wires default launchd service.
2. Windows/Linux: use `moon restart` or `moon watch --daemon` manually.
3. Avoid `cargo run -- watch --daemon` for long-running operations.

## 8. Skill Placement (Required)

MOON ships two role-scoped skill files:

1. `SKILL.md` for admin/operator scope.
2. `SKILL_SUBAGENT.md` for least-privilege sub-agent scope.

Install to Codex skills home when needed:

```bash
MOON_REPO="<path-to-moon-repo>"
SKILLS_HOME="${CODEX_HOME:-$HOME/.codex}/skills"

mkdir -p "$SKILLS_HOME/moon-admin" "$SKILLS_HOME/moon-subagent"
cp "$MOON_REPO/SKILL.md" "$SKILLS_HOME/moon-admin/SKILL.md"
cp "$MOON_REPO/SKILL_SUBAGENT.md" "$SKILLS_HOME/moon-subagent/SKILL.md"
```

Sub-agent policy:

1. Primary operator can use `moon-admin`.
2. Sub-agents should use `moon-subagent` only.
