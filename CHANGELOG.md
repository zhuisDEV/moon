# Changelog

All notable changes to this project are documented in this file.

The format is based on Keep a Changelog and this project follows Semantic
Versioning.

## [Unreleased]

## [1.2.1] - 2026-04-24

### Added

- Added active-context packet coverage diagnostics so Moon can report whether
  injected context is enough, current-turn-only, or should be followed by live
  search/read work.
- Added `project_status=updated|skipped-unchanged` diagnostics to
  `moon context-engine`.

### Changed

- Tightened active-context packet selection to omit weak zero-overlap evidence,
  avoid duplicating latest user turns as hot evidence, and keep recent activity
  relevant to the current query or actionable work.
- The optional assembly curator prompt now preserves the packet's
  `Context Coverage` section.

### Fixed

- Repeated context-engine sync/assemble passes now skip regenerating unchanged
  active-session hot projections and avoid re-marking embed maintenance pending
  when the copied raw transcript is unchanged.

## [1.2.0] - 2026-04-23

### Added

- Added `moon context-engine --sync-only` for record/project/state refresh work
  that does not need to rebuild assembly or the active context packet.

### Changed

- Moon context assembly now parses the raw session once per checkpoint and
  reuses that snapshot for `cleanse`, `assemble`, and packet building.
- Active context packet retrieval now routes to one primary source family first
  instead of broad routine fanout:
  - `hot`
  - `memory`
  - `library`
  - `distill`
  - bounded `semantic` fallback
- The packet builder now elects one canonical source for duplicate evidence
  before section budgeting, reducing repeated evidence across source families.
- The bundled Moon plugin now uses `--sync-only` during `afterTurn`, keeping
  refresh work out of the pre-dispatch assembly hot path.

### Fixed

- Reduced repeated context-engine work on common turns by removing duplicate raw
  parsing and routine broad multi-source packet retrieval.
- Reduced routine QMD usage by reserving it for routed fallback paths instead of
  calling both hot and library recall on the common path.

## [1.1.5] - 2026-04-23

### Docs

- Moved troubleshooting guidance out of `README.md` into a dedicated
  `docs/troubleshooting.md` file so known failure modes have a stable reference
  point.
- Documented the `spawn moon ENOENT` incident and recovery steps in the new
  troubleshooting file.
- Removed the standalone resolved incident report after folding its durable
  guidance into the troubleshooting docs.

## [1.1.4] - 2026-04-23

### Fixed

- Moon's OpenClaw plugin now preserves configured absolute `moonPath` values
  instead of sending them through host path resolution and silently falling back
  to bare `moon`.
- Context-engine launch failures now report the resolved executable path and
  process cwd, making `spawn ... ENOENT` incidents diagnosable from gateway
  logs.

## [1.1.3] - 2026-04-22

### Fixed

- Moon `cleanse` and `distill` now give `openai-codex` streamed responses more
  time to complete and retry transient timeout, overload, and missing-text
  failures before surfacing a context-engine error.
- This reduces intermittent `context engine assemble failed` /
  `afterTurn failed` noise caused by provider-side stalls after the OpenClaw
  timeout fix in `1.1.2`.

## [1.1.2] - 2026-04-22

### Fixed

- Managed OpenClaw installs, upgrades, and the bundled Moon plugin now use
  `contextEngineTimeoutMs=120000` instead of `20000`, preventing long
  `moon context-engine` runs from being killed on larger sessions.
- Added regression coverage for install, config patching, canonical-path, and
  idempotency flows so the managed timeout remains stamped into plugin config.

### Docs

- Updated `README.md`, `BOOTSTRAP.md`, `assets/plugin/README.md`, and
  `handoff.md` to document the managed context-engine timeout.

## [1.1.1] - 2026-04-22

### Fixed

- Restored the standard Clap version flag so Moon now supports:
  - `moon --version`
  - `moon -V`

### Docs

- Updated `README.md` and `handoff.md` to document the version flag.

## [1.1.0] - 2026-04-21

### Added

- Added owner-only runtime secret permission checks to the shared
  `status`/`verify` path so strict verify now enforces the Moon runtime security
  contract instead of only reporting OpenClaw/plugin state.
