# Handoff

## 2026-04-07 00:18 AEDT

- Fast-forwarded Moon from `badcd3c` to `c22d84e` (`v1.0.9`).
- Verified OpenClaw against fetched upstream `origin/main` at `48a3511233`
  (`v2026.4.5`) without changing the local divergent branch
  `codex/acp-visible-text-refresh`.
- Traced the concrete Moon -> OpenClaw -> provider flow from code and docs.
- Wrote the report to:
  - `openclaw-moon-context-flow-report.md`
- Key verified conclusion:
  - Moon produces assembled context and returns it to OpenClaw as
    `systemPromptAddition`.
  - OpenClaw still owns final system prompt assembly, session messages, provider
    stream resolution, and outbound provider request shaping.

## 2026-04-08 04:32 AEST

- Restructured `CC_research.md` from raw copied X-post text into a
  decision-ready research note.
- Preserved the main Claude Code architectural details while separating:
  - source claims
  - Moon / OpenClaw relevance
  - verification status
- Added cross-entry synthesis and practical follow-up buckets so the file is
  usable for future Moon design work without treating X posts as verified
  architecture.

## 2026-04-08 05:47 AEST

- Wrote `mip-proposal.md` as a new proposed MIP for cache-aware Moon context
  assembly.
- Grounded the proposal in verified code paths:
  - Moon writes an assembly artifact and returns it through the plugin as
    `systemPromptAddition`
  - OpenClaw inserts that addition after its cache boundary and still owns the
    final prompt envelope
- Key proposal direction:
  - split operator/debug artifact from model-facing prompt addition
  - remove volatile metadata from the model-facing path
  - add stability checks for prompt-facing Moon output

## 2026-04-08 07:27 AEST

- Re-verified `mip-proposal.md` against the current source-of-truth repos:
  - Moon `origin/main` at `c22d84eaa44c2e6aecfbde66ea599198adbe04f5`
  - OpenClaw local `main` tracking `zhuisDEV/main` at
    `5fb6aeaf86c7bcab1984c9719888e4987dfea139`
- Confirmed the proposal remains compatible with the current OpenClaw context
  engine contract and Moon plugin/runtime split.
- Refined the proposal to add:
  - explicit compatibility baseline
  - verified extension seams (`stripFrontMatter`, `trimAssemblyText`, status,
    verify)
  - a caveat that Phase 3 needs a replacement for blind head/tail clipping
    rather than assuming current trimming is sufficient.

## 2026-04-08 08:49 AEST

- Added `dev-notes.md` to capture a verified note on OpenClaw prompt layers.
- The note distinguishes:
  - static system-prompt prefix
  - dynamic system-prompt suffix
  - user/message-history layer
  - structured tool-definition layer
  - compaction summaries as `compactionSummary` message context
- Grounded the note in current OpenClaw and Moon code paths rather than prior
  assumptions from Moon v1 design.

## 2026-04-08 09:58 AEST

- Re-verified OpenClaw prompt-boundary behavior against the true source of
  truth: `origin/main` at `a44a26f0a0a4`.
- Confirmed the checked-out local OpenClaw branch
  `codex/context-engine-main-refresh` was stale (`ahead 2, behind 581`), so the
  planning update was based on `origin/main`, not the local branch tip.
- Verified current boundary mechanics:
  - `systemPromptAddition` is inserted after `OPENCLAW_CACHE_BOUNDARY`
  - the boundary applies inside OpenClaw's system prompt, not to `messages` or
    tools
  - transcript compaction entries replay as `compactionSummary` message-history
    context
- Adopted the next Moon design direction:
  - Moon `cleanse` is the Moon compaction stage
  - Moon `cleanse` summaries should use the `compactionSummary` lane
  - Moon should stop using routine `systemPromptAddition` injection for now
  - indexing/embed/operator receipts should stay out of model-facing prompt
    context by default
- Updated `dev-notes.md` with the verified `origin/main` boundary addendum and
  the adopted Moon design decision.
