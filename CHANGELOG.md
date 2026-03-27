# Changelog

All notable changes to this project are documented in this file.

The format is based on Keep a Changelog and this project follows Semantic Versioning.

## [Unreleased]

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
- Added an explicit runtime-doc handoff to `$MOON_HOME/README.md` from `SKILL.md`.
- Removed hardcoded absolute/home paths from repo documentation and replaced
  them with variable-based path forms.
- Consolidated troubleshooting guidance flow so repo docs remain portable for
  GitHub publishing.

## [1.0.3] - 2026-03-20

### Fixed

- Resolved strict lint warnings affecting macOS-gated imports and test paths.
- Kept release metadata aligned so `main`, crate version, and plugin version
  now ship together as `v1.0.3`.

## [1.0.2] - 2026-03-20

### Fixed

- `moon` now resolves its runtime env file from `~/.moon/.env` when
  `MOON_HOME` is unset or blank, matching the documented default runtime root.
- `moon stop` and `moon restart` now work from a normal shell without requiring
  an exported `MOON_HOME`, as long as `~/.moon/.env` exists.

## [1.0.1] - 2026-03-18

### Changed

- Watcher daemon lifecycle now uses a real exclusive process lock for
  `moon-watch.daemon.lock`, preventing duplicate daemon instances under the
  same `MOON_HOME`.
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
- Updated README and skill docs to align with final v1 architecture and
  fallback behavior.