- Added regression coverage for:
  - owner-only runtime permissions on install/login artifacts
  - strict verify failure on insecure runtime secret permissions
  - sanitized OpenAI Codex OAuth and cleanse failure output

### Changed

- `moon install` and `moon update` now repair secret-bearing Moon runtime paths
  to owner-only permissions on Unix:
  - `$MOON_HOME/.env`
  - `$MOON_HOME/auth/`
  - `$MOON_HOME/auth/openai-codex.json`
  - `$MOON_HOME/logs/`
  - `$MOON_HOME/logs/audit.log`
  - `$MOON_HOME/logs/distill.audit.log`

### Fixed

- Managed OpenAI Codex credentials are now persisted through hardened private
  filesystem helpers instead of default world-readable writes.
- Moon audit and distill audit logs are now opened through owner-only append
  paths instead of inheriting ambient file modes.
- OpenAI Codex OAuth/login, cleanse, and distill failure paths no longer echo
  raw remote response bodies into CLI errors or Moon audit trails; they now
  retain only status plus request id when available.

### Docs

- Updated `README.md`, `docs/runbook.md`, `docs/security_checklist.md`, and
  `handoff.md` to document the runtime secret permission contract and sanitized
  provider failure behavior.

## [1.0.16] - 2026-04-21

### Fixed

- Scheduled watcher `syns` now catches up later the same local day if Moon
  missed the exact trigger minute while the daemon was down or unhealthy.
- Scheduled watcher `syns` now uses only the previous completed local
  daily-memory file plus `MEMORY.md`; it no longer falls back to the current
  day's daily-memory file.
- When the previous-day daily-memory file is missing or empty, scheduled watcher
  `syns` now skips cleanly and leaves the daily trigger state unchanged.

## [1.0.15] - 2026-04-21

### Changed

- The optional Moon assembly curator subagent now defaults to fast
  provider-specific models when gated mode is enabled and the operator omits an
  explicit subagent model:
  - `openai` -> `gpt-5.4-mini`
  - `openai-codex` -> `gpt-5.4-mini`
  - `google` / `gemini` -> `gemini-3.1-flash-lite-preview`
  - `anthropic` -> `claude-3-5-haiku-latest`
- The plugin now infers `openai` as the curator provider when the configured
  subagent model already implies an OpenAI family model such as `gpt-5.4-mini`.

## [1.0.14] - 2026-04-21

### Changed

- Default active-context packet directory now resolves to `$MOON_HOME/mcp/`
  instead of `$MOON_HOME/context-packets/`.

### Fixed

- `moon verify --strict` now treats OpenClaw doctor timeouts as advisory when
  Moon status is otherwise healthy, so successful `moon update` runs do not
  return a false failure on transient doctor timeout.

## [1.0.13] - 2026-04-20

### Added

- Added the Moon active-context packet path end to end:
  - routine hot projection refresh on every checkpoint
  - packet artifact generation under `$MOON_HOME/context-packets/`
  - plugin-side packet injection through the OpenClaw `messages` lane
  - optional gated Moon-owned curator subagent for packet curation
- Added `moon uninstall` with:
  - safe default cleanup for OpenClaw integration and generated Moon runtime
    artifacts
  - `--purge` for full `MOON_HOME` removal
  - `--remove-binary` to attempt `cargo uninstall moon`

### Changed

- Rewrote `README.md` to reflect the current Moon architecture, command surface,
  Codex OAuth flow, active packet design, and uninstall behavior.

### Fixed

- `moon status` and `moon health` now treat the context-packet dir as
  not-yet-created runtime state instead of a hard failure when no packet has
  been generated yet.

### Docs

- Updated `docs/runbook.md`, `docs/contracts.md`, `assets/plugin/README.md`,
  `dev-notes.md`, `moon.toml.example`, `mip.md`, and `handoff.md` to match the
  current runtime and uninstall contract.

## [1.0.12] - 2026-04-20

### Added

- Added end-to-end OpenAI Codex OAuth login support via
  `moon login
  openai-codex`, including managed token storage under Moon
  runtime state.

### Fixed

- Fixed OpenAI Codex cleanse/distill request handling in the remote model lane.
- `moon verify --strict --json` no longer falls back to interactive OpenClaw
  doctor execution; verify now uses the non-interactive doctor path only and
  returns a bounded failure when doctor times out.

