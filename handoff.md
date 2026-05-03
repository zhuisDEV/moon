# Handoff

## 2026-05-04 09:02 AEST

- Prepared release `v1.2.5` for the active-context topic-switch weighting fix.
- Incident from the live Discord conversation:
  - user discussed Discord slash command config (`commands.native`, gateway
    status)
  - user then started a new topic about a child's night-time nasal congestion
    and soup suggestions in Chinese
  - after the user explicitly said the topic was soup, the agent later answered
    with stale Discord command/gateway status
- Root cause:
  - Moon active packet retrieval built query terms from recent user turns, but
    the tokenizer was mostly ASCII-oriented.
  - The Chinese/CJK soup-topic turns produced weak or no useful current-query
    terms.
  - Sparse-query fallback reused keywords from the whole session, allowing stale
    English terms from the prior Discord command topic to dominate relevance
    scoring.
  - A second reinforcement risk existed when injected `# Moon Active Context`
    packets were replayed into projection parsing and could be treated as real
    assistant history.
- Context boundary decision:
  - old active packets are valid as injected model-facing context for the
    current provider call
  - old active packets are not valid primary source material when Moon builds
    the next active packet from projection data
  - old packets may only be consulted through an explicit, separately gated
    recovery fallback for damaged/compacted transcripts
  - primary flow must stay current transcript/projection first; fallback must
    not be mixed into that path
- Implementation:
  - `src/moon/context_packet.rs`
    - added CJK-aware tokenization for active packet query terms
    - changed sparse current-query expansion to borrow terms from the recent
      conversation tail instead of whole-session keywords
    - bumped active packet generation to `v=2` so stale packet caches are not
      reused under the new scoring rules
    - refactored packet candidate helpers through a shared query context so
      clippy stays clean
    - added regression coverage for the observed topic-switch shape: English
      Discord config topic, then Chinese soup topic, then "please continue"
  - `src/moon/distill.rs`
    - filters replayed `# Moon Active Context` packets as synthetic projection
      noise
    - added regression coverage proving replayed active packets do not enter
      projection entries
  - `src/moon/project.rs`
    - cleaned a duplicated branch found by the release clippy pass
  - release metadata bumped to `1.2.5` in `Cargo.toml`, `Cargo.lock`,
    `assets/plugin/package.json`, and plugin runtime info
  - release-facing docs updated:
    - `CHANGELOG.md`
    - `README.md`
    - `assets/plugin/README.md`
    - `docs/contracts.md`
    - `docs/runbook.md`
    - `docs/troubleshooting.md`
    - `handoff.md`
- Validation completed:
  - `cargo fmt --all -- --check`
  - `deno fmt --check assets/plugin/index.js assets/plugin/index.test.ts assets/plugin/openclaw.plugin.json assets/plugin/README.md CHANGELOG.md README.md docs/contracts.md docs/runbook.md docs/troubleshooting.md RELEASE.md SUPPORT.md SECURITY.md handoff.md`
  - `deno lint assets/plugin/index.js assets/plugin/index.test.ts`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test --all-targets --all-features`
  - `deno test --allow-read --allow-write --allow-env --allow-run assets/plugin/index.test.ts`
  - `git diff --check`

## 2026-04-24 22:08 AEST

- Prepared release `v1.2.3` for the remaining Moon/OpenClaw usage-pressure
  mismatch.
- Root cause from live session `843ed391-6ab4-401f-aa98-1b528f65020b`:
  - OpenClaw provider usage reported `64,587` prompt tokens and matched
    `/status` at about `65k/200k`.
  - Moon stored `last_usage_ratio=0.945665`, which implies `189,133/200,000`.
  - The inflated value came from an untrusted `currentTokenCount` estimate path,
    not from provider `promptCache.lastCallUsage`.
- Plugin pressure handling changed:
  - `promptCache.lastCallUsage` is now the only trusted source for
    `--used-tokens`.
  - `currentTokenCount` is no longer forwarded to `moon context-engine` as
    cleanse pressure.
  - forced compaction may still keep `currentTokenCount` as a local
    `tokensBefore` metric, but it cannot update Moon `last_usage_ratio` or trip
    cleanse thresholds.
- Added plugin regression coverage proving:
  - current-token-count alone sends no pressure
  - provider last-call usage still sends pressure
  - provider usage wins over the observed inflated `189,133` estimate
  - compact skip/fallback does not forward inflated current-token pressure
- Validation completed:
  - `deno fmt --check assets/plugin/index.js assets/plugin/index.test.ts CHANGELOG.md handoff.md`
  - `deno lint assets/plugin/index.js assets/plugin/index.test.ts`
  - `deno test --allow-read --allow-write --allow-env --allow-run assets/plugin/index.test.ts`
  - `cargo fmt --all -- --check`
  - `cargo test checkpoint_records_and_assembles_without_cleanse_below_trigger -- --nocapture`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test --all-targets --all-features`

