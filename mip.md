# MIP: Moon Context Assembly Optimisation Through Incremental And Single-Source Retrieval

## Status

1. Proposed on `2026-04-23`.
2. Replaces the previous `mip.md`, whose active-context packet rollout is
   already implemented.
3. Verified planning baseline against Moon `main` at
   `58b704d65a46fc345ce350004fc0f570a120f22c`.
4. This document is a control plan only. None of the optimisation work below is
   implemented yet.

## Why This MIP Exists

Moon now has the correct ownership boundary for active context assembly, but the
hot path still repeats too much work.

The current issue is not primarily packet shape. The issue is that Moon often:

1. parses the same raw session more than once in one checkpoint
2. rescans the same markdown trees on routine assembles
3. searches several source families even when one source is enough
4. pulls repeated evidence from multiple places and dedupes only after the work
   has already been done
5. keeps QMD and other external work on the common path more often than needed

The next optimisation step is therefore:

1. reduce repeat work first
2. route retrieval to one source first, not many
3. keep multi-source search as an explicit fallback, not the default

## Verified Baseline

Current verified behavior in the repo:

1. The Moon plugin `assemble()` shells out to `moon context-engine` before model
   dispatch.
2. The Moon plugin `afterTurn()` can also run `moon context-engine`, which means
   similar work may happen both before and after a turn.
3. `run_checkpoint()` currently executes a serial path:
   `record -> hot lifecycle -> project -> optional cleanse -> assemble -> packet -> save`.
4. `assemble_context()` reparses the raw session by calling
   `load_source_excerpt()`.
5. `run_cleanse_checkpoint()` reparses the same raw source again when cleanse
   triggers.
6. `build_context_packet()` currently gathers candidates from:
   - hot projection data
   - latest cleanse summary
   - `MEMORY.md`
   - recent daily memory files
   - recent library docs
   - recent distilled artifacts
   - QMD hot recall
   - QMD library recall
7. `recent_markdown_files()` recursively gathers full markdown trees and sorts
   the full result set before truncating to the few newest files.
8. Duplicate evidence is suppressed late, after multi-source collection and
   scoring have already happened.
9. The optional curator subagent already exists and is gated, but it is not the
   main optimisation target for this MIP.

## Problem

Moon active context quality is no longer blocked by the old prompt-boundary
problem. It is now blocked by unnecessary hot-path work.

The concrete inefficiencies are:

1. Repeated raw parsing: `project`, `assemble`, and `cleanse` all derive
   overlapping data from the same session source.
2. Repeated source scanning: memory, library, and distill trees are rescanned on
   routine packet builds.
3. Repeated broad retrieval: the packet builder fans out across multiple source
   families even when the user intent points clearly to one primary source.
4. Repeated evidence competition: the same decision or fact may arrive from hot,
   daily memory, durable memory, distill, and QMD in the same pass.
5. Repeated command overhead: QMD and collection-lifecycle work remain on the
   critical path more often than needed.
6. Repeated end-to-end orchestration: `afterTurn()` and pre-dispatch
   `assemble()` are still too close to the same full checkpoint flow.

## Goals

1. Reduce common-case assemble latency by removing repeated parse, scan, and
   search work.
2. Search one primary source lane first, not many repeated source lanes.
3. Search one canonical source artifact first inside that lane where possible.
4. Allow deterministic fallback only when the primary source is insufficient.
5. Keep Moon as the owner of active context assembly.
6. Keep the current packet boundary and compaction boundary intact while the hot
   path gets faster.
7. Make packet evidence more compact by avoiding duplicate source coverage
   before section selection.

## Non-Goals

1. Do not move active context ownership from Moon to OpenClaw.
2. Do not reintroduce routine `systemPromptAddition`.
3. Do not expand the curator subagent role. It stays optional and secondary.
4. Do not depend on OpenClaw memory tools or memory slots.
5. Do not optimize for broad multi-source retrieval first. That is explicitly
   the behavior being reduced.

## Decision

Moon will move from broad repeated retrieval to incremental routed retrieval.

The new rules are:

1. Parse once per checkpoint.
2. Reuse one shared checkpoint snapshot across project, assemble, and cleanse.
3. Route each assemble pass to one primary source lane first.
4. Within that lane, search one canonical source artifact first where possible.
5. Escalate to at most one fallback lane unless the prompt explicitly asks for
   cross-source comparison.
