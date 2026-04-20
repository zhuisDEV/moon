# MIP: Moon Active Context Packet And Subagent-Assisted Retrieval

## Status

1. Proposed on `2026-04-20`.
2. Implemented on `2026-04-20`.
3. The implementation now exists in the repo:
   - hot projection refresh runs every checkpoint
   - Moon writes an active context packet under `$MOON_HOME/context-packets/`
   - the plugin injects that packet through the `messages` lane
   - the plugin can run the gated Moon curator subagent when configured
4. Verified against:
   - Moon `main` at `1b1254b5464a473f65336c3d97420a768526d61a`
   - OpenClaw `origin/main` at `94e2bf258d6ee35f4661c73bc3400c6bba52885a`
5. This MIP supersedes the previous `mip.md` prompt-boundary implementation
   record. That earlier scope is already complete and remains part of repo
   history.
6. This document is both the control plan and the implementation record for the
   active-context packet rollout.

## Verified Baseline

Current verified behavior:

1. Moon owns the short-lived active checkpoint controller:
   `record -> conditional cleanse -> assemble` in `src/moon/context_engine.rs`.
2. Moon plugin `assemble()` currently calls `moon context-engine`, but returns
   the incoming `messages` unchanged and does not emit routine
   `systemPromptAddition` in `assets/plugin/index.js`.
3. Moon plugin `compact()` appends transcript `compaction` entries from Moon
   `cleanse` output, and OpenClaw replays those as `compactionSummary`
   message-history context.
4. Moon still writes an operator assembly artifact under `$MOON_HOME/mce/`.
5. Moon `recall` is currently a direct QMD `query/search` wrapper, not an
   integrated active-window retrieval path.
6. Moon hot projection and immediate embed work only run when `cleanse` is
   triggered in the current checkpoint path.
7. OpenClaw context engines may return an ordered `messages` set plus optional
   `systemPromptAddition`.
8. OpenClaw also has a separate optional `active-memory` plugin that runs a
   bounded pre-reply subagent, but this MIP keeps that plugin disabled.
9. Moon-owned installs currently pin OpenClaw memory ownership off:
   `plugins.slots.memory = "none"` and
   `agents.defaults.memorySearch.enabled = false`.

## Problem

Moon currently has the right ownership boundary, but the active assembly path is
too thin and the searchable hot corpus is not fresh enough.

The concrete gaps are:

1. Routine `assemble()` does not add a Moon-owned active context packet to the
   model-facing message lane.
2. Moon has useful memory/search surfaces, but they are not yet integrated into
   routine active-window assembly:
   - latest `cleanse` summary
   - hot projection docs
   - library projection/docs
   - distill outputs
   - daily memory
   - durable `MEMORY.md`
   - QMD-backed embedded collections
3. Hot searchable state is refreshed only when `cleanse` runs, which makes
   active retrieval stale between compaction events.
4. Moon `recall` can search QMD, but that search is not yet part of the normal
   control loop.
5. If Moon jumps straight to a per-turn full subagent assembler, it will likely
   add latency, cost, and nondeterminism before the local retrieval layer is
   good enough.

## Goals

1. Improve Moon active context quality without returning to routine
   `systemPromptAddition`.
2. Keep OpenClaw `active-memory` disabled.
3. Keep Moon as the owner of active context retrieval and memory selection.
4. Use OpenClaw only as the host runtime for a bounded Moon-owned curator
   subagent when needed.
5. Prefer deterministic local retrieval and ranking first.
6. Add a subagent only as a bounded second-stage selector or summarizer.
7. Keep model-facing prompt content free of duplicate summaries and operator
   telemetry.
8. Improve performance by refreshing the hot corpus continuously and by caching
   expensive selection work.

## Non-Goals

1. Do not move final prompt-envelope ownership from OpenClaw to Moon.
2. Do not reintroduce routine `systemPromptAddition` for Moon assembly.
3. Do not enable OpenClaw `active-memory`.
4. Do not depend on OpenClaw `memory_search` or `memory_get`, because Moon-owned
   installs intentionally disable that lane.
5. Do not make a raw-transcript subagent the first retrieval stage.
6. Do not duplicate the latest `cleanse` summary if the same summary is already
   present in replayed `compactionSummary`.

## Decision

Moon will adopt a two-stage active assembly design:

1. Stage A: deterministic Moon-owned retrieval and packet building.
2. Stage B: optional bounded curator subagent hosted by OpenClaw, used only when
   gating conditions say the local packet is too broad or too ambiguous.

The model-facing context will travel in the `messages` lane, not in
`systemPromptAddition`.

The final design is:

1. Moon continues to write the operator assembly artifact for inspection.
2. Moon also produces a separate model-facing `active context packet`.
3. The Moon plugin reads that packet during `assemble()`.
4. The plugin returns an ordered `messages` set that includes the Moon packet in
   the message lane.
5. `systemPromptAddition` remains empty by default.
6. The latest `cleanse` summary continues to use the transcript compaction lane.
7. The subagent, when enabled, never searches the raw transcript directly. It
   only curates a bounded candidate set produced by Moon retrieval.

## Clean Boundary

### 1. Operator lane

Moon operator artifacts stay on disk and may include:

1. paths
2. timestamps
3. queue state
4. embed receipts
5. projection status
6. debug counters

These are not prompt-facing by default.

### 2. Message-history lane

Moon model-facing active context must use the `messages` lane.

That includes:

1. replayed `compactionSummary`
2. original user and assistant history retained for the turn
3. one Moon active context packet message when needed

### 3. System-prompt lane

Routine Moon `systemPromptAddition` remains unused.

Reserve it only for a future case where Moon truly needs a short,
high-priority dynamic instruction in system-prompt space.

### 4. Tools lane

The Moon assembly subagent must not assume OpenClaw memory tools are present.
Any retrieval it uses must come from Moon-owned artifacts or Moon-owned helper
commands.

## Target Architecture

### 1. Retrieval corpus

Moon active retrieval will read from these sources, in this order of trust:

1. latest replayable `cleanse` summary metadata
2. hot session projection
3. daily memory files under `$MOON_HOME/memory/`
4. durable `MEMORY.md`
5. library projection/docs
6. distilled artifacts and synthesis outputs
7. QMD semantic search results over Moon-managed embedded collections

Structured state such as pending embed collections, corpus generation, and cache
versioning may inform ranking and invalidation, but that raw telemetry must not
be dumped into the packet.

### 2. Hot corpus freshness

Moon must refresh the hot session projection on every checkpoint or after-turn
sync, not only when `cleanse` triggers.

That means:

1. `project` for the hot lane becomes routine and cheap.
2. bounded embed stays asynchronous and best-effort.
3. stale hot-search windows stop being tied to compaction frequency.

### 3. Active context packet

Moon will build a separate model-facing packet with deterministic section order.

Suggested packet structure:

1. `# Moon Active Context`
2. `## Current Goal`
3. `## Active Work`
4. `## Relevant Memory`
5. `## Open Items`
6. `## Evidence`

Rules:

1. fixed heading order
2. omit empty sections deterministically
3. no YAML frontmatter in the model-facing packet
4. no absolute local paths unless directly needed for action
5. no raw queue counters or embed receipts
6. no duplicated `cleanse` text already present in the current replay window
7. one packet per assemble pass

### 4. Packet placement

Moon plugin `assemble()` will return an ordered message set that inserts the
Moon packet immediately before the current user prompt when the last message is a
user turn. If there is no trailing user message, prepend the packet at the start
of the assembled sequence.

The implementation may use a synthetic assistant-context message for this entry,
because the OpenClaw context-engine contract guarantees ordered messages but does
not provide a separate dedicated packet role.

### 5. Subagent role

The subagent is not the retriever. The subagent is the curator.

It may:

1. select the best candidate snippets
2. collapse duplicates
3. resolve minor conflicts
4. produce a shorter packet from a bounded candidate set

It may not:

1. search raw transcript history directly
2. invent missing evidence
3. rewrite the packet boundary into system-prompt space
4. duplicate the latest `cleanse` summary verbatim

### 6. Subagent host

The subagent will run through OpenClaw embedded-agent runtime from the Moon
plugin, similar in shape to OpenClaw active-memory, but owned by Moon and fed by
Moon retrieval output.

Moon CLI remains the control plane. OpenClaw provides only the bounded embedded
run host.

### 7. Gating

The subagent will run only when one or more of these are true:

1. candidate count exceeds the local packet budget
2. candidate token estimate exceeds the packet budget
3. top candidates are semantically redundant
4. top candidates conflict across sources
5. the current prompt is recall-heavy and local ranking confidence is low

If none of those are true, Moon uses the deterministic local packet directly.

### 8. Caching

Moon should cache both:

1. local retrieval results
2. final curator subagent outputs

Cache key inputs:

1. session id
2. normalized current prompt hash
3. latest user-turn hash
4. corpus generation fingerprint
5. packet config version

Invalidate when:

1. latest `cleanse` summary changes
2. hot projection changes
3. daily memory or `MEMORY.md` changes
4. library/distill artifacts change
5. embedded corpus generation changes
6. packet config changes

## Configuration Plan

### 1. Moon config

Add a new `moon.toml` section:

```toml
[context_packet]
enabled = true
max_candidates = 12
max_packet_tokens = 1400
dedupe_cleanse = true
include_hot_projection = true
include_daily_memory = true
include_memory_file = true
include_library = true
include_distill = true
include_qmd = true
```

This config owns Moon retrieval and packet-building behavior.

### 2. Plugin config

Add Moon plugin config keys for the subagent host path:

1. `assemblySubagentMode = "off" | "gated" | "always"`
2. `assemblySubagentTimeoutMs`
3. `assemblySubagentCacheTtlMs`
4. optional `assemblySubagentModel`
5. optional `assemblySubagentModelFallback`

Recommended default:

1. `assemblySubagentMode = "gated"`
2. inherit current session model when no explicit model is set
3. short timeout
4. short TTL cache

## Implementation Plan

### Phase 0: Baseline And Contracts

Goal:

1. Define the new packet contract before code changes spread across Rust and the
   plugin.

Concrete work:

1. Rewrite this `mip.md` as the control plan.
2. Add code comments and docs notes clarifying:
   - operator artifact vs model-facing packet
   - messages lane vs `systemPromptAddition`
   - Moon-owned subagent vs OpenClaw active-memory
3. Add packet/report field names to the implementation checklist before coding.

Completion criteria:

1. Coding team has one agreed contract for packet shape, placement, and
   ownership.

### Phase 1: Fresh Hot Corpus On Every Turn

Goal:

1. Make the searchable hot corpus current enough for routine active assembly.

Concrete work:

1. Update `src/moon/context_engine.rs` so hot projection refresh is not gated on
   `should_cleanse`.
2. Keep `cleanse` conditional, but run hot-lane `project` on every checkpoint or
   after-turn sync.
3. Mark embed maintenance pending when the hot projection changes.
4. Keep bounded embed asynchronous and failure-tolerant.
5. Add status/audit details showing hot projection freshness separately from
   `cleanse`.

Primary files:

1. `src/moon/context_engine.rs`
2. `src/moon/project.rs`
3. `src/moon/embed.rs`
4. `src/commands/moon_context_engine.rs`

Completion criteria:

1. Hot projection changes on every turn where raw transcript changed.
2. Searchable hot corpus is no longer stale between compaction events.

### Phase 2: Deterministic Moon Retriever

Goal:

1. Build a local retriever that can assemble a bounded candidate set without a
   model call.

Concrete work:

1. Add a new Moon retrieval module, for example:
   - `src/moon/context_packet.rs`
   - `src/moon/context_retrieval.rs`
2. Retrieve and rank evidence from:
   - latest `cleanse`
   - hot projection
   - daily memory
   - `MEMORY.md`
   - library docs
   - distill outputs
   - QMD semantic hits
3. Implement deterministic dedupe and ordering.
4. Build a `corpus_generation_fingerprint` for cache invalidation.
5. Write the candidate set and final local packet as separate artifacts.

Primary files:

1. `src/moon/context_packet.rs` or equivalent new module
2. `src/moon/context_engine.rs`
3. `src/moon/qmd.rs`
4. `src/moon/paths.rs`
5. `src/commands/moon_context_engine.rs`

Completion criteria:

1. Moon can build a bounded local packet without a subagent.
2. Packet content is deterministic for identical logical inputs.

### Phase 3: Packet Artifact And Plugin Message Injection

Goal:

1. Move from operator-only assembly to a dual-output model:
   operator artifact plus model-facing packet.

Concrete work:

1. Extend the checkpoint report with:
   - `context_engine.packet_path`
   - `context_engine.packet_mode`
   - `context_engine.packet_candidate_count`
   - `context_engine.packet_cache_hit`
2. Keep `$MOON_HOME/mce/` for the operator artifact.
3. Add a separate packet artifact directory, for example `$MOON_HOME/mcp/` or an
   equivalent Moon-specific path.
4. Update `assets/plugin/index.js`:
   - read packet artifact
   - inject packet through returned `messages`
   - keep `systemPromptAddition` empty
5. Preserve the current compaction path unchanged.

Primary files:

1. `src/moon/assemble.rs`
2. `src/moon/context_engine.rs`
3. `assets/plugin/index.js`
4. `assets/plugin/index.test.ts`
5. `assets/plugin/openclaw.plugin.json`

Completion criteria:

1. Routine Moon `assemble()` now returns a Moon packet in the messages lane.
2. Routine Moon `assemble()` still returns no `systemPromptAddition`.

### Phase 4: Gated Curator Subagent

Goal:

1. Add a bounded Moon-owned subagent for difficult retrieval-selection cases.

Concrete work:

1. In `assets/plugin/index.js`, add a gated branch after local retrieval:
   - if packet is already good enough, skip subagent
   - if gating says packet is too broad or ambiguous, run subagent
