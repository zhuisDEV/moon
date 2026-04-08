# MIP Proposal: Cache-Aware Moon Context Assembly

## Status

1. Proposed on `2026-04-08`.
2. Superseded by the implemented `mip.md` prompt-boundary cleanup on
   `2026-04-08`.
3. This proposal is retained as design history for the earlier cache-aware
   assembly exploration.
4. This proposal is based on two inputs:
   - verified Moon -> OpenClaw -> provider flow from
     `openclaw-moon-context-flow-report.md`
   - external Claude Code research notes captured in `CC_research.md`
5. This is a Moon proposal, not an OpenClaw ownership rewrite.

## Compatibility Baseline

This proposal was checked against the current source-of-truth repos on
`2026-04-08`:

1. Moon `origin/main` at `c22d84eaa44c2e6aecfbde66ea599198adbe04f5`
2. OpenClaw local `main`, tracking `zhuisDEV/main`, at
   `5fb6aeaf86c7bcab1984c9719888e4987dfea139`

Relevant verified seams:

1. Moon assembly rendering currently happens in `src/moon/assemble.rs`.
2. Moon plugin `assemble()` currently reads the full assembly artifact and feeds
   it through `trimAssemblyText(...)` before returning `systemPromptAddition`.
3. Moon plugin already has a `stripFrontMatter()` helper, but it is currently
   used in compaction normalization rather than the main assemble path.
4. OpenClaw still accepts `messages` plus optional `systemPromptAddition` from
   context engines.
5. OpenClaw inserts `systemPromptAddition` after `OPENCLAW_CACHE_BOUNDARY` when
   that boundary exists.

## Verified Current Flow

1. Moon currently assembles a markdown artifact under
   `$MOON_HOME/mce/<session>.md`.
2. The Moon OpenClaw plugin reads that artifact and returns it as
   `systemPromptAddition` during `assemble(...)`.
3. OpenClaw still owns:
   - final system prompt structure
   - tool definitions and tool guidance
   - provider stream selection
   - provider payload shaping
4. OpenClaw inserts `systemPromptAddition` after its cache boundary when the
   boundary exists.
5. Therefore:
   - Moon does not own the final prompt envelope.
   - Moon does directly influence the dynamic prompt suffix that reaches the
     provider.

## Problem

Moon currently emits one artifact that serves too many purposes at once.

Today the assembled text includes both:

1. model-facing context
2. operator/debug metadata
3. volatile telemetry fields
4. raw-path provenance fields

The verified current assembly format includes fields such as:

- `assembled_at_epoch_secs`
- `session_id`
- `raw_source_path`
- `cleanse_summary_path`
- embedding index counters and pending collections
- hot projection paths
- raw context excerpt

That entire markdown payload is then trimmed and used as `systemPromptAddition`.

This creates three concrete problems:

### 1. Moon is adding avoidable volatility to the provider-facing prompt

Even though OpenClaw preserves its stable prefix with `OPENCLAW_CACHE_BOUNDARY`,
Moon still controls part of the dynamic suffix. If Moon injects timestamps,
absolute paths, index counters, and other turn-volatile metadata, the dynamic
suffix churns more than necessary.

### 2. Model-facing context is mixed with operator telemetry

Fields that are useful for debugging or provenance are not automatically useful
for the model. Combining them in one block spends token budget on data that may
not improve the model's behavior.

### 3. Moon's assembly contract is not yet shaped around stable, selective context

The Claude Code research strongly suggests that high-quality agent systems win
through:

- layered prompt assembly
- stable reusable prefixes
- precise memory selection
- compaction that preserves semantic units
- keeping tooling/runtime ownership separated from upstream context producers

Moon's current assembly is useful, but it is still closer to a diagnostic dump
than a cache-aware context contract.

## Compatibility Findings

### 1. The proposal fits the current OpenClaw contract

No OpenClaw API change is required for this proposal. The existing context
engine contract already accepts a smaller, more selective `systemPromptAddition`
while leaving final prompt ownership in OpenClaw.

### 2. The proposal fits the current Moon architecture

Moon already separates:

1. assembly generation
2. disk artifact writing
3. plugin-side prompt injection