6. Deduplicate by evidence identity before final packet section selection.
7. Keep QMD as a fallback or background aid, not the default first-step search
   on common turns.

## Target Architecture

### 1. Parse-Once Checkpoint Snapshot

Introduce a shared checkpoint snapshot produced once from the raw session.

That snapshot should carry:

1. parsed projection data
2. raw excerpt
3. latest goal lines
4. active work lines
5. extracted query intent terms
6. source-generation metadata needed for invalidation

The checkpoint flow should stop re-deriving those values independently in
multiple steps.

`run_checkpoint()` should orchestrate around one parsed session snapshot instead
of each step calling back into raw extraction helpers separately.

### 2. Routed Source Lanes

Moon should classify the current assemble request into one primary source lane.

The initial lane set should be:

1. `hot`
   - for current task status, active work, unresolved steps, current-session
     goals
   - primary source: current hot projection
2. `memory`
   - for preferences, stable decisions, durable conventions, previously agreed
     rules
   - primary source: `MEMORY.md`
   - fallback source: newest relevant daily-memory file
3. `library`
   - for workspace or reference-document lookups
   - primary source: library docs / library projection docs
   - fallback source: QMD library recall
4. `distill`
   - for older completed work, prior outcomes, or historical summaries
   - primary source: distilled artifacts
   - fallback source: recent daily-memory files
5. `semantic`
   - reserved for explicit fallback when the routed lexical source did not
     provide enough usable evidence
   - primary source: QMD

The search contract becomes:

1. choose one primary lane
2. search that lane first
3. stop if confidence is sufficient
4. search one fallback lane only if needed
5. only perform routine multi-lane retrieval when the user explicitly asks for a
   comparison across sources

This is the core change for the current focus: search should be based on one
source first, not multiple repeated sources.

### 3. Canonical Source Election

Inside a lane, Moon should prefer one canonical source artifact before opening
adjacent artifacts that likely repeat the same information.

Examples:

1. Memory recall: check `MEMORY.md` first; only fall back to one recent
   daily-memory file if durable memory underfills.
2. Hot current-task recall: check the current hot projection first; do not also
   query daily memory or distill by default.
3. Library reference: check library docs first; only use QMD library recall if
   the direct library lane underfills.
4. Historical outcome recall: check distill first; only fall back to daily
   memory if the distilled lane does not answer the question.

Moon should not gather the same likely evidence from several source artifacts
just to throw most of it away later.

### 4. Incremental Source Manifests And Line Caches

Moon should stop recursively rescanning large source trees on routine assembles.

Introduce per-lane manifests that track:

1. relevant file paths
2. mtimes
3. sizes
4. fingerprints
5. cached stripped markdown bodies
6. cached selected lines or snippets keyed by lane intent

The manifests should be invalidated only when:

1. a source file changes
2. a new source file appears
3. lane-specific selection inputs change

This allows common assembles to become read-mostly and index-backed rather than
directory-scan-heavy.

### 5. Pre-Dedup Evidence Clustering

Moon should deduplicate before final section assembly, not only after broad
candidate collection.

Introduce a stable `EvidenceKey` based on normalized text and source-aware
identity.

The packet builder should then:

1. cluster repeated evidence
2. elect one canonical source per cluster
3. keep conflict context only when multiple sources genuinely disagree
4. feed section budgets with unique evidence clusters, not raw repeated rows

This makes packet budgets reflect distinct facts rather than distinct copies of
the same fact.

### 6. Split Sync Work From Hot Dispatch Work

Moon should separate:

1. sync or refresh work that can happen after the turn
2. minimal read-mostly work needed immediately before dispatch

Target behavior:

1. `afterTurn()` performs record, hot projection refresh, manifest updates, and
   best-effort background preparation.
2. pre-dispatch `assemble()` mostly reads the current checkpoint snapshot,
   routes the source lane, pulls cached evidence, and renders the packet.
3. expensive fallback work only happens when routing confidence is low or the
   prompt explicitly demands deeper recall.

The hot path should stop paying full refresh costs for work that could have been
prepared earlier.

### 7. Subagent Boundary

The curator subagent remains out of the main optimisation path.

Rules:

1. keep it disabled by default
2. do not use it to expand search breadth
3. only allow it after routed retrieval still produces overflow or ambiguity
4. do not let it hide repeated-work regressions in the deterministic path

