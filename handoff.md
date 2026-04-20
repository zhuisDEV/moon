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

## 2026-04-08 13:28 AEST

- Polished `moon update` post-install plugin reporting after observing a false
  `assets_match_local=false` during a successful `v1.0.10` upgrade.
- Root cause:
  - `moon update` begins under the old binary version
  - after `cargo install`, the old in-process binary was still running
    `align_openclaw_plugin_state(...)`
  - that caused embedded old plugin assets to be compared against freshly
    installed new plugin files
- Fix:
  - when update installs a new binary, plugin alignment reporting is now
    deferred to the newly installed binary's `verify --strict` run
  - in-process alignment remains unchanged for no-op update paths where no
    reinstall occurs
- Added unit coverage in `src/commands/update.rs` for the alignment-strategy
  split.

## 2026-04-08 14:17 AEST

- Added OpenClaw-style `openai-codex` remote provider support in Moon.
- Scope:
  - `moon cleanse`
  - remote `moon distill`
  - remote wisdom synthesis (`MOON_WISDOM_PROVIDER`)
- Provider contract:
  - provider id: `openai-codex`
  - alias: `codex`
  - auth env: `OPENAI_OAUTH_TOKEN`
  - optional base URL override: `OPENAI_CODEX_BASE_URL`
  - default base URL: `https://chatgpt.com/backend-api`
  - request path: `/codex/responses`
- Implementation notes:
  - kept `openai-codex` distinct from standard `openai`
  - model-name inference maps `*codex*` models to the new provider
  - standard OpenAI-style response text extraction is reused for Codex
- Tests/docs:
  - added a fake-server integration test in `tests/moon_primary_flow_test.rs`
    covering `moon cleanse` with `OPENAI_OAUTH_TOKEN`
  - added provider parsing/inference coverage in `src/moon/distill.rs`
  - updated `.env.example`, `README.md`, and `src/commands/install.rs`
  - corrected docs to use `ANTHROPIC_API_KEY` instead of the stale
    `CLAUDE_API_KEY`

## 2026-04-08 14:42 AEST

- Prepared the next release as `v1.0.11`.
- Release-scope cleanup:
  - aligned `Cargo.toml` package metadata with the live GitHub remote
    (`zhuisDEV/moon`)
  - bumped crate/plugin runtime versions to `1.0.11`
  - added a `CHANGELOG.md` entry for:
    - OpenClaw-style `openai-codex` support
    - the earlier `moon update` plugin-alignment reporting fix
  - updated `docs/runbook.md` with the Codex OAuth example lane
- Code cleanup:
  - deduplicated the `openai-codex` remote request path in `src/moon/distill.rs`
    so remote distill and wisdom synthesis share the same helper

## 2026-04-20 15:18 AEST

- Investigated a reported runtime caveat where `moon verify --strict --json`
  appeared to hang after `moon restart` fixed the watcher/runtime state.
- Findings:
  - current live `moon verify --strict --json` could complete successfully on
    the installed `v1.0.11` binary, but the doctor path remained structurally
    unsafe for non-interactive verification
  - `verify` called OpenClaw doctor through `src/openclaw/gateway.rs`
  - that path retried `openclaw doctor --non-interactive`, then fell back to a
    plain interactive `openclaw doctor`
  - that fallback is inappropriate for `verify --json` / strict automation and
    can present as a hang when OpenClaw doctor is slow or waiting on interactive
    behavior
- Fix:
  - `run_doctor()` now uses only `openclaw doctor --non-interactive`
  - added a dedicated 30s doctor timeout instead of inheriting the generic 120s
    external-command timeout
  - removed the interactive doctor fallback from the verify path
  - improved verify issue text to preserve the underlying doctor error cause
    chain
- Added regression coverage in `tests/context_engine_slot_status_test.rs` to
  assert `moon verify --strict --json` reports doctor failure without ever
  invoking plain `openclaw doctor`.

## 2026-04-20 15:37 AEST

- Merged `codex/openai-codex-oauth-login` forward onto local `main`.
- Prepared release metadata for `v1.0.12`:
  - bumped crate/plugin runtime versions
  - added `CHANGELOG.md` release notes for:
    - OpenAI Codex OAuth login
    - Codex cleanse request handling fixes
    - bounded non-interactive verify doctor behavior
- Release validation follow-up:
  - fixed a `clippy::match_like_matches_macro` lint in
    `src/moon/openai_codex_auth.rs` so `cargo clippy --all-targets --all-features -- -D warnings`
    passes for the release build

## 2026-04-17 20:05 AEST

- Implemented end-to-end OpenAI Codex OAuth login in Moon.
- Scope:
  - new `moon login` command for `openai-codex`
  - Moon-managed auth store at `$MOON_HOME/auth/openai-codex.json`
  - automatic bearer-token refresh for Moon-managed `openai-codex` credentials
  - fresh `~/.codex/auth.json` reuse as a read-only fallback when Moon-managed
    credentials are absent
- Runtime behavior:
  - `OPENAI_OAUTH_TOKEN` remains the highest-priority explicit override
  - `moon cleanse` and remote `moon distill` now resolve `openai-codex`
    credentials from the managed auth store instead of requiring only the env
    token path
  - when Moon falls back from an inferred provider with missing credentials, it
    can now promote `openai-codex` automatically if the OAuth lane is available
- Tests/docs:
  - added `tests/moon_oauth_login_test.rs` covering:
    - headless login and auth-file persistence
    - `moon cleanse` using the Moon-managed auth store
    - refresh of an expired managed credential before the Codex request
  - updated `README.md`, `docs/runbook.md`, `.env.example`, and
    `src/commands/install.rs` to document `moon login`