2. Use OpenClaw embedded-agent runtime with:
   - lightweight bootstrap
   - short timeout
   - no message tool
   - no dependency on OpenClaw memory tools
3. Feed the subagent only:
   - current prompt
   - bounded candidate snippets
   - Moon packet schema instructions
4. Cache the subagent result with a short TTL keyed by prompt and corpus
   generation.
5. Fall back to the deterministic local packet on timeout or failure.

Primary files:

1. `assets/plugin/index.js`
2. `assets/plugin/index.test.ts`
3. `assets/plugin/README.md`

Completion criteria:

1. Subagent path is bounded, cached, and optional.
2. Timeout or failure never breaks Moon assembly.

### Phase 5: Duplicate Control, Telemetry, And Verification

Goal:

1. Make the new path observable and safe to operate.

Concrete work:

1. Add duplicate suppression against the latest replayable `cleanse` summary.
2. Add packet diagnostics:
   - retrieval source counts
   - packet token estimate
   - packet cache hit
   - subagent used
   - subagent latency
   - fallback reason
3. Update `moon status` and `moon verify` with packet diagnostics where useful.
4. Add a stable packet hash for debugging deterministic drift.

Primary files:

1. `src/commands/moon_status.rs`
2. `src/commands/verify.rs`
3. `assets/plugin/index.js`
4. `src/moon/context_packet.rs`

Completion criteria:

1. Operators can tell whether Moon used local packeting or curator subagent.
2. Duplicate `cleanse` injection is blocked by tests.

### Phase 6: Docs, Rollout, And Cleanup

Goal:

1. Finish the feature without leaving the old mental model half alive.

Concrete work:

1. Update:
   - `README.md`
   - `docs/runbook.md`
   - `docs/contracts.md`
   - `assets/plugin/README.md`
   - `handoff.md`
2. Mark the old operator-only description of `assemble` as outdated and
   replace it with the dual-output model.
3. Keep the earlier prompt-boundary rules intact in docs:
   - no routine `systemPromptAddition`
   - `cleanse` remains in `compactionSummary`
4. Remove or rewrite stale comments and dead compatibility paths that no longer
   match the final design.

Completion criteria:

1. Repo docs describe the shipped architecture instead of the transition state.

## Test Plan

### Rust tests

1. hot projection updates every checkpoint even when `cleanse` does not run
2. packet retrieval produces deterministic output for fixed inputs
3. packet dedupe removes repeated evidence across hot/library/memory sources
4. latest `cleanse` summary is omitted from packet when already represented in
   replayable compaction state
5. corpus generation fingerprint changes only when relevant sources change

### Plugin tests

1. `assemble()` injects the Moon packet through `messages`
2. `assemble()` still returns no `systemPromptAddition`
3. subagent path is skipped when local packet is within threshold
4. subagent path runs when candidate budget or ambiguity threshold is exceeded
5. subagent timeout falls back to the local packet
6. subagent cache hits skip repeated embedded runs

### End-to-end tests

1. long session without `cleanse` still gets fresh hot-context retrieval
2. post-`cleanse` session does not receive duplicate summary content
3. memory edits in `$MOON_HOME/memory/` and `MEMORY.md` influence the next packet
4. QMD-backed semantic hits can enter the packet through local retrieval
5. OpenClaw active-memory remains disabled during the entire flow

## Rollout Plan

1. Ship Phase 1 and Phase 2 first behind config.
2. Validate deterministic packet quality before turning on the subagent path.
3. Ship Phase 3 next so the local packet reaches the messages lane.
4. Ship Phase 4 behind `assemblySubagentMode = "gated"`.
5. Make gated mode the default only after latency and duplicate-control metrics
   are acceptable.

## Risks

1. Synthetic assistant-context packet placement could bias some models more than
   expected.
2. If the hot projection gets too verbose, local retrieval quality will degrade
   even before the subagent runs.
3. If the subagent is allowed to see too much candidate text, it will recreate
   the same latency problem as a full per-turn assembler.
4. If duplicate suppression against `compactionSummary` is weak, Moon will start
   repeating itself.

## Acceptance Criteria

This MIP is complete only when all of the following are true:

1. Moon builds and uses a model-facing active context packet in the `messages`
   lane.
2. Routine Moon `systemPromptAddition` remains unused.
3. Hot projection freshness is no longer gated on `cleanse`.
4. Moon retrieval covers hot, memory, library, distill, and QMD-backed semantic
   sources.
5. A bounded curator subagent exists and runs only when gated.
6. Timeout or failure falls back cleanly to deterministic local packeting.
7. Duplicate `cleanse` injection is blocked.
8. Docs, tests, and operator diagnostics all reflect the new architecture.