## 2026-04-24 21:29 AEST

- Prepared release `v1.2.2` for Moon/OpenClaw context-window usage alignment.
- Plugin pressure handling now prefers OpenClaw runtime token snapshots:
  - `runtimeContext.currentTokenCount`
  - `runtimeContext.promptCache.lastCallUsage` as input + cache read + cache
    write
  - `runtimeContext.tokenBudget` for the active model window when present
- When no trusted runtime used-token snapshot exists, the plugin now omits
  cleanse pressure instead of sending a message-envelope estimate that can
  overwrite Moon state with an inflated `last_usage_ratio`.
- `moon install` now clears persisted `last_usage_ratio` state during runtime
  refresh so stale pre-upgrade usage does not remain visible after install.
- Fallback estimated-token reporting now counts visible prompt/message content
  and ignores non-prompt metadata envelopes.
- Added plugin regression coverage for the stale usage mismatch class:
  - runtime `currentTokenCount` wins over metadata-heavy messages
  - last-call prompt/cache usage derives pressure correctly
  - no trusted runtime count means no `--used-tokens`/`--max-tokens` pressure
  - visible-message estimates ignore metadata envelopes
- Added Rust state coverage for clearing stale usage-ratio snapshots.

## 2026-04-24 19:09 AEST

- Prepared release `v1.2.1` for the active-context packet quality and unchanged
  hot projection skip work.
- Release metadata aligned across:
  - `Cargo.toml`
  - `Cargo.lock`
  - `assets/plugin/package.json`
  - `assets/plugin/index.js`
- Updated `CHANGELOG.md` with the `1.2.1` release note.

## 2026-04-24 18:16 AEST

- Improved Moon context-engine active-context quality and reduced avoidable
  repeated hot projection work.
- Rust runtime changes:
  - `src/moon/context_packet.rs`
    - tightened packet evidence selection so weak zero-overlap markdown lines
      are not injected as evidence
    - excluded latest user turns from hot evidence because `Current Goal`
      already carries them
    - filtered `Active Work` to recent assistant/tool/result lines that overlap
      the current query or are actionable
    - added explicit packet coverage guidance:
      - `enough`
      - `current_only`
      - `search_more`
    - added diagnostics for coverage decision, reason, positive candidate count,
      and top score
    - added tests proving irrelevant recent activity and irrelevant memory lines
      are omitted
  - `src/moon/context_engine.rs`
    - added hot projection cursor checks so unchanged active-session hot
      projections skip rewrite and avoid re-marking embed maintenance pending
    - preserved strict hot collection lifecycle behavior
    - reports `project_status=updated|skipped-unchanged`
  - `src/moon/state.rs`
    - added `hot_projection_cursors` as isolated hot projection metadata
    - this cache does not evict memory, library, distill, packet, or QMD cache
      data
  - `src/moon/project.rs`
    - prunes only stale hot projection cursor metadata on session switch,
      matching the existing hot-session cache lifecycle
  - `src/commands/moon_context_engine.rs` and `src/commands/moon_assemble.rs`
    - surfaced the new packet coverage diagnostics
- Plugin changes:
  - `assets/plugin/index.js`
    - updated the optional assembly curator prompt to preserve the new
      `Context Coverage` section when curation is enabled
- Cache rule:
  - no new broad retrieval cache was added
  - the new cache state is only a byte/line cursor for deciding whether the
    current session's hot projection file needs to be regenerated
  - existing old-cache retention semantics are unchanged except for hot-session
    metadata following the existing strict hot lifecycle
