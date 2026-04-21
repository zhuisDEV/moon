# M.O.O.N. Runbook

## Bootstrap

Read repo `BOOTSTRAP.md` before first-time installation. This runbook assumes
you are either preparing for install from source or operating an already
installed runtime.

Minimal setup:

```bash
export MOON_HOME="${MOON_HOME:-$HOME/.moon}"
mkdir -p "$MOON_HOME"
cp .env.example "$MOON_HOME/.env"
cp moon.toml.example "$MOON_HOME/moon.toml"
moon install
moon verify --strict
moon status
moon health
moon config --show
```

Admin/bootstrap commands:

1. `moon install`: wire the current MOON runtime shell, provision the
   `$MOON_HOME` runtime root, and write OpenClaw config pointers for `moonHome`,
   `memory/`, and `MEMORY.md`
2. `moon uninstall`: remove Moon-managed OpenClaw wiring and runtime artifacts
   (`--purge` for full Moon home removal)
3. `moon verify --strict`: verify runtime shell wiring, provenance,
   dependencies, and health (concise by default; add `--verbose` for full
   detail)
4. `moon status`: inspect resolved runtime paths and runtime shell state
5. `moon config --show`: inspect resolved config
6. `moon health`: inspect overall runtime health

## Core Flow

Record active context:

```bash
moon record
```

Execution note:

1. `record` is the stable-checkpoint step and should run even when `cleanse`
   does not trigger.

Project raw into MDS:

```bash
moon project
```

Execution note:

1. `project` is deterministic background/deferred work derived from recorded raw
   state.

Apply pressure relief compaction:

```bash
moon cleanse
```

Execution note:

1. `cleanse` is the conditional pressure-relief step.
2. Its summary should feed the next active context window through `assemble`.

Assemble the next active context window:

```bash
moon assemble
```

Execution note:

1. `assemble` is the explicit pre-dispatch boundary.
2. It writes the operator artifact to `$MOON_HOME/mce/`.
3. It also writes the model-facing active context packet to `$MOON_HOME/mcp/`.

Run the integrated checkpoint controller:

```bash
moon context-engine --used-tokens 65000 --max-tokens 200000
```

Execution note:

1. `context-engine` is the short-lived normal-path controller for active context
   preparation.
2. It runs `record` first, triggers `cleanse` only when policy requires it, and
   persists the assembled context window.
3. Native OpenClaw takeover now depends on the plugin slot selecting `moon`
   (`plugins.slots.contextEngine = "moon"`), which `moon install` writes
   automatically.
4. Moon-owned installs also pin the OpenClaw memory contract to
   `plugins.slots.memory = "none"` and
   `agents.defaults.memorySearch.enabled = false`.

Run L1 normalisation:

```bash
moon distill --mode norm
```

Run L2 synthesis:

```bash
moon distill --mode syns
```

Recommended model config:

```bash
MOON_CLEANSE_PROVIDER=gemini
MOON_CLEANSE_MODEL=gemini-3.1-flash-lite-preview
GEMINI_API_KEY=...

MOON_WISDOM_PROVIDER=openai
MOON_WISDOM_MODEL=gpt-4.1
OPENAI_API_KEY=...

# Optional managed OpenAI Codex OAuth lane
# MOON_CLEANSE_PROVIDER=openai-codex
# MOON_CLEANSE_MODEL=gpt-5.4
# MOON_WISDOM_PROVIDER=openai-codex
# MOON_WISDOM_MODEL=gpt-5.4
# moon login
# OPENAI_CODEX_BASE_URL=https://chatgpt.com/backend-api
# Optional manual override instead of `moon login`
# OPENAI_OAUTH_TOKEN=...
```

`moon login` stores managed OpenAI Codex OAuth credentials in
`$MOON_HOME/auth/openai-codex.json`. Moon refreshes that credential
automatically and can also reuse a fresh Codex CLI login from
`~/.codex/auth.json` when no Moon-managed auth store exists.

## Search

Recall prior content:

```bash
moon recall --query "keyword" --name history_lib
```

Refresh search embeddings:

```bash
moon embed --name history_lib --max-docs 25
```

Rules:

1. `recall` is the user-facing search command.
2. `embed` is the public search-maintenance command.
3. Moon v1 does not keep a separate public `index` command.
4. `embed` should remain bounded via `--max-docs`.
5. Only a minimal indexing anchor, if any, should enter the next active context
   window; full embed receipts should stay out of prompt assembly.

## Transitional Watcher

The watcher is the separate long-running maintenance worker. It remains
transitional infrastructure and does not own the active context window.

Run one watcher cycle:

```bash
moon watch --once
```

Scheduling note:

1. Daily watcher SYNS trigger uses local time from
   `distill.syns_trigger_time_local` (`HH:MM`) in `moon.toml`.
2. Timezone for that local trigger is `distill.residential_timezone`.
3. Scheduled watcher SYNS catches up later the same local day if Moon missed the
   exact trigger minute while the daemon was down or unhealthy.
4. Scheduled watcher SYNS always synthesizes the last completed daily-memory
   file (`$MOON_HOME/memory/<previous-local-day>.md`), never the current day's
   file.
5. If the previous-day daily-memory file is missing or empty, scheduled watcher
   SYNS skips instead of falling back to the current day's file.

Dry-run watcher cycle:

```bash
moon watch --once --dry-run
```

Start transitional daemon:

```bash
moon watch --daemon
```

If another watcher already holds the daemon lock for this `MOON_HOME`, start
fails fast with `moon watcher daemon already running pid=...`.

Stop transitional daemon:

```bash
moon stop
```

Restart transitional daemon:

```bash
moon restart
```

## Key Paths

1. Runtime root: `$MOON_HOME`
2. Raw documents: `$MOON_HOME/raw/`
3. Projection markdown: `$MOON_HOME/mds/`
4. Cleanse summaries: `$MOON_HOME/cleanse/`
5. Operator assembly artifacts: `$MOON_HOME/mce/`
6. Active context packets: `$MOON_HOME/mcp/`
7. Daily memory: `$MOON_MEMORY_DIR/YYYY-MM-DD.md`
8. Durable memory: `$MOON_MEMORY_FILE`
9. Logs: `$MOON_LOGS_DIR`

## Troubleshooting

1. If `verify` or `status` reports provenance drift, run `moon install` and then
   `moon verify --strict`.
2. If search maintenance fails, verify `QMD_BIN` and re-run bounded
   `moon embed --max-docs <N>`.
3. If `syns` is unavailable, confirm provider API keys and
   `MOON_WISDOM_PROVIDER` / `MOON_WISDOM_MODEL`.
4. If a mutating command fails with an out-of-bounds error, run from your
   workspace tree or use `--allow-out-of-bounds`.
5. Optional env default: set `MOON_ALLOW_OUT_OF_BOUNDS=1` (truthy: `1`, `true`,
   `yes`, `on`) to apply the same bypass for that process environment.
6. If OpenClaw cannot find Moon memory paths, run `moon install`, then
   `moon verify --strict`, and check `plugins.entries.moon.config.memoryDir` /
   `memoryFile`.
7. If `moon status` reports memory drift, inspect `plugins.slots.memory` and
   `agents.defaults.memorySearch.enabled` in `$OPENCLAW_CONFIG_PATH`.