- Rewrote `mip.md` into a concrete implementation MIP for the coding team,
  focused on:
  - removing routine Moon `systemPromptAddition`
  - keeping the Moon assembly artifact operator-only
  - using the existing OpenClaw compaction-summary message lane for Moon
    `cleanse`

## 2026-04-08 10:34 AEST

- Implemented Phase 1 of `mip.md` in the Moon OpenClaw plugin.
- Updated `assets/plugin/index.js` so `assemble()`:
  - still runs `moon context-engine`
  - preserves normal `messages`
  - keeps operator-side effects intact
  - stops returning routine `systemPromptAddition`
- Removed the now-dead assemble-path `maxAssemblyChars` config and
  `trimAssemblyText(...)` logic.
- Kept the compaction lane unchanged:
  - Moon `compact()` still appends transcript `compaction` entries
  - `stripFrontMatter(...)` remains in use for compaction-summary normalization
- Updated plugin docs/schema to match the new boundary:
  - `assets/plugin/README.md`
  - `assets/plugin/openclaw.plugin.json`
- Added a regression test in `assets/plugin/index.test.ts` asserting that
  routine Moon assembly no longer injects `systemPromptAddition`.
- Validation completed:
  - `deno fmt assets/plugin/index.js assets/plugin/index.test.ts assets/plugin/openclaw.plugin.json`
  - `deno test --allow-read --allow-write --allow-env assets/plugin/index.test.ts`
  - `cargo test -q --manifest-path Cargo.toml assemble_context_renders_cleanse_summary_and_raw_excerpt -- --nocapture`

## 2026-04-08 11:02 AEST

- Continued the `mip.md` implementation through the remaining acceptance/doc
  cleanup phases.
- Added an explicit no-duplication regression to `assets/plugin/index.test.ts`:
  - compaction still appends the Moon `cleanse` summary into the transcript
    `compaction` lane
  - subsequent `assemble()` still returns no `systemPromptAddition`
  - the compaction summary text does not leak back into the assemble result
- Updated repository docs to match the implemented boundary:
  - `README.md` now states Moon owns preparation/artifacts while OpenClaw owns
    the final provider-facing prompt envelope
  - `docs/contracts.md` now defines the OpenClaw boundary and states that normal
    Moon summary context must use the transcript compaction lane
  - `dev-notes.md` now includes the landed implementation status
- Full validation completed:
  - `deno fmt assets/plugin/index.js assets/plugin/index.test.ts assets/plugin/openclaw.plugin.json`
  - `deno test --allow-read --allow-write --allow-env --allow-run assets/plugin/index.test.ts`
  - `cargo test --quiet`
- Note: the first in-sandbox `cargo test --quiet` hit a local bind
  `PermissionDenied` in fake-server tests; rerunning the same command outside
  the sandbox passed cleanly.

## 2026-04-08 11:18 AEST

- Followed up on post-implementation review findings in the Moon OpenClaw
  plugin.
- Restored `maxAssemblyChars` in `assets/plugin/openclaw.plugin.json` as a
  deprecated compatibility-only config key so existing installs with legacy
  plugin config continue to validate cleanly.
- Removed the dead `assemblyText` read from `runMoonContextEngine()` in
  `assets/plugin/index.js` now that routine `assemble()` no longer injects Moon
  text into `systemPromptAddition`.
- Updated `assets/plugin/README.md` to mark `maxAssemblyChars` as accepted but
  ignored.
- Added a Deno regression test in `assets/plugin/index.test.ts` to keep the
  compatibility key present in the manifest.

## 2026-04-08 11:42 AEST

- Prepared the Moon prompt-boundary cleanup for release as `v1.0.10`.
- Aligned release metadata across:
  - `Cargo.toml`
  - `Cargo.lock`
  - `assets/plugin/package.json`
  - `assets/plugin/index.js`
- Marked `mip.md` as implemented/completed for the current scope.
- Updated release-facing docs:
  - `CHANGELOG.md`
  - `RELEASE.md`
  - `dev-notes.md`
  - `mip-proposal.md`
  - `openclaw-moon-context-flow-report.md`
