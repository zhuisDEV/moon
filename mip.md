# MIP: Moon Cleanse Summary Lane And Prompt Boundary

## Status

1. Proposed on `2026-04-08`.
2. Implemented on `2026-04-08`.
3. Verified against:
   - Moon `main` at `c22d84eaa44c2e6aecfbde66ea599198adbe04f5`
   - OpenClaw `origin/main` at `a44a26f0a0a4`
4. Validated in the Moon workspace with:
   - `cargo test --quiet`
   - `deno test --allow-read --allow-write --allow-env --allow-run assets/plugin/index.test.ts`
5. All phases in this MIP are complete for the current scope.
6. This MIP replaces the previous contents of `mip.md` and now serves as the
   implementation record for the prompt-boundary cleanup.

## Verified Baseline

From current code:

1. OpenClaw owns final system prompt assembly.
2. OpenClaw inserts `OPENCLAW_CACHE_BOUNDARY` into its system prompt.
3. Context engines may return:
   - `messages`
   - optional `systemPromptAddition`
4. OpenClaw inserts `systemPromptAddition` **after** the cache boundary, inside
   the dynamic system-prompt suffix.
5. OpenClaw strips the boundary marker before transport.
6. OpenClaw replay turns can include `compactionSummary` as message-history
   context.
7. Moon plugin compaction already appends a transcript `compaction` entry based
   on Moon `cleanse` output, which OpenClaw can replay as `compactionSummary`.

## Terminology

1. In Moon terminology, `cleanse` is the compaction stage.
2. In OpenClaw terminology, the replayed compaction summary appears as
   `compactionSummary` in message-history context.
3. For this MIP:
   - `cleanse summary` = Moon compaction summary
   - `operator artifact` = Moon on-disk assembly/debug artifact
   - `model-facing prompt` = system prompt + messages + tools actually sent to
     the provider

## Problem Before Implementation

Moon currently sends the wrong thing down the wrong lane.

Before implementation:

1. Moon builds a rich assembly artifact.
2. The Moon plugin injects that artifact into OpenClaw as
   `systemPromptAddition`.
3. That artifact contains operator/debug material such as:
   - timestamps
   - raw and cleanse paths
   - embedding/index counters
   - pending collection names
   - raw excerpts

This creates four problems:

1. Moon routine context lives in dynamic system-prompt space instead of the
   verified message-history compaction lane.
2. Model-facing prompt content duplicates operational context that already has a
   better representation path through `compactionSummary`.
3. Indexing and operator receipts are exposed to the model even when they are
   not action-relevant.
4. Dynamic churn is higher than necessary, even before any provider-specific
   cache optimization work.

## Decision

Moon will adopt the clean boundary design below.

1. Moon `cleanse` summaries are the only Moon summary content that should enter
   model-facing context for now.
2. Moon `cleanse` summaries should travel through the verified message-history
   compaction lane:
   - Moon plugin appends transcript `compaction` entries
   - OpenClaw replays them as `compactionSummary`
3. Moon should stop using `systemPromptAddition` for routine context assembly.
4. Moon should keep `systemPromptAddition` reserved for a future case where a
   short, high-priority dynamic instruction truly belongs in system-prompt
   space.
5. Indexing, embed, projection, and operator receipts should stay out of the
   model-facing prompt by default.
6. No provider-specific cache optimization is part of this MIP. The goal is a
   correct generic boundary first.

## Clean Boundary Definition

### 1. Static prompt

Static prompt means everything above `OPENCLAW_CACHE_BOUNDARY`.

This is OpenClaw-owned prompt material that should stay stable unless stable
inputs change.

Examples:

1. OpenClaw core prompt sections
2. tool guidance text in the system prompt
3. stable provider prompt contribution
4. stable injected workspace files
5. stable runtime guidance and safety text

### 2. Dynamic prompt

Dynamic prompt means everything below `OPENCLAW_CACHE_BOUNDARY`.

Examples:

1. dynamic project context files
2. `extraSystemPrompt`
3. provider dynamic suffix
4. heartbeat/runtime dynamic sections
5. `systemPromptAddition` when a context engine returns it

### 3. Message-history context

This is not system-prompt space.

Examples:

1. `messages` returned by the context engine
2. replayed `compactionSummary`
3. normal user/assistant/toolResult history

### 4. Tools

Tools are separate structured definitions, not part of the static/dynamic
system-prompt split.

## When Static Prompt May Change

Static prompt should change only when stable inputs change, such as:

1. OpenClaw hardcoded prompt text changes
2. stable injected workspace/bootstrap files change
3. stable provider prompt contribution changes
4. tool inventory or tool guidance changes
5. prompt mode changes
6. stable workspace/sandbox/config inputs used above the boundary change

Moon checkpointing, `cleanse`, indexing churn, timestamps, receipt counters, and
other operational telemetry are **not** valid reasons to mutate the static
prompt prefix.

## Non-Goals

1. Do not move final prompt ownership from OpenClaw to Moon.
2. Do not add provider-specific cache-control logic in Moon.
3. Do not treat `compactionSummary` as a generic carrier for arbitrary operator
   telemetry.