### Docs

- Updated the runtime docs and env examples for the Codex OAuth login/setup
  flow.

## [1.0.11] - 2026-04-08

### Added

- Added OpenClaw-style `openai-codex` remote provider support for
  `moon cleanse`, remote `moon distill`, and remote wisdom synthesis via
  `OPENAI_OAUTH_TOKEN` and optional `OPENAI_CODEX_BASE_URL`.

### Fixed

- `moon update` no longer reports a false `assets_match_local=false` after a
  successful reinstall; post-install plugin alignment is now reported by the
  newly installed binary's `verify --strict` path.
- Aligned crate metadata with the current GitHub remote so release metadata now
  points to `zhuisDEV/moon`.

### Docs

- Updated `.env.example`, `README.md`, `docs/runbook.md`, `handoff.md`, and the
  install-time env template to document the new Codex OAuth lane and the current
  provider environment variable contract.

## [1.0.10] - 2026-04-08

### Changed

- Moon's OpenClaw context-engine path no longer injects the rich Moon assembly
  artifact into routine `systemPromptAddition` during normal `assemble()`.
- Moon `cleanse` summaries now rely on transcript `compaction` entries and the
  downstream `compactionSummary` message-history lane instead of routine dynamic
  system-prompt text.
- Operator/debug assembly artifacts remain on disk while provider-facing prompt
  context stays focused on compaction summaries.

### Fixed

- Restored legacy `maxAssemblyChars` plugin config acceptance as a deprecated
  compatibility no-op so existing installs continue to validate after upgrade.
- Removed the dead assembly-artifact read from the plugin assemble path now that
  routine assembly is operator-only.

### Docs

- Updated `mip.md`, `README.md`, `docs/contracts.md`, `assets/plugin/README.md`,
  `dev-notes.md`, and `RELEASE.md` to reflect the landed prompt-boundary
  contract and release validation steps.

## [1.0.9] - 2026-04-02

### Added

- Added a Moon-owned OpenClaw memory contract in install/config repair:
  - `plugins.slots.memory = "none"`
  - `agents.defaults.memorySearch.enabled = false`
- Added explicit memory-contract diagnostics in `status` and `moon status`:
  - resolved memory slot reporting
  - legacy memory-search state reporting
  - exact-key drift issues for missing/stale values
- Added regression coverage for:
  - install-time stale memory contract repair
  - strict verify failures for stale/missing memory contract
  - `moon status` drift reporting and clean-config expectations

### Changed

- `verify` concise mode now reports the true count of suppressed `status`
  details (instead of total status details).
- `status` OpenClaw registry/load issue text now reflects `plugins info` as the
  primary source with `plugins list` fallback.
- Simplified plugin verify internals by removing retained-but-unused provenance
  message collections.

### Docs

- Fixed numbering drift in `README.md` command notes/upgrade sections.
- Clarified `verify --strict` source preference (`plugins info` primary,
  `plugins list` fallback).
- Documented Moon-owned OpenClaw memory contract behavior in `README.md` and
  `docs/runbook.md`.

## [1.0.8] - 2026-03-28

### Fixed

- `verify` now parses OpenClaw `plugins info` nested schema (`plugin.id`,
  `plugin.status`) so loaded moon plugins are detected correctly.
- `verify` now prefers `plugins info` first and only falls back to
  `plugins list` when needed, reducing large JSON pressure in normal paths.
- External command timeout handling now drains subprocess stdout/stderr while
  waiting, preventing pipe-buffer stalls on large outputs.
- `status` no longer emits “plugin is listed but not loaded” when the plugin is
  not listed.

### Changed

- `verify` now defaults to concise summary output and supports full detail with
  `--verbose`.
- Non-JSON verify output now prints issues before details when verification
  fails.

### Docs

- Updated CLI docs for `verify [--strict] [--verbose]`.

## [1.0.7] - 2026-03-28

### Fixed

- `moon install`, `moon update`, and `moon restart` now run an OpenClaw plugin
  alignment check and attempt registry/load repair when Moon is not listed or
  loaded.
- Aligned release metadata so crate + plugin package + plugin runtime ship as
  `v1.0.7`.

### Docs

