# M.O.O.N.

> **Strategic Memory Augmentation & Context Distillation System**

### <span style="font-family:'Orbitron','Bank Gothic','Eurostile',sans-serif;"><font color="#dd0000">M</font>emory</span>

### <span style="font-family:'Orbitron','Bank Gothic','Eurostile',sans-serif;"><font color="#dd0000">O</font>ptimisation</span>

### <span style="font-family:'Orbitron','Bank Gothic','Eurostile',sans-serif;"><font color="#dd0000">O</font>rganisation</span>

### <span style="font-family:'Orbitron','Bank Gothic','Eurostile',sans-serif;"><font color="#dd0000">N</font>ode</span>

---

## Tactical Overview

**M.O.O.N.** is a high-performance, background-active memory optimiser designed
to enhance AI systems with autonomous memory management. It records active
context, compacts pressure-heavy sessions, projects searchable documents, and
distills durable memory from overwhelming context streams.

Moon v1 centers on `moon-context-engine` as the normal-path controller for
active context preparation. The watcher remains a separate long-running
maintenance worker, and OpenClaw remains a bootstrap shell plus explicit
fallback path rather than the owner of normal-path context decisions.
The repo is ready for real OpenClaw integration testing of the primary flow,
but should still be treated as integration-test ready rather than
production-stable.

## Core Features

1. **Primary MOON Context Control**: MOON uses `moon-context-engine` as the
   normal-path controller, with `record`, `cleanse`, and `assemble` as the
   defining runtime boundaries. `project` is deterministic and lane-based:
   hot lane (`raw -> mds`) for trigger-coupled active-window support and
   library lane (`raw -> mlib`) for watcher maintenance and `distill --mode norm`.
2. **Search And Retrieval**: `moon recall` is the user-facing search command for
   Moon-managed content. `moon embed` is the public search-maintenance command
   that refreshes embeddings for the searchable corpus. Moon v1 does not keep a
   separate public `index` command.
3. **Two-Layer Memory Pipeline**:
   - **L1 Normalisation (`distill -mode norm`)**: deterministic
     filtering/normalisation into daily logs (`memory/YYYY-MM-DD.md`) without
     high-reasoning synthesis.
   - **L2 Synthesis (`distill -mode syns`)**: model-driven synthesis that
     rewrites durable memory from selected source files.
   - **Source control for synthesis**: default is `today + memory.md`; explicit
     `-file` inputs synthesize only those files.
4. **Execution Model**:
   - `moon-context-engine` is short-lived and runs when OpenClaw needs the
     active context window prepared.
   - `record` runs unconditionally at stable checkpoints and journals raw
     session state.
   - `cleanse` runs only when context pressure requires compaction.
   - `assemble` injects the latest cleanse summary into the next active context
     window with compact memory anchors (hot high-attention, library
     low-attention availability).
   - When the native `moon` context-engine plugin owns compaction, same-turn
     overflow recovery appends MOON compaction summaries into the OpenClaw
     session transcript on success; on configured fallback triggers, ownership
     hands back to OpenClaw compaction.
   - At cleanse trigger, `project --lane hot` runs first, marks hot embed
     pending, then `mce` runs immediate hot `embed` and `cleanse` in the same
     checkpoint window.
   - Watcher runs maintenance on raw deltas:
     `project --lane library -> embed(history_lib) -> distill --mode norm`.
   - `mce` and watcher run in parallel. `mce` hot-lane project/embed is
     immediate and is not governed by watcher cooldown or cycle timing.
5. **Admin And Bootstrap Shell**:
   - `install`, `verify`, `repair`, `status`, `config`, and `health` remain as
     admin/bootstrap commands around the evolving `moon-context-engine` runtime.
   - `watch`, `stop`, and `restart` remain transitional commands while the
     watcher is still part of the migration shell.

## Recommended Agent Integration

To ensure reliable long-term memory and optimal token hygiene, it is recommended
to explicitly define the boundary between the **M.O.O.N.** (automated) and the
**Agent** (strategic) within your workspace rules (e.g., `AGENTS.md`):

- **M.O.O.N. (Automated Lifecycle)**: Handles record/project flow, token
  compaction via `cleanse`, L1 Normalisation to daily memory, and L2 Synthesis
  to `memory.md`.
- **Agent (Strategic Review)**: Audits memory quality, adjusts prompts/rules,
  and curates long-term memory direction.

This modular architecture prevents the Agent from being overwhelmed by raw
session data while ensuring that distilled knowledge is persisted with high
signal-to-noise ratios.