4. Do not inject indexing receipts into model-facing prompt context by default.
5. Do not require OpenClaw code changes for the first implementation slice.

## Practical Implementation Plan

### Phase 1: Stop routine `systemPromptAddition` injection (Completed)

Goal:

1. Remove Moon's current prompt duplication immediately.

Concrete work:

1. Update `assets/plugin/index.js`:
   - keep calling `runMoonContextEngine(...)`
   - stop converting `output.assemblyText` into `systemPromptAddition`
   - return assembled `messages` and `estimatedTokens` only
2. Keep `moon context-engine` side effects intact:
   - record checkpointing
   - optional `cleanse`
   - operator artifact writing
   - after-turn sync behavior
3. Remove any now-dead assemble-path trimming logic if no longer used.

Expected result:

1. Normal Moon assembly no longer mutates OpenClaw dynamic system-prompt text.
2. The model no longer sees the rich Moon assembly artifact as prompt text.

### Phase 2: Make the `cleanse` summary lane authoritative (Completed)

Goal:

1. Use the verified message-history lane for Moon compaction output.

Concrete work:

1. Keep the current plugin compaction path that appends a transcript
   `compaction` entry from Moon `cleanse` output.
2. Keep `stripFrontMatter(...)` and summary normalization for the compaction
   path.
3. Update code comments and naming where useful so it is explicit that:
   - Moon `cleanse` == compaction summary source
   - OpenClaw replay turns convert that into `compactionSummary`
4. Do not duplicate the same `cleanse` summary in `systemPromptAddition`.

Expected result:

1. After compaction, Moon summary context reaches the model via
   `compactionSummary`, not via dynamic system-prompt injection.

### Phase 3: Keep the operator artifact, but mark it as operator-only (Completed)

Goal:

1. Preserve the current Moon debugging surface without sending it to the model.

Concrete work:

1. Keep `src/moon/assemble.rs` writing the on-disk artifact under
   `$MOON_HOME/mce/<session>.md`.
2. Treat that artifact as operator/debug state only.
3. Keep current status, audit, and path reporting that depends on the written
   artifact.
4. Make the operator-only intent explicit in docs and, where useful, inline
   comments.

Expected result:

1. Operators keep the current Moon inspection artifact.
2. Provider-facing prompt no longer depends on that artifact.

### Phase 4: Keep indexing and maintenance receipts out of prompt context (Completed)

Goal:

1. Eliminate non-actionable operational noise from model-facing payloads.

Concrete work:

1. Do not inject:
   - embedding counters
   - pending collection names
   - projection paths
   - timestamps
   - receipt summaries
2. Leave those signals in:
   - Moon operator artifact
   - logs
   - status/verify surfaces
3. If a future use case truly needs prompt-facing dynamic instructions, evaluate
   that separately and intentionally before using `systemPromptAddition`.

Expected result:

1. Moon prompt behavior is summary-driven, not telemetry-driven.

### Phase 5: Test plan (Completed)

Required Moon test updates:

1. `assets/plugin/index.test.ts`
   - add a regression that successful Moon `assemble()` returns no
     `systemPromptAddition`
   - keep compaction tests that verify transcript `compaction` entry creation
2. Rust tests
   - keep verifying the operator artifact is still written
   - keep verifying `cleanse` content remains in the artifact for operator
     inspection
3. Add a no-duplication assertion:
   - routine Moon assemble path does not inject summary text into system prompt
   - compaction path still yields transcript compaction summary behavior

Validation commands:

1. `cargo test --quiet`
2. `deno test --allow-read --allow-write --allow-env --allow-run assets/plugin/index.test.ts`

### Phase 6: Documentation updates (Completed)

Update the Moon docs to match the new boundary:

1. `README.md`
2. `docs/contracts.md`
3. `assets/plugin/README.md`
4. `dev-notes.md`
5. `handoff.md`

## File-Level Work List

Primary Moon files for the first implementation slice:

1. `assets/plugin/index.js`
2. `assets/plugin/index.test.ts`
3. `README.md`
4. `docs/contracts.md`
5. `assets/plugin/README.md`

Likely no required OpenClaw code change for Phase 1 through Phase 4, because the
needed boundary and `compactionSummary` replay behavior already exist upstream.

## Acceptance Criteria

1. Normal Moon `assemble` no longer returns routine `systemPromptAddition`.
2. Moon `cleanse` remains the Moon compaction stage.
3. Moon compaction summaries reach the model only through the transcript
   compaction lane and replay as `compactionSummary`.
4. No Moon summary is duplicated in both dynamic system-prompt text and
   message-history context.
5. Indexing/embed/projection receipts do not enter model-facing prompt context
   by default.
6. Moon operator artifact still exists on disk for inspection.
7. Moon test suite and plugin test suite pass after the change.

## Release Note Shape

For the implementation release, the release note should say:

1. Moon no longer injects its rich assembly artifact into OpenClaw's system
   prompt during normal runs.
2. Moon `cleanse` summaries now rely on OpenClaw's existing compaction-summary
   message-history path.
3. Operator/debug assembly artifacts remain available on disk.