## Implementation Plan

### Phase 1: Remove Repeated Parse Work

1. Introduce a shared checkpoint snapshot type reused across project, assemble,
   and cleanse.
2. Remove duplicate `extract_projection_data()` / `load_source_excerpt()` calls
   inside one checkpoint.
3. Add explicit counters for:
   - raw parse count per checkpoint
   - source files read per assemble
   - QMD calls per assemble
   - fallback depth

Exit criteria:

1. one checkpoint parses the raw source at most once
2. assemble diagnostics report parse and source-read counts

### Phase 2: Remove Repeated Source Scans

1. Add source manifests for memory, library, and distill lanes.
2. Replace recursive directory gathers on the common assemble path with manifest
   reads.
3. Add cached stripped-body and selected-line reuse keyed by file fingerprint.

Exit criteria:

1. common assembles do not recursively scan `memory/` or `mlib/`
2. source reads fall to only the files actually selected for the routed lane

### Phase 3: Add Single-Source Routing

1. Add a query-intent router that chooses one primary lane.
2. Add lane-specific confidence rules.
3. Add one-lane-only common behavior.
4. Allow one fallback lane only when confidence underfills.
5. Move QMD to fallback-only for common paths.

Exit criteria:

1. common active-work prompts hit only the `hot` lane
2. memory prompts hit `MEMORY.md` first and do not also search unrelated lanes
3. library prompts do not also search memory or distill by default

### Phase 4: Add Canonical Source Election And Pre-Dedup

1. Introduce `EvidenceKey`.
2. Elect canonical evidence before section budgeting.
3. Keep only one source per repeated evidence cluster unless conflict handling
   is explicitly needed.

Exit criteria:

1. non-compare prompts do not emit duplicate evidence clusters from multiple
   source families
2. packet budgets operate on distinct facts, not repeated copies

### Phase 5: Split Sync And Assemble Modes

1. Reduce pre-dispatch `assemble()` to a fast read-mostly path.
2. Move refresh-heavy work toward `afterTurn()` or another explicit sync mode.
3. Ensure the fast path reuses the outputs of the sync path instead of
   recomputing them.

Exit criteria:

1. the common dispatch path avoids full refresh work when cached state is fresh
2. `afterTurn()` and `assemble()` stop duplicating the same end-to-end heavy
   work

## Acceptance Criteria

This MIP is complete when the repo can show all of the following:

1. One checkpoint parses the raw source at most once.
2. Common non-compare assembles route to exactly one primary lane.
3. Common non-compare assembles do not perform routine broad multi-source
   fanout.
4. QMD is not called on the common hot-lane or memory-lane happy path.
5. `MEMORY.md`-style recall does not also search unrelated lanes unless fallback
   is triggered.
6. Final packets contain no duplicate evidence clusters across source families
   for non-compare prompts.
7. Common-path assemble latency is materially lower than the current baseline.

Suggested performance targets:

1. reduce common-path median assemble wall time by at least `50%`
2. reduce common-path QMD calls by at least `75%`
3. reduce per-assemble source-file reads to the routed lane plus at most one
   fallback lane

## Test Plan

1. Unit tests for the query-intent router and lane selection.
2. Unit tests for canonical source election and evidence clustering.
3. Regression tests proving one raw parse per checkpoint.
4. Regression tests proving routed memory recall checks `MEMORY.md` before daily
   memory.
5. Regression tests proving routed library recall does not also search memory
   lanes by default.
6. Regression tests proving QMD is skipped on common happy paths.
7. Plugin tests proving pre-dispatch assembly uses the fast path while
   after-turn sync handles refresh work.

## Risks

1. A wrong primary-lane decision could under-retrieve. Mitigation: deterministic
   fallback and explicit routing diagnostics.
2. Cached manifests could go stale. Mitigation: fingerprint-based invalidation
   on every relevant source write.
3. Some prompts genuinely need multi-source comparison. Mitigation: allow
   explicit compare-mode routing rather than making multi-source fanout the
   default.

## Summary

The next Moon optimisation step is not broader retrieval. It is less repeated
work.

Moon should:

1. parse once
2. scan incrementally
3. search one source first
4. fall back only when needed
5. dedupe before packet budgeting

That is the shortest path to lower latency, lower cost, and higher signal in the
active context packet.