## 2026-04-20 20:55 AEST

- Rewrote `mip.md` as the next control-plan MIP for Moon active context
  assembly and memory-management performance.
- Verified planning baseline before the rewrite:
  - Moon `main` at `1b1254b5464a473f65336c3d97420a768526d61a`
  - OpenClaw `origin/main` at `94e2bf258d6ee35f4661c73bc3400c6bba52885a`
- New planning direction:
  - keep OpenClaw `active-memory` disabled
  - keep routine Moon `systemPromptAddition` unused
  - move Moon active context into the `messages` lane as a Moon-owned active
    context packet
  - refresh the hot searchable corpus on every checkpoint, not only when
    `cleanse` runs
  - add deterministic Moon retrieval first
  - add a bounded Moon-owned curator subagent in the plugin only as a gated
    second-stage selector
- The rewritten MIP now includes:
  - verified baseline and ownership boundary
  - target architecture for packet retrieval and placement
  - config plan
  - phased implementation plan
  - test plan
  - rollout and acceptance criteria

## 2026-04-20 23:40 AEST

- Implemented the active-context packet MIP end to end.
- Rust control-plane changes:
  - `src/moon/context_engine.rs`
    - hot projection now refreshes every checkpoint, not only on `cleanse`
    - context-engine now builds a separate active context packet artifact
    - context-engine reports packet path, chars, candidate count, cache hit, and
      query
  - new `src/moon/context_packet.rs`
    - deterministic packet builder over:
      - hot projection / raw session projection data
      - latest `cleanse` summary when replay does not already have
        `compactionSummary`
      - durable `MEMORY.md`
      - recent daily memory files
      - recent library docs
      - recent distilled artifacts
      - QMD recall hits when available
    - packet artifact path is `$MOON_HOME/context-packets/<session_id>.md`
    - local generation cache reuses the previous packet when the source corpus
      has not changed
  - `src/commands/moon_assemble.rs` and `src/commands/moon_context_engine.rs`
    now surface packet diagnostics
  - `src/commands/moon_status.rs` and `src/commands/moon_health.rs` now report
    the packet directory/state without treating a not-yet-created packet
    directory as a failure
  - added `context_packet` config to `src/moon/config.rs`,
    `moon.toml.example`, and state tracking in `src/moon/state.rs`
- Plugin changes:
  - `assets/plugin/index.js`
    - reads `context_engine.packet_*` details
    - injects the packet into the OpenClaw `messages` lane during routine
      `assemble()`
    - keeps routine `systemPromptAddition` unused
    - passes `--replay-has-compaction-summary` back to Moon when replay already
      contains `compactionSummary`
    - adds an optional gated Moon-owned curator subagent path hosted via
      `api.runtime.agent.runEmbeddedPiAgent`
    - caches curator outputs by session/prompt/packet fingerprint
  - `assets/plugin/openclaw.plugin.json`
    - added packet/subagent config schema keys
    - kept `maxAssemblyChars` as a deprecated compatibility no-op
- Docs updated:
  - `README.md`
  - `docs/contracts.md`
  - `assets/plugin/README.md`
  - `dev-notes.md`
  - `moon.toml.example`
  - `mip.md` status now marks the plan implemented
- Verification:
  - `cargo fmt --all`
  - `cargo test --all-targets --all-features`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `deno fmt assets/plugin/index.js assets/plugin/index.test.ts assets/plugin/openclaw.plugin.json`
  - `deno lint assets/plugin/index.js assets/plugin/index.test.ts`
  - `deno test --allow-read --allow-write --allow-env --allow-run assets/plugin/index.test.ts`

## 2026-04-20 23:58 AEST

- Added `moon uninstall` and wired it into the CLI.
- Uninstall behavior:
  - default uninstall removes runtime artifacts, daemon wiring, OpenClaw plugin
    integration, and Moon runtime skills
  - preserves user-owned state by default:
    - `$MOON_HOME/memory/`
    - `$MOON_HOME/MEMORY.md`
    - `$MOON_HOME/.env`
    - `$MOON_HOME/moon.toml`
    - `$MOON_HOME/auth/`
  - `--purge` removes the full `MOON_HOME`
  - `--remove-binary` attempts `cargo uninstall moon`
- Added OpenClaw cleanup helpers for uninstall:
  - removes `plugins.entries.moon`
  - removes `plugins.installs.moon`
  - clears Moon-owned slot/config toggles when present
- Rewrote `README.md` to reflect the current Moon architecture and command
  surface, including the active-context packet and uninstall flow.
- Updated `docs/runbook.md` for current runtime paths, assemble behavior, and
  uninstall usage.
- Added uninstall regression tests in `tests/uninstall_flow_test.rs`.
- Validation run during this pass:
  - `cargo fmt --all`
  - `deno fmt README.md docs/runbook.md assets/plugin/index.js assets/plugin/index.test.ts assets/plugin/openclaw.plugin.json`
  - `cargo test --test uninstall_flow_test -- --nocapture`
  - `cargo fmt --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test --all-targets --all-features`
  - `deno fmt --check assets/plugin/index.js assets/plugin/index.test.ts assets/plugin/openclaw.plugin.json README.md docs/runbook.md`
  - `deno lint assets/plugin/index.js assets/plugin/index.test.ts`
  - `deno test --allow-read --allow-write --allow-env --allow-run assets/plugin/index.test.ts`