### Skill Placement (Admin vs Sub-agent)

Keep both skill source files in this repo root:

1. `SKILL.md` for admin/operator tasks (`install`, `verify`, `repair`, `status`,
   `config`, `health`, and Moon runtime operations).
2. `SKILL_SUBAGENT.md` for least-privilege sub-agent tasks (`recall`, `distill`,
   bounded `embed`).

For first-time setup from source, read repo [`BOOTSTRAP.md`](./BOOTSTRAP.md)
before running `moon install`.

After `moon install`, Moon exports installed-runtime operator docs into
`$MOON_HOME` (`README.md`, `.env.example`, `moon.toml.example`,
`docs/troubleshooting.md`) and exports role-scoped skills into the OpenClaw
skills tree under `$OPENCLAW_STATE_DIR/skills/`.

If you are running from source before install, or you want to copy the skills
manually into another runtime, use:

```bash
MOON_REPO="/absolute/path/to/moon"
SKILLS_HOME="${CODEX_HOME:-$HOME/.codex}/skills"

mkdir -p "$SKILLS_HOME/moon-admin" "$SKILLS_HOME/moon-subagent"
cp "$MOON_REPO/SKILL.md" "$SKILLS_HOME/moon-admin/SKILL.md"
cp "$MOON_REPO/SKILL_SUBAGENT.md" "$SKILLS_HOME/moon-subagent/SKILL.md"
```

Recommended role split:

1. Primary/operator agent: `moon-admin`.
2. Sub-agents: `moon-subagent` only.

### AGENTS.md Recall Policy Template

Add this block to your workspace `AGENTS.md` (adjust the repo path if
different):