- Validation completed:
  - `cargo fmt --all`
  - `deno fmt handoff.md`
  - `cargo test context_packet_ -- --nocapture`
  - `cargo test checkpoint_records_and_assembles_without_cleanse_below_trigger -- --nocapture`
  - `deno fmt --check assets/plugin/index.js assets/plugin/index.test.ts`
  - `deno lint assets/plugin/index.js assets/plugin/index.test.ts`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `deno test --allow-read --allow-write --allow-env --allow-run assets/plugin/index.test.ts`
  - `cargo test --all-targets --all-features`

## 2026-04-23 20:43 AEST

- Implemented the optimisation MIP end to end and prepared release `v1.2.0`.
- Rust runtime changes:
  - `src/moon/distill.rs`
    - added `ProjectionSnapshot` plus `extract_projection_snapshot()`
    - raw-session parsing can now be shared instead of repeated
  - `src/moon/assemble.rs`
    - added `assemble_context_with_excerpt()` so callers can reuse a prepared
      raw excerpt
  - `src/moon/context_engine.rs`
    - refactored checkpoint setup into shared preparation
    - `run_checkpoint()` now parses raw session once and reuses that snapshot
      for `cleanse`, `assemble`, and packet build
    - added `run_sync_checkpoint()` for record/project/state refresh without
      assembly or packet work
  - `src/moon/context_packet.rs`
    - replaced broad multi-source packet fanout with routed single-source-first
      retrieval
    - added primary source family routing:
      - `hot`
      - `memory`
      - `library`
      - `distill`
      - bounded `semantic`
    - added canonical-source election for duplicate evidence
    - bounded fallback now happens only when the primary family underfills
    - packet diagnostics now report:
      - primary source family
      - fallback source
      - source read count
      - QMD query count
  - `src/commands/moon_assemble.rs`
    - assemble path now parses raw session once and reuses it for packet build
  - `src/commands/moon_context_engine.rs` and `src/cli.rs`
    - added `moon context-engine --sync-only`
    - added packet/source diagnostics and raw parse count reporting
- Plugin changes:
  - `assets/plugin/index.js`
    - added `runMoonContextSync()`
    - `afterTurn()` now uses `moon context-engine --sync-only`
  - `assets/plugin/index.test.ts`
    - added coverage proving `afterTurn()` uses sync-only mode
- Audit result:
  - no blocking implementation issues found after diff review and full
    validation
  - residual risk remains that routed source-family selection is lexical and can
    misroute ambiguous prompts, but fallback still preserves correctness
- Release packaging:
  - bumped version to `v1.2.0` in:
    - `Cargo.toml`
    - `Cargo.lock`
    - `assets/plugin/package.json`
    - `assets/plugin/index.js`
  - updated `CHANGELOG.md`
  - rebased the release commit onto upstream `v1.1.4` and `v1.1.5` before
    publishing