- Updated `README.md` to document post-update OpenClaw plugin alignment.

## [1.0.6] - 2026-03-28

### Added

- Added `distill.syns_trigger_time_local` (`HH:MM`, 24-hour local time) so
  `moon.toml` can set the daily watcher SYNS trigger time.

### Fixed

- Watcher SYNS scheduling now honors configured local trigger time while
  preserving once-per-day execution guardrails.
- `moon config --show` now reports `distill.syns_trigger_time_local`.
- Aligned release metadata so crate + plugin package + plugin runtime ship as
  `v1.0.6`.

### Docs

- Updated `moon.toml.example` and `README.md` baseline config with
  `distill.syns_trigger_time_local`.

## [1.0.5] - 2026-03-20

### Added

- Added `moon update` command with stable/main channel targeting, plus `--check`
  and `--dry-run`.
- `moon update` now preserves existing `$MOON_HOME/.env` and
  `$MOON_HOME/moon.toml` across upgrade/install flows.

### Fixed

- `moon status` now checks watcher daemon lock runtime health and fails when a
  stale lock references a dead PID.

### Docs

- Rewrote `README.md` to align with current CLI/runtime behavior and remove
  stale guidance drift.
- Updated uninstall guidance for safer plugin/service/runtime cleanup and clear
  optional full-wipe behavior.

## [1.0.4] - 2026-03-20

### Docs

- Rewrote `SKILL.md` to a minimal runtime CLI guide for agents.
- Added an explicit runtime-doc handoff to `$MOON_HOME/README.md` from
  `SKILL.md`.
- Removed hardcoded absolute/home paths from repo documentation and replaced
  them with variable-based path forms.
- Consolidated troubleshooting guidance flow so repo docs remain portable for
  GitHub publishing.

## [1.0.3] - 2026-03-20

### Fixed

- Resolved strict lint warnings affecting macOS-gated imports and test paths.
- Kept release metadata aligned so `main`, crate version, and plugin version now
  ship together as `v1.0.3`.

## [1.0.2] - 2026-03-20

### Fixed

- `moon` now resolves its runtime env file from `~/.moon/.env` when `MOON_HOME`
  is unset or blank, matching the documented default runtime root.
- `moon stop` and `moon restart` now work from a normal shell without requiring
  an exported `MOON_HOME`, as long as `~/.moon/.env` exists.

## [1.0.1] - 2026-03-18

### Changed

- Watcher daemon lifecycle now uses a real exclusive process lock for
  `moon-watch.daemon.lock`, preventing duplicate daemon instances under the same
  `MOON_HOME`.
- Daemon startup now fails fast with a clear owner PID message when another
  watcher already holds the lock.
- Daemon loop now records watcher cycle failures to `logs/audit.log`
  (`phase=watcher`, `status=error`) instead of silently swallowing errors.

### Fixed

- Reduced stale-lock drift caused by concurrent `moon watch --daemon` starts.
- Improved lock cleanup reliability on graceful daemon shutdown.

### Docs

- Updated README, runbook, and troubleshooting guidance for the new
  single-instance daemon behavior and lock recovery flow.

## [1.0.0] - 2026-03-16

### Added

- `moon-context-engine` as the primary normal-path controller with explicit
  `record`, `cleanse`, and `assemble` boundaries.
- Two-lane projection model:
  - hot lane: `raw -> mds`
  - library lane: `raw -> mlib`
- Explicit hot collection lifecycle policy:
  - `hot_collection.lifecycle_mode = degrade|strict`
  - `hot_collection.lifecycle_command_mode = primary|fallback`
- Watcher strict parity for hot collection lifecycle and hot degraded-embed
  handling.
- Native plugin fallback handoff controls:
  - `fallbackMode`
  - `compactFallbackOnSkip`

### Changed

- `moon cleanse` is now compaction-only (no implicit pre-project).
- `distill --mode norm` maintenance flow consumes projected library docs from
  `mlib`.
- Runtime `.env` handling is strict from `$MOON_HOME/.env`.
- Public OSS paperwork and maintainer workflow docs were added for v1 launch.

### Docs

- Added release/support/governance docs and GitHub issue/PR templates.
- Updated README and skill docs to align with final v1 architecture and fallback
  behavior.