```md
### moon History Recall Policy (Required)

1. Library history search backend is QMD collection `history_lib` over Moon
   projected library documents (`$MOON_HOME/mlib/*.md`).
2. Default history retrieval command is
   `moon recall --name history_lib --query "<user-intent-query>"`. (If running from
   source instead of a compiled binary, use
   `cargo run --manifest-path /path/to/moon/Cargo.toml -- recall --name history_lib --query "<user-intent-query>"`).
3. For same-session pre-cleanse recall, use hot collection
   `history_hot_<session_id>` (or fallback `history_hot`) when needed. The
   matching hot projection lives under `$MOON_HOME/mds/<collection>/`.
4. Run history retrieval before answering when any condition is true: user
   references past sessions, pre-compaction context, prior decisions, or
   current-session context is insufficient.
5. Retrieval procedure is strict: run one primary query, run one fallback query
   if no hits, and use top 3 hits only; include source/path metadata in
   reasoning when available.
6. If finer detail is required, fetch only the minimal raw or markdown source
   segment needed from the Moon-managed corpus.
7. If both primary and fallback queries return no relevant hit, explicitly reply
   `HISTORY_NOT_FOUND` (cannot find in Moon-managed history).
8. Never fabricate prior-session facts when `recall` returns no relevant match.
```

Query semantics:

1. Primary query: direct user intent in natural language.
2. Fallback query: broader keywords from the same intent when primary has no
   relevant match.
3. Top 3 hits: highest-score results returned by `recall`.

## Agent bootstrap checklist

1. Set runtime `.env` at `$MOON_HOME/.env` (at minimum: ensure `openclaw` is on
   `PATH`; optional: set `OPENCLAW_BIN`; recommended: explicit path block
   below).
2. Apply plugin install + provenance self-heal: `moon install` (or
   `cargo run -- install`)
   - This also provisions the MOON runtime root directories under `$MOON_HOME`.
   - It exports installed-runtime docs into `$MOON_HOME` (`README.md`,
     `.env.example`, `moon.toml.example`) and troubleshooting docs into
     `$MOON_HOME/docs/`.
   - Repo `BOOTSTRAP.md` remains the pre-install guide and is not copied into
     `$MOON_HOME`.
   - It exports `SKILL.md` and `SKILL_SUBAGENT.md` into the OpenClaw skills tree
     under `$OPENCLAW_STATE_DIR/skills/`.
   - It selects `plugins.slots.contextEngine = "moon"` and writes the managed
     `moonPath` / `moonHome` plugin config needed for native MCE handoff.
   - On macOS (installed binary), this also enables a `launchd` watcher
     maintenance service with auto-start + auto-restart.
   - On Windows/Linux, autostart wiring is skipped; run `moon restart` (or
     `moon watch --daemon`) manually.
3. Validate environment and plugin wiring: `moon verify --strict` (or
   `cargo run -- verify --strict`)
4. Check moon runtime paths: `moon status` (or `cargo run -- status`)
5. Check daemon/state health: `moon health` (or `cargo run -- health`)
6. Inspect resolved runtime config: `moon config --show` (or
   `cargo run -- config --show`)
7. Run the current transitional maintenance watcher cycle when needed:
   `moon watch --once` (or `cargo run -- watch --once`)
8. On macOS, `moon install` already wires transitional watcher auto-start via
   `launchd`; use `moon restart` after config/binary updates when the
   maintenance worker is still in use.
9. Install role-scoped skills (`moon-admin`, `moon-subagent`) if your runtime
   uses `$CODEX_HOME/skills`.

## Quick start

```bash
export MOON_HOME="${MOON_HOME:-$HOME/.moon}"
mkdir -p "$MOON_HOME"
cp .env.example "$MOON_HOME/.env"
cp moon.toml.example "$MOON_HOME/moon.toml"
$EDITOR "$MOON_HOME/.env"
$EDITOR "$MOON_HOME/moon.toml"
cargo install --path .
moon install
moon verify --strict
moon status
moon health
moon config --show
```

Recommended first real OpenClaw smoke test:

```bash
moon install
moon status
moon health
moon config --show
moon watch --once
```

Then verify:

1. OpenClaw config contains `moonHome`, `memoryDir`, and `memoryFile` under
   `plugins.entries.moon.config`.
2. A real OpenClaw turn triggers `moon-context-engine` and writes to
   `$MOON_HOME/raw` and `$MOON_HOME/mce`.
3. When pressure crosses the configured trigger, hot-lane artifacts also appear
   under `$MOON_HOME/mds` and `$MOON_HOME/cleanse`.
4. Watcher maintenance writes library projections to `$MOON_HOME/mlib` and
   daily memory to `$MOON_HOME/memory/YYYY-MM-DD.md`.

`.env.example` and `moon.toml.example` are templates. Keep them generic; put
machine-specific values in `$MOON_HOME/.env` and `$MOON_HOME/moon.toml`
only.

Bootstrap/install document split:

1. Repo `BOOTSTRAP.md` is the first-time install guide to read before
   installation.
2. Installed `$MOON_HOME/README.md` is the runtime/operator guide after
   installation.
3. `moon install` does not copy `BOOTSTRAP.md` into `$MOON_HOME`.

Workspace model (agent-facing):

1. `MOON_HOME` is the moon runtime root.
2. When `MOON_HOME` is unset, moon defaults to `$HOME/.moon`.
3. Recommended explicit setting: `MOON_HOME=$HOME/.moon` (or another dedicated
   runtime root).
4. Repo path is separate from `MOON_HOME`; do not assume the repo lives inside
   the runtime root.
5. Daily memory path is `$MOON_HOME/memory/YYYY-MM-DD.md`.

`.env` loading is strict:

1. Moon only loads environment from `$MOON_HOME/.env`.
2. `MOON_HOME` must be set and non-empty.
3. If `$MOON_HOME/.env` is missing or unreadable, moon exits with an error.

Agent check: always export `MOON_HOME` and ensure `$MOON_HOME/.env` exists
before running any moon command.

Workspace boundary safety:

1. Mutating commands validate CWD against the daemon-recorded workspace (or
   explicit `MOON_HOME` when no daemon lock is present).
2. Diagnostic commands (`status`, `health`, `verify`, `config`) are always
   allowed from any directory.
3. Escape hatch: pass global `--allow-out-of-bounds` to bypass CWD enforcement.
4. Env-gated bypass: set `MOON_ALLOW_OUT_OF_BOUNDS=1` to enable the same bypass
   by default for the current process environment.

OpenClaw binary resolution:

```bash
# Preferred: ensure `openclaw` is available on PATH.
# Optional override: pin an explicit binary path.
OPENCLAW_BIN=/absolute/path/to/openclaw
```

Runtime-root profile (recommended):

```bash
# Binaries
# QMD is an external dependency (separate repo/project). moon only calls its CLI.
# Set this to your real qmd path (for macOS/Homebrew commonly /opt/homebrew/bin/qmd).
QMD_BIN=/opt/homebrew/bin/qmd

# moon runtime paths
MOON_HOME=$HOME/.moon
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

# OpenClaw session source
OPENCLAW_STATE_DIR=$HOME/.openclaw
OPENCLAW_CONFIG_PATH=$OPENCLAW_STATE_DIR/openclaw.json
OPENCLAW_SESSIONS_DIR=$HOME/.openclaw/agents/main/sessions
```

`moon.toml` is optional. If `MOON_CONFIG_PATH` points to a missing file, moon
continues with built-in defaults plus `.env` overrides.

`moon.toml` resolve order:

1. `MOON_CONFIG_PATH` (exact file path)
2. `$MOON_HOME/moon.toml`
3. default fallback when `MOON_HOME` is unset: `$HOME/.moon/moon.toml`

State path override precedence:

1. `MOON_STATE_FILE` (exact file path)
2. `MOON_STATE_DIR` (directory; file becomes `moon_state.json`)
3. fallback: `$MOON_HOME/state/moon_state.json`

Recommended split:

1. `.env`: paths, binaries, provider/model/API keys, and env-only runtime knobs.
2. `moon.toml`: tuning in `[context]`, `[watcher]`, `[distill]`, `[embed]`,
   `[hot_collection]` (and optional `[thresholds]`).

If the same tuning key appears in both places, `.env` wins.

Create a local config file:

```bash
cp moon.toml.example "$MOON_HOME/moon.toml"
```

Context policy (optional but recommended when moon owns compaction):

```toml
[context]
window_mode = "fixed"
window_tokens = 200000
compaction_authority = "moon"      # "moon" or "openclaw"
cleanse_trigger_ratio = 0.50
cleanse_emergency_ratio = 0.90
```

With the example values above:

1. `cleanse_trigger_tokens = 100000`
2. `cleanse_emergency_tokens = 180000`

When `compaction_authority = "moon"`:

1. MOON owns the decision to compact the active context in the target
   architecture.
2. `record` remains unconditional; `cleanse` is the conditional pressure-relief
   step.
3. Current ratio-based knobs remain transitional compatibility controls while
   automatic `moon-context-engine` triggering is being completed.
4. OpenClaw compaction behavior should remain a fallback path only.
5. `moon status` should report policy drift when fallback shell config conflicts
   with MOON-owned compaction policy.

Current config rules:

1. Do not use `prune_mode`; it has been removed.
2. Do not use `[retention]`; it is suspended for the current stage.
3. Do not use `[inbound_watch]`; it is suspended for the current stage.
4. Do not use `MOON_HOT_COLLECTION_LIFECYCLE_MODE` or
   `MOON_HOT_COLLECTION_LIFECYCLE_COMMAND_MODE`; hot lifecycle policy belongs in
   `$MOON_HOME/moon.toml`.

Synthesis model profile (recommended for the agent):

```bash
# `norm` uses no LLM.
# `project` uses no LLM.
# `cleanse` uses its own compaction model.
# `syns` uses a separate higher-reasoning model.
MOON_CLEANSE_MODEL=gemini-3.1-flash-lite-preview
GEMINI_API_KEY=...

# Recommend a high-reasoning model for better durable memory quality.
MOON_WISDOM_PROVIDER=openai
MOON_WISDOM_MODEL=gpt-4.1
OPENAI_API_KEY=...

# Alternative high-reasoning options:
# MOON_WISDOM_PROVIDER=anthropic
# MOON_WISDOM_MODEL=claude-3-7-sonnet-latest
#
# MOON_WISDOM_PROVIDER=gemini
# MOON_WISDOM_MODEL=gemini-2.5-pro
```

Distill safety guardrails (recommended):

```toml
[context]
window_mode = "fixed"
window_tokens = 200000
compaction_authority = "moon"
cleanse_trigger_ratio = 0.50
cleanse_emergency_ratio = 0.90

[watcher]
poll_interval_secs = 30
cooldown_secs = 30

[distill]
max_per_cycle = 3
residential_timezone = "UTC"
topic_discovery = true
# Optional L1 chunk planning controls:
# chunk_bytes = "auto"
# max_chunks = 128
# model_context_tokens = 200000

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

Optional env overrides (keep these in `.env` only when needed):

```bash
# Optional explicit context-window hints for `syns` large-file chunk planning.
# If unset, moon auto-detects/infers context window per provider/model.
# MOON_WISDOM_CONTEXT_TOKENS=200000
```

Cheapest possible mode (zero API cost, local-only synthesis):

```bash
MOON_WISDOM_PROVIDER=local
```

Run a few basics (assuming `moon` is installed in `$PATH`, otherwise prefix with
`cargo run --`):

```bash
moon status
moon install --dry-run
moon install
moon verify --strict
moon status
```

## CLI

Binary name: `moon`

This section describes the intended Moon v1 CLI surface. During migration, the
compiled binary may temporarily lag behind the target command set documented
here.

It is strongly recommended to install the binary to your `$PATH` using
`cargo install --path .` rather than relying on `cargo run -- <command>` in
production scenarios. You only need to run `cargo install --path .` again if you
modify the Rust source code or plugin assets.

### Binary Rebuild Guide

Use this when you changed Rust code or plugin assets and want the installed
`moon` binary to pick up changes.

1. Rebuild and reinstall the binary.
2. Re-apply plugin/runtime wiring.
3. Verify strict health/provenance checks.
4. Restart watcher daemon if it is running.

```bash
cargo install --path . --force
moon install
moon verify --strict
moon restart
```

```bash
moon <command> [flags]
```

Global flag:

1. `--json` outputs machine-readable `CommandReport`
2. `--allow-out-of-bounds` bypasses workspace CWD lock checks for mutating
   commands
3. `MOON_ALLOW_OUT_OF_BOUNDS=1` enables the same bypass as an environment
   default (truthy values: `1`, `true`, `yes`, `on`)

Commands:

1. `install [--force] [--dry-run] [--apply true|false]`
   - wires the current MOON bootstrap shell, provenance state, and runtime-root
     directories
   - macOS default behavior: writes/refreshes
     `~/Library/LaunchAgents/com.moon.watch.plist`, then bootstraps and
     kickstarts the transitional maintenance watcher service.
   - Windows/Linux behavior: service autostart wiring is not managed by
     `moon install` yet.
   - Safety guard: when running from development binaries (`target/debug` or
     `target/release`), autostart setup is skipped and a hint is printed.
2. `verify [--strict]`
   - verifies runtime shell wiring, provenance, dependencies, and health
3. `repair [--force]`
   - repairs runtime shell and provenance drift
4. `status`
   - reports resolved runtime paths, dependency visibility, and runtime shell
     state
5. `record`
   - captures active context into Moon-owned raw state under `$MOON_HOME/raw/`
6. `project`
   - converts raw session documents into projection markdown with explicit lanes:
     hot lane writes to `$MOON_HOME/mds/<collection>/`; library lane writes to
     `$MOON_HOME/mlib/`
7. `cleanse`
   - runs true LLM-backed context compaction and writes recovery summaries under
     `$MOON_HOME/cleanse/`
8. `assemble`
   - composes the next MOON-owned dispatch context and writes it under
     `$MOON_HOME/mce/`
9. `context-engine [--source <path>] [--session-id <id>] [--used-tokens <N>] [--max-tokens <N>] [--force-cleanse]`
   - runs the primary checkpoint flow: `record`, conditional `cleanse`, and
     `assemble`
10. `distill -mode <norm|syns> [-archive <path>] [-session-id <id>] [-file <path> ...] [-dry-run]`

- `-mode norm` (default): L1 Normalisation into daily memory
- `-mode syns`: L2 Synthesis rewrites durable memory

11. `recall --query <text> [--name <collection>]`

- searches Moon-managed content
- default collection is `history_lib`

12. `embed [--name <collection>] [--max-docs <N>] [--dry-run] [--watcher-trigger]`

- refreshes search embeddings for Moon-managed documents
- default collection is `history_lib`; hot collections must be named explicitly
- on qmd builds without collection-bounded embed, Moon uses global
  `qmd embed --max-docs-per-batch <n>` against Moon-owned qmd state under
  `$MOON_HOME/qmd/`

13. `config [--show]`
14. `health`
15. `watch [--once|--daemon] [--dry-run]`

- long-running maintenance only; not the active-window controller

16. `stop`

- transitional only

17. `restart`

- transitional only

Exit codes:

1. `0` command completed with `ok=true`
2. `2` command completed with `ok=false`
3. `1` runtime/process error

## Provenance Behavior (Agent-critical)

1. `moon install` always normalizes `plugins.installs.moon` (`source`,
   `sourcePath`, `installPath`) to the managed plugin directory.
2. `moon verify --strict` treats OpenClaw runtime diagnostics from
   `openclaw plugins list --json` as the authoritative provenance signal.
3. If runtime diagnostics report `loaded without install/load-path provenance`,
   `verify --strict` fails hard.
4. If `plugins.installs.moon` is missing or path-mismatched but runtime
   diagnostics are clean, `verify` prints a non-fatal `provenance repair hint`.
5. First-time bootstrap and upgrade routine should always include `moon install`
   before `moon verify --strict`.

### Local Development & Testing

If you are actively developing the moon codebase or writing an AI agent that
needs to run tests:

Running the background watcher daemon (`watch --daemon`) via `cargo run` is
explicitly blocked. This is a safety feature to prevent file-locking starvation
and CPU spikes loop issues if the daemon restarts.

Watcher daemon startup is single-instance per `MOON_HOME`: if another watcher
already holds the daemon lock, a second `watch --daemon` exits with
`moon watcher daemon already running pid=...`.

To test the daemon with unreleased local changes, you must compile the binary
first and execute it directly:

```bash
cargo build
./target/debug/moon watch --daemon
```

## Common workflows

After OpenClaw upgrade:

```bash
moon install
moon verify --strict
```

If you upgraded from older builds, clean legacy macOS LaunchAgents to avoid
conflicting watcher services or stale `/tmp/moon*system*.log` logs:

```bash
launchctl list | rg -i "moon|moon.*system" || true
ls "$HOME/Library/LaunchAgents" | rg -i "com\\.moon\\.(watch|agent)|moon.*system" || true
```

Run manual embed sprint:

```bash
moon embed --name history_lib --max-docs 25
```

Recall prior context:

```bash
moon recall --name history_lib --query "your query"
```

Manual Moon v1 active-window control:

```bash
moon context-engine --used-tokens 65000 --max-tokens 200000
```

Primary-flow contracts when you need to run stages explicitly:

```bash
moon record
moon cleanse
moon assemble
moon project
moon embed --name history_lib --max-docs 25
moon distill -mode norm
moon distill -mode syns
```

Execution notes:

1. `moon context-engine` is the normal-path short-lived controller for active
   context preparation.
2. `record` is the stable-checkpoint step and should run even when compaction is
   not needed.
3. `cleanse` is conditional and should run only when context pressure requires
   compaction.
4. `assemble` is the pre-dispatch context boundary.
5. `project` and `embed` are background/deferred work derived from recorded raw
   state.
6. `distill -mode norm` continues to consume projected markdown from
   `$MOON_HOME/mlib/`.

Run one watcher cycle:

```bash
moon watch --once
```

`watch` remains a separate long-running maintenance worker. It is transitional
infrastructure and not the active-window controller.

Dry-run watcher planning cycle (no mutation/state writes):

```bash
moon watch --once --dry-run
```

Stop the watcher daemon:

```bash
moon stop
```

Health probe:

```bash
moon health
```

Troubleshooting & Maintenance:

If the daemon/runtime looks inconsistent, use this clean recovery sequence:

1. `moon stop`
2. `launchctl bootout "gui/$(id -u)" "$HOME/Library/LaunchAgents/com.moon.watch.plist" 2>/dev/null || true`
3. `pkill -f "moon watch --daemon" 2>/dev/null || true`
4. `pkill -f "moon restart" 2>/dev/null || true`
5. `rm -f "$MOON_HOME/logs/moon-embed.lock" "$MOON_HOME/logs/l1-normalisation.lock"`
6. If `moon health` still reports a stale daemon lock, run `rm -f "$MOON_HOME/logs/moon-watch.daemon.lock"`
7. `moon install`
8. `moon verify --strict`
9. `moon health`

Important:

1. Installed Moon runtime operations should not depend on the repo checkout.
2. On macOS, `moon install` now writes launchd autostart with
   `WorkingDirectory=$MOON_HOME` rather than the caller CWD.

L1 auto trigger behavior:

1. Watcher L1 path is auto: `watch` checks L1 every cycle.
2. Cooldown must pass (`watcher.cooldown_secs`).
3. Pending source must exist in `$MOON_HOME/mds/<collection>/*.md` (projection
   markdown only).
4. Selection is deterministic and bounded by `distill.max_per_cycle`.
5. L1 runs under a non-blocking lock; if busy, watcher degrades/skips and
   retries next cycle.

Daily `syns` schedule:

1. Watcher attempts `syns` once per residential day
   (`distill.residential_timezone`) on the first cycle after local midnight.
2. Auto `syns` sources are yesterday's daily file (`memory/YYYY-MM-DD.md`) plus
   current `memory.md` (when present).
3. Agents can run `moon distill -mode syns` directly at any time.
4. `moon watch --once` remains the manual trigger for one immediate L1 queue
   processing cycle.

Retention lifecycle windows:

1. Active (`<= active_days`): keep recorded session artifacts available for fast
   debug/resume.
2. Warm (`active_days < age <= warm_days`): retained and indexed.
3. Cold candidate (`>= cold_days`): deleted only when a distill marker exists.

Embed lifecycle windows:

1. Watcher embed mode is fixed to `auto`.
2. Watcher attempts embed after compaction/L1 stages and before daily `syns`
   when `syns` is due.
3. Watcher execution is gated by `embed.cooldown_secs` and
   `embed.min_pending_docs`.
4. Manual `embed` runs immediately and bypasses watcher cooldown gating.
5. Manual `embed` does not reset the watcher cooldown clock.
6. If qmd supports bounded embed (`--max-docs`), Moon uses collection-scoped
   embed. Otherwise, if qmd supports only global embed, Moon uses
   `qmd embed --max-docs-per-batch <n>` against Moon-owned qmd state; if embed
   capability is missing entirely, watcher degrades and manual embed returns
   capability-missing.
7. There is no idle gate for embed execution.
8. Lock behavior is non-blocking: watcher embed skips current cycle when lock is
   busy; manual embed returns lock error (no wait queue).

Runtime layout:

1. `$MOON_HOME/raw/*.jsonl`: raw snapshot copy (full fidelity).
2. `$MOON_HOME/mds/<collection>/*.md`: deterministic hot-session projection
   markdown.
3. `$MOON_HOME/cleanse/*.md`: LLM compaction summaries.

## Configuration

`.env` loading is strict. Moon only loads runtime env from `$MOON_HOME/.env`.
If `$MOON_HOME` is unset/empty or `$MOON_HOME/.env` is missing, moon exits
with an error.

Start from:

1. `.env.example`
2. `moon.toml.example`

Recommended location:

1. `$MOON_HOME/.env`

Most-used `.env` variables:

1. `OPENCLAW_BIN` (optional override; `openclaw` is auto-resolved from `PATH`
   when unset)
2. `QMD_BIN`
3. `MOON_HOME`
4. `MOON_CONFIG_PATH`
5. `MOON_STATE_FILE` / `MOON_STATE_DIR`
6. `OPENCLAW_SESSIONS_DIR`
7. `MOON_WISDOM_PROVIDER` (primary provider selector for `distill -mode syns`)
8. `MOON_WISDOM_MODEL` (primary model selector for `syns`)
9. `MOON_WISDOM_CONTEXT_TOKENS` (optional context-window hint for large-file
   chunk planning in `syns`)
10. `GEMINI_API_KEY` / `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` / `AI_API_KEY`
    (for `syns`)
11. `MOON_ENABLE_COMPACTION_WRITE`
12. `MOON_ENABLE_SESSION_ROLLOVER`
13. `MOON_EMBED_MODE` (`auto` only)
14. `MOON_EMBED_COOLDOWN_SECS`
15. `MOON_EMBED_MAX_DOCS_PER_CYCLE`
16. `MOON_EMBED_MIN_PENDING_DOCS`
17. `MOON_EMBED_MAX_CYCLE_SECS`
18. `MOON_HEALTH_MAX_CYCLE_AGE_SECS` (health freshness threshold; default `600`)

Most-used plugin runtime config keys (`plugins.entries.moon.config` in OpenClaw config):

1. `moonPath`
2. `moonHome`
3. `memoryDir`
4. `memoryFile`
5. `contextEngineTimeoutMs`
6. `maxAssemblyChars`
7. `syncAfterTurn`
8. `fallbackMode` (`openclaw` or `disabled`)
9. `compactFallbackOnSkip` (`true` or `false`)

Config hardening behaviors:

1. Unknown `MOON_*` variables are warned on startup, with typo suggestions when
   close matches exist (allowlist is generated from source at build time).
2. `moon config --show` prints fully resolved config values (defaults ->
   `moon.toml` -> env overrides).
3. Secret env values are masked in diagnostics (`status`, `config --show`).

Primary tuning belongs in `moon.toml`:

1. `[context] window_mode`, `window_tokens`, `compaction_authority`, `cleanse_trigger_ratio`,
   `cleanse_emergency_ratio`
2. `[watcher] poll_interval_secs`, `cooldown_secs`
3. `[distill] max_per_cycle`, `residential_timezone`, `topic_discovery`,
   `chunk_bytes`, `max_chunks`, `model_context_tokens`
4. `[embed] mode` (`auto`), `cooldown_secs`, `max_docs_per_cycle`,
   `min_pending_docs`, `max_cycle_secs`
5. `[hot_collection] lifecycle_mode`, `lifecycle_command_mode`
6. `[thresholds] trigger_ratio` (fallback path when context policy is not active)

## Repository map

1. `src/cli.rs`: argument parsing + command dispatch
2. `src/commands/*.rs`: top-level command handlers
3. `src/openclaw/*.rs`: OpenClaw config/plugin/gateway operations
4. `src/moon/*.rs`: recall/distill/embed/watch logic and Moon runtime subsystems
   - `src/moon/util.rs`: shared utilities (`now_epoch_secs`,
     `truncate_with_ellipsis`)
5. `assets/plugin/*`: plugin files embedded and installed by `install`
6. `tests/*.rs`: regression tests
7. `docs/*`: deeper operational docs
8. `audit_report.md`: latest code audit findings and fixes

## Detailed docs

1. `docs/runbook.md`
2. `docs/contracts.md`
3. `docs/failure_policy.md`
4. `docs/security_checklist.md`
5. `CHANGELOG.md`
6. `RELEASE.md`
7. `SUPPORT.md`
8. `GOVERNANCE.md`

## Uninstall (quick)

This removes moon services/plugin/runtime files and keeps user assets intact.

User assets that are preserved:

1. `$MOON_MEMORY_DIR` (daily memory)
2. `$MOON_MEMORY_FILE` (long-term memory)

Use trash-first cleanup (preferred):

```bash
trash_path() {
  [ -e "$1" ] || return 0
  if command -v trash >/dev/null 2>&1; then
    trash "$1"
  elif command -v gio >/dev/null 2>&1; then
    gio trash "$1"
  else
    mkdir -p "$HOME/.Trash"
    mv "$1" "$HOME/.Trash/"
  fi
}

# Stop/unload known moon service names
LAUNCHD_DOMAIN="gui/$(id -u)"
LAUNCHD_MOON_WATCH_LABEL="com.moon.watch"
LAUNCHD_MOON_WATCH_PLIST="$HOME/Library/LaunchAgents/$LAUNCHD_MOON_WATCH_LABEL.plist"
LAUNCHD_MOON_LEGACY_PLIST="$HOME/Library/LaunchAgents/com.moon.agent.plist"

launchctl bootout "$LAUNCHD_DOMAIN/$LAUNCHD_MOON_WATCH_LABEL" 2>/dev/null || true
launchctl bootout "$LAUNCHD_DOMAIN" "$LAUNCHD_MOON_WATCH_PLIST" 2>/dev/null || true
launchctl bootout "$LAUNCHD_DOMAIN" "$LAUNCHD_MOON_LEGACY_PLIST" 2>/dev/null || true
systemctl --user stop moon 2>/dev/null || true
systemctl --user disable moon 2>/dev/null || true

trash_path "$LAUNCHD_MOON_WATCH_PLIST"
trash_path "$LAUNCHD_MOON_LEGACY_PLIST"
trash_path "$HOME/.config/systemd/user/moon.service"
systemctl --user daemon-reload 2>/dev/null || true

OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR:-$HOME/.openclaw}"
OPENCLAW_CONFIG_PATH="${OPENCLAW_CONFIG_PATH:-$OPENCLAW_STATE_DIR/openclaw.json}"
openclaw plugins uninstall moon 2>/dev/null || true
trash_path "$OPENCLAW_STATE_DIR/extensions/moon"

MOON_HOME="${MOON_HOME:-$HOME/.moon}"
# Remove moon-owned runtime artifacts only (keep memory/MEMORY.md)
trash_path "$MOON_HOME/continuity"
trash_path "$MOON_HOME/state"
trash_path "$MOON_HOME/logs"
[ -n "${MOON_LOGS_DIR:-}" ] && trash_path "$MOON_LOGS_DIR"
[ -n "${MOON_STATE_FILE:-}" ] && trash_path "$MOON_STATE_FILE"
[ -n "${MOON_STATE_DIR:-}" ] && trash_path "$MOON_STATE_DIR"

# Optional: remove persisted moon config if you created one
trash_path "$MOON_HOME/moon.toml"
```

Note: uninstalling the plugin does not automatically restore custom OpenClaw
config values previously written under `plugins.entries.moon` or
`agents.defaults.*`. Remove or revert those keys manually in
`$OPENCLAW_CONFIG_PATH` (default: `$OPENCLAW_STATE_DIR/openclaw.json`) if you
want a full config rollback.