- Validation completed:
  - `cargo fmt --all`
  - `deno fmt mip.md handoff.md assets/plugin/index.js assets/plugin/index.test.ts`
  - `cargo test context_packet -- --nocapture`
  - `cargo test checkpoint_ -- --nocapture`
  - `deno test --allow-read --allow-write --allow-env --allow-run assets/plugin/index.test.ts`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test --all-targets --all-features`
  - `deno lint assets/plugin/index.js assets/plugin/index.test.ts`

## 2026-04-23 20:13 AEST

- Replaced `mip.md` with a new optimisation-focused control plan.
- The new MIP supersedes the earlier active-context packet rollout plan and
  shifts the current focus to reducing repeated work in Moon context assembly.
- New planning direction:
  - parse the raw session once per checkpoint
  - stop rescanning broad source trees on routine assembles
  - route each assemble pass to one primary source lane first
  - search one canonical source artifact first inside that lane where possible
  - use fallback lanes and QMD only when the primary source underfills
  - dedupe repeated evidence before packet section budgeting
  - split refresh-heavy sync work from the pre-dispatch hot path
- No runtime code changed in this pass; this was a planning-doc rewrite only.

## 2026-04-23 00:14 AEST

- Wrote a dedicated Moon assembly curator RCA and remediation plan in:
  - `docs/assembly-subagent-root-cause-plan.md`
- Plan contents cover:
  - primary root cause: recursive / re-entrant nested OpenClaw embedded runs
    from inside Moon `contextEngine.assemble`
  - secondary cause: curator over-coupling to full OpenClaw run/session/lock
    machinery plus session/transcript identity mismatch
  - root-cause fix plan: recursion guards and curator-session bypass
  - secondary fix plan: move curator toward a direct bounded rewrite path or a
    minimal child-run mode
  - additional additions: observability, safety fuse, release/operator notes,
    and validation checklist

## 2026-04-22 20:01 AEST

- Cut the next patch release as `v1.1.3` for the post-`1.1.2` `openai-codex`
  provider timeout hardening work.
- Release metadata aligned across:
  - `Cargo.toml`
  - `Cargo.lock`
  - `assets/plugin/package.json`
  - `assets/plugin/index.js`
- Updated `CHANGELOG.md` with the `1.1.3` release note for the `openai-codex`
  retry/timeout fix in Moon `cleanse` and `distill`.
- Validation completed:
  - `cargo fmt --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test --all-targets --all-features`
  - `deno fmt --check assets/plugin/index.js assets/plugin/index.test.ts assets/plugin/openclaw.plugin.json`
  - `deno lint assets/plugin/index.js assets/plugin/index.test.ts`
  - `deno test --allow-read --allow-write --allow-env --allow-run assets/plugin/index.test.ts`

## 2026-04-22 19:39 AEST

- Hardened Moon's `openai-codex` provider calls used by context-engine `cleanse`
  and `distill` against transient timeout and overload failures.
- Root cause from live investigation after the local OpenClaw timeout fix:
  - user-visible chats could succeed while adjacent `assemble` / `afterTurn`
    hooks still failed with `error decoding response body: operation timed out`
  - the failures were inside Moon's nested `openai-codex` HTTP call, not the old
    OpenClaw `contextEngineTimeoutMs=20000` kill path
- Code changes:
  - `src/moon/util.rs`
    - added shared helpers to classify retryable `openai-codex` statuses and
      errors plus simple backoff calculation
  - `src/moon/cleanse.rs`
    - raised the internal `openai-codex` request timeout to `90s`
    - added up to `3` attempts for transient transport, HTTP status, body-read,
      and empty-text failures
  - `src/moon/distill.rs`
    - mirrored the same `90s` timeout and `3`-attempt retry behavior for the
      `openai-codex` distill path
- Validation completed:
  - `cargo fmt --all`
  - `cargo test openai_codex`
  - `cargo test --test install_flow_test --test idempotency_test --test install_canonical_paths_test --test config_patch_test`

## 2026-04-22 19:03 AEST

- Prepared the OpenClaw context-engine timeout fix for release as `v1.1.2`.
- Release metadata aligned across:
  - `Cargo.toml`
  - `Cargo.lock`
  - `assets/plugin/package.json`
  - `assets/plugin/index.js`
- Updated `CHANGELOG.md` with the `1.1.2` release note for the managed
  `contextEngineTimeoutMs=120000` timeout fix.
- Validation completed:
  - `cargo fmt --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test --all-targets --all-features`
  - `deno fmt --check assets/plugin/index.js assets/plugin/index.test.ts assets/plugin/openclaw.plugin.json`
  - `deno lint assets/plugin/index.js assets/plugin/index.test.ts`
  - `deno test --allow-read --allow-write --allow-env --allow-run assets/plugin/index.test.ts`

## 2026-04-22 18:57 AEST

- Patched Moon install/upgrade behavior to prevent OpenClaw from timing out
  valid long-running `moon context-engine` runs.
- Root cause from live investigation:
  - OpenClaw Moon plugin defaulted `contextEngineTimeoutMs` to `20000`
  - real `openai-codex` compaction runs on long sessions completed in about
    `46-49s` in isolated end-to-end repros
  - OpenClaw surfaced the resulting timeout/signal kill as
    `moon context-engine exited with null`
- Code changes:
  - `src/openclaw/config.rs`
    - added managed `contextEngineTimeoutMs = 120000` to
      `ensure_plugin_runtime_config(...)`
  - `assets/plugin/index.js`
    - raised `DEFAULT_CONTEXT_ENGINE_TIMEOUT_MS` from `20_000` to `120_000`
  - docs updated in:
    - `README.md`
    - `BOOTSTRAP.md`
    - `assets/plugin/README.md`
  - install/config regression coverage updated in:
    - `tests/install_flow_test.rs`
    - `tests/idempotency_test.rs`
    - `tests/install_canonical_paths_test.rs`
    - `tests/config_patch_test.rs`

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
    `src/moon/openai_codex_auth.rs` so
    `cargo clippy --all-targets --all-features -- -D warnings` passes for the
    release build

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

- Rewrote `mip.md` as the next control-plan MIP for Moon active context assembly
  and memory-management performance.
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
    - packet artifact path is `$MOON_HOME/mcp/<session_id>.md`
    - local generation cache reuses the previous packet when the source corpus
      has not changed
  - `src/commands/moon_assemble.rs` and `src/commands/moon_context_engine.rs`
    now surface packet diagnostics
  - `src/commands/moon_status.rs` and `src/commands/moon_health.rs` now report
    the packet directory/state without treating a not-yet-created packet
    directory as a failure
  - added `context_packet` config to `src/moon/config.rs`, `moon.toml.example`,
    and state tracking in `src/moon/state.rs`
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

## 2026-04-21 00:32 AEST

- Renamed the default active-context packet directory from
  `$MOON_HOME/context-packets/` to `$MOON_HOME/mcp/`.
- Scope:
  - `src/moon/paths.rs` now resolves `context_packet_dir` to `$MOON_HOME/mcp`
  - packet-path tests and checkpoint assertions now expect `/mcp/<session>.md`
  - uninstall removes both the current `mcp/` directory and the legacy
    `context-packets/` directory
  - README/runbook/dev notes/MIP now reflect the new default path
- Rationale:
  - the directory contains Moon active context packets, not operator assembly
    artifacts
  - `mcp` is the shorter runtime path the operator asked to standardize on

## 2026-04-21 00:54 AEST

- Fixed the `moon update` / `moon verify --strict` false-negative path caused by
  transient OpenClaw doctor timeouts.
- Implementation:
  - `src/openclaw/gateway.rs` now resolves doctor timeout from
    `MOON_OPENCLAW_DOCTOR_TIMEOUT_SECS` with a default of 30s
  - `src/openclaw/doctor.rs` now classifies doctor results as:
    - ok
    - timed out
    - failed
  - `src/commands/verify.rs` now treats doctor timeout as an advisory detail
    instead of a hard verify issue
  - real doctor failures still fail strict verify
- Regression coverage:
  - added timeout advisory coverage in
    `tests/context_engine_slot_status_test.rs`
  - kept the existing failure-path test proving non-interactive doctor errors
    still fail strict verify and no interactive fallback is used
- Validation during this pass:
  - `cargo fmt --all`
  - `cargo test --test context_engine_slot_status_test -- --nocapture`
  - `deno fmt --check assets/plugin/index.js assets/plugin/index.test.ts assets/plugin/openclaw.plugin.json README.md docs/runbook.md`
  - `deno lint assets/plugin/index.js assets/plugin/index.test.ts`
  - `deno test --allow-read --allow-write --allow-env --allow-run assets/plugin/index.test.ts`
  - `cargo fmt --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test --all-targets --all-features`
  - `cargo run --quiet -- --allow-out-of-bounds install`
  - `cargo run --quiet -- verify --strict --json`

## 2026-04-21 01:18 AEST

- Updated the Moon OpenClaw plugin to use fast provider-specific defaults for
  the optional assembly curator subagent when gated mode is enabled and the
  operator omits an explicit model.
- New curator defaults:
  - `openai` -> `gpt-5.4-mini`
  - `openai-codex` -> `gpt-5.4-mini`
  - `google` / `gemini` -> `gemini-3.1-flash-lite-preview`
  - `anthropic` -> `claude-3-5-haiku-latest`
- The plugin also infers `openai` when the operator sets
  `assemblySubagentModel=gpt-5.4-mini` without a provider.
- Deliberately did not change Moon Rust cleanse / wisdom defaults in this pass.
  Current OpenAI two-level split remains:
  - fast pass: `gpt-4.1-mini`
  - stronger synthesis pass: `gpt-4.1`
- Release packaging:
  - version bump to `v1.0.15`
  - updated `CHANGELOG.md`, `Cargo.toml`, `Cargo.lock`,
    `assets/plugin/package.json`, and the plugin runtime version string
- Validation during release packaging:
  - `deno fmt --check assets/plugin/index.js assets/plugin/index.test.ts assets/plugin/openclaw.plugin.json assets/plugin/README.md CHANGELOG.md handoff.md`
  - `deno lint assets/plugin/index.js assets/plugin/index.test.ts`
  - `deno test --allow-read --allow-write --allow-env --allow-run assets/plugin/index.test.ts`
  - `cargo fmt --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test --all-targets --all-features`

## 2026-04-21 19:00 AEST

- Fixed watcher `syns` scheduling so the daily synthesis catches up later the
  same local day if Moon missed the exact trigger minute while the daemon was
  down or unhealthy.
- Tightened scheduled `syns` source selection:
  - watcher-triggered `syns` now always uses the previous completed local
    daily-memory file under `memory/<previous-day>.md`
  - it no longer falls back to the current day's daily-memory file
  - if the previous-day file is missing or empty, watcher-triggered `syns` skips
    and leaves `last_syns_trigger_epoch_secs` unchanged
- Updated watcher regression coverage:
  - added a catch-up test proving `syns` still runs later the same day after a
    missed midnight window
  - added a guard test proving scheduled `syns` skips instead of using the
    current day's daily-memory file when the previous-day file is missing
- Updated `README.md`, `docs/runbook.md`, and `docs/contracts.md` to document
  the catch-up rule and previous-day-only scheduled `syns` contract.
- Release packaging:
  - version bump to `v1.0.16`
  - updated `CHANGELOG.md`, `Cargo.toml`, `Cargo.lock`,
    `assets/plugin/package.json`, and the plugin runtime version string

## 2026-04-21 20:28 AEST

- Hardened Moon runtime secret handling for the `v1.1.0` release.
- Added `src/moon/fs_security.rs` and routed Moon secret-bearing file creation
  through private filesystem helpers.
- Runtime hardening scope:
  - `moon install` now repairs owner-only permissions for:
    - `$MOON_HOME/.env`
    - `$MOON_HOME/auth/`
    - `$MOON_HOME/auth/openai-codex.json`
    - `$MOON_HOME/logs/`
    - `$MOON_HOME/logs/audit.log`
    - `$MOON_HOME/logs/distill.audit.log`
  - managed OpenAI Codex auth persistence now writes through private file
    helpers instead of ambient default modes
  - Moon audit/distill audit paths now append through owner-only log handles
- Sanitized remote failure behavior:
  - OpenAI Codex OAuth/login, cleanse, and distill failure paths no longer
    include raw provider response bodies in CLI/audit error text
  - request ids are preserved when the backend returns them
- Verification contract:
  - shared `src/commands/status.rs` now enforces the runtime secret permission
    contract so `moon verify --strict` fails on insecure secret-bearing runtime
    paths
  - `src/commands/moon_status.rs` uses the same shared check for direct runtime
    diagnostics
- Added regression coverage for:
  - owner-only install/login artifacts
  - strict verify failure on insecure runtime secret permissions
  - sanitized OAuth/login and Codex cleanse failure output
- Updated docs for the new security contract:
  - `README.md`
  - `docs/runbook.md`
  - `docs/security_checklist.md`
  - `CHANGELOG.md`

## 2026-04-22 00:06 AEST

- Restored a standard CLI version flag for Moon.
- Implementation:
  - added Clap version metadata in `src/cli.rs` so Moon now supports:
    - `moon --version`
    - `moon -V`
- Added regression coverage in `tests/default_env_loading_test.rs` to prove the
  version flag works through the current env-loading bootstrap path.
- Updated `README.md` CLI global flags to include `--version` / `-V`.

## 2026-04-22 00:16 AEST

- Prepared the version-flag follow-up as `v1.1.1` so `moon update` on the stable
  channel can consume it.
- Release metadata aligned across:
  - `Cargo.toml`
  - `Cargo.lock`
  - `assets/plugin/package.json`
  - `assets/plugin/index.js`
- Updated `CHANGELOG.md` with the `1.1.1` release note for the restored standard
  CLI version flag.