That means the proposal can be implemented by adding a model-facing renderer and
changing only what the plugin returns to OpenClaw, without removing the richer
on-disk artifact.

### 3. There is already a low-risk first implementation slice

The existing `stripFrontMatter()` helper in the plugin provides a concrete,
incremental starting point for Phase 1. It is not the whole solution, but it
shows the codebase already has a natural seam for removing non-model-facing
material before prompt injection.

### 4. One refinement is required for Phase 3

The current plugin `trimAssemblyText()` function still performs simple head/tail
character clipping. That is compatible with today's runtime, but it does not
satisfy the proposal's semantic-preservation goal by itself.

So if Phase 3 is implemented, Moon will need one of these:

1. upstream budgeting that avoids blind clipping
2. a new structured compactor for model-facing sections
3. section-aware truncation instead of raw character slicing

### 5. Existing status and verify surfaces are sufficient

Moon already has `status` and `verify` commands with detailed diagnostics. That
means the proposed volatility checks and model-facing hash reporting fit the
current command surface rather than requiring a brand-new operator command.

## Proposal

Moon should split its current single assembly artifact into two distinct
outputs:

1. `operator artifact`
   - full provenance and diagnostic record
   - written to disk for debugging and inspection
2. `model-facing addition`
   - a smaller, stable, deterministic text block intended specifically for
     `systemPromptAddition`

This keeps the existing Moon -> OpenClaw contract intact while improving the
quality of the text Moon contributes downstream.

## Proposed Changes

### 1. Introduce a dedicated model-facing renderer

Moon should stop reusing the full assembly artifact as the prompt addition.
Instead, it should render a separate model-facing section.

That section should:

1. exclude YAML frontmatter entirely
2. exclude absolute file paths unless they are directly action-relevant
3. exclude wall-clock timestamps such as `assembled_at_epoch_secs`
4. exclude embedding-index counters and queue telemetry unless they materially
   affect the agent's next action
5. exclude storage-oriented provenance fields that are only useful for humans

The full artifact can still exist on disk for operator inspection.

### 2. Define a stable section schema for the model-facing addition

Moon should produce the model-facing addition in a deterministic, byte-stable
layout with fixed section ordering.

Suggested section order:

1. `## Moon Context Contract`
   - brief statement of what Moon is providing and what it is not providing
2. `## Stable Project Constraints`
   - only durable constraints extracted from cleanse/project memory
3. `## Relevant Working Memory`
   - distilled, high-signal items for the current task
4. `## Current Session Delta`
   - only the minimum volatile turn-specific context needed now
5. `## Raw Excerpt`
   - only when necessary, and budgeted aggressively

Rules:

1. Omit entire sections when empty, but use deterministic omission behavior.
2. Normalize whitespace and bullet formatting.
3. Avoid reordering equal-priority items between runs.
4. Keep field names and headings stable across turns.

### 3. Keep volatile telemetry out of the model-facing path

The current `Embedding Index Anchor` block is useful operationally, but it is
not obviously model-useful in its current raw form.

Proposal:

1. keep the full embedding anchor only in the operator artifact
2. reduce model-facing embedding state to a tiny semantic status when needed,
   for example:
   - `embedding status: ready`
   - `embedding status: pending recent session projection`
3. include no counts, paths, or collection names unless the model must act on
   them directly

The same rule should apply to session IDs, source paths, and timestamp fields.

### 4. Make Moon prefer distilled context over raw transcript bulk

The new Claude Code research and the verified Moon/OpenClaw split both point to
precision over volume.

Proposal:

1. prefer cleanse/project summaries over long raw transcript excerpts
2. include raw excerpt material only when it contributes information not already
   captured in distilled summaries
3. budget raw excerpt separately from stable constraints and working memory
4. when truncation is necessary, preserve semantic groups instead of clipping at
   arbitrary character boundaries

This does not require Moon to mimic Claude Code internals. It only means Moon
should become more selective and more structured.

### 5. Add explicit stability and cache-drift checks

Moon should gain checks that validate whether the generated model-facing
addition is unnecessarily volatile.

Suggested verification rules:

1. fail or warn if the model-facing addition contains:
   - epoch timestamps
   - absolute local paths
   - queue counters
   - pending collection lists
   - raw operator telemetry blocks
