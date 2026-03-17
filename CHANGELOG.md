# Changelog

All notable changes to this project are documented in this file.

The format is based on Keep a Changelog and this project follows Semantic Versioning.

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