2. report a stable hash of the model-facing addition in `moon status`
3. report when only operator metadata changed but the model-facing addition also
   changed unexpectedly
4. keep a regression test that the same logical input produces byte-identical
   model-facing output

### 6. Keep ownership boundaries intact

This proposal does **not** move final prompt ownership from OpenClaw to Moon.

Moon should continue to own:

1. context selection
2. checkpointing and cleanse decisions
3. context assembly
4. optional compaction ownership when configured

OpenClaw should continue to own:

1. final system prompt structure
2. tool inventory and tool prompt injection
3. cache-boundary placement
4. provider selection and request shaping

This ownership split is already verified and should remain explicit.

## Proposed Deliverables

### Phase 1: Contract split

1. Add a new Moon renderer for model-facing prompt text.
2. Keep the current full artifact on disk for provenance/debugging.
3. Update the plugin so `systemPromptAddition` uses the new model-facing text,
   not the raw artifact.
4. As an incremental guardrail, stop sending frontmatter through the assemble
   path even before the full renderer split is complete.

### Phase 2: Stable schema

1. Introduce fixed headings and ordering for the model-facing addition.
2. Remove volatile metadata from that path.
3. Normalize whitespace and omission rules.

### Phase 3: Context quality improvements

1. Prefer distilled summaries over raw excerpt bulk.
2. Budget raw excerpt separately.
3. Replace blind head/tail clipping with section-aware or semantic-unit-aware
   truncation for the model-facing path.

### Phase 4: Diagnostics and verification

1. Add status/verify reporting for model-facing volatility.
2. Add regression tests for byte stability and omission behavior.
3. Add docs explaining the split between operator artifact and model-facing
   addition.

## Non-Goals

1. Reimplement Claude Code architecture wholesale.
2. Move tool prompt ownership into Moon.
3. Make Moon responsible for provider transport or final prompt wrapping.
4. Turn Moon into a general-purpose memory engine for OpenClaw internals.

## Acceptance Criteria

1. Moon no longer sends frontmatter, timestamps, raw paths, or full embedding
   telemetry in `systemPromptAddition`.
2. The same logical Moon state produces byte-stable model-facing output.
3. Operator-facing provenance detail is still preserved on disk.
4. `moon status` or `moon verify` can detect volatility leaks into the
   model-facing addition.
5. The plugin contract with OpenClaw remains compatible.
6. The resulting model-facing addition is smaller and more selective than the
   current full assembly artifact.

## Risks And Tradeoffs

### 1. Risk: removing too much detail

If Moon strips too aggressively, the model may lose context that was useful.

Mitigation:

1. keep the full operator artifact on disk
2. roll out the model-facing contract in phases
3. add regression tests around known useful context categories

### 2. Risk: overfitting to external Claude Code claims

The Claude Code research is useful, but it is not a spec for Moon.

Mitigation:

1. use the research as directional input only
2. ground Moon changes in the already verified Moon/OpenClaw boundary
3. require code-level evidence before copying any Claude-specific mechanism

### 3. Risk: duplicated compaction logic with OpenClaw

Moon already owns compaction in some setups, but OpenClaw owns final prompt
assembly.

Mitigation:

1. keep Moon focused on context quality and structure
2. avoid duplicating provider-facing prompt logic that belongs in OpenClaw

## Why This Proposal Fits The Verified Architecture

1. It respects the verified ownership boundary:
   - Moon = context producer
   - OpenClaw = final prompt assembler and transport owner
2. It uses the Claude Code research in the right place:
   - not as a clone target
   - as evidence that prompt stability, selective memory, and layered context
     matter
3. It addresses a real verified gap in Moon today:
   - the same artifact currently serves both operator/debug and model-facing
     roles
   - those roles should be separated

## Bottom Line

Moon should not try to become the final prompt owner. It should become a better
upstream context producer.

The next MIP-worthy improvement is therefore:

- keep Moon's disk artifact rich for humans
- make Moon's `systemPromptAddition` smaller, more stable, more selective, and
  explicitly model-facing
- let OpenClaw continue to own the final prompt envelope and provider path
