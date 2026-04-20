# Dev Notes

## OpenClaw Prompt Layers: Verified Notes

This note is based on the current code, not memory.

Relevant files:

- `openclaw/src/agents/system-prompt.ts`
- `openclaw/src/agents/system-prompt-cache-boundary.ts`
- `openclaw/src/agents/pi-embedded-runner/run/attempt.ts`
- `openclaw/src/context-engine/types.ts`
- `moon/assets/plugin/index.js`
- `openclaw/src/agents/pi-embedded-runner/manual-compaction-boundary.test.ts`
- `openclaw/src/agents/pi-embedded-runner/replay-history.ts`

### First Principle

The current OpenClaw request shape has three distinct layers:

1. `systemPrompt`
2. `messages`
3. structured `tool definitions`

The static/dynamic split applies to the `systemPrompt`, not to the whole
request.

## Static Prompt Group

This is the part of the system prompt above `OPENCLAW_CACHE_BOUNDARY`.

Verified in:

- `src/agents/system-prompt.ts`
- `src/agents/system-prompt-cache-boundary.ts`

Above the boundary, OpenClaw currently builds hardcoded or less-volatile system
sections such as:

- `## Tooling`
- `## Tool Call Style`
- execution-bias guidance
- provider stable-prefix override
- `## Safety`
- `## OpenClaw CLI Quick Reference`
- `## Skills`
- memory section
- self-update section
- model aliases
- `## Workspace`
- docs section
- sandbox section
- user identity section
- time section
- `## Workspace Files (injected)`
- reply tags
- messaging guidance
- voice guidance
- reactions
- reasoning format
- stable project context files
- `## Silent Replies`

Important distinction:

- tool guidance text inside the system prompt is part of this static group
- structured tool definitions and schemas are separate from this boundary logic

## Dynamic Prompt Group

This is the part of the system prompt below `OPENCLAW_CACHE_BOUNDARY`.

Verified in:

- `src/agents/system-prompt.ts`
- `src/agents/system-prompt-cache-boundary.ts`

Below the boundary, OpenClaw currently places:

- dynamic project context files
- `extraSystemPrompt`
  - labeled as `## Group Chat Context` or `## Subagent Context`
- `providerDynamicSuffix`
- heartbeats section
- runtime section

Also verified:

- only `heartbeat.md` is explicitly classified as a dynamic context file in the
  current code

So the dynamic group is still system-prompt space. It is the volatile suffix of
the system prompt.

## User Prompts

User prompts are not part of the static/dynamic system-prompt split.

They live in the `messages` array.

Verified in:

- `src/context-engine/types.ts`
- `src/agents/pi-embedded-runner/run/attempt.ts`
- `assets/plugin/index.js`

Current verified behavior:

- Moon `assemble()` returns `messages`
- the Moon plugin currently passes the incoming `messages` through unchanged
- OpenClaw sends those as conversation context

So:

- user messages are not in the static system group
- user messages are not in the dynamic system suffix either
- they are a separate message-history layer

## Tool Prompts

The phrase "tool prompts" is ambiguous in the current architecture. There are
really two tool-related layers:

1. tool guidance text inside the system prompt
2. structured tool definitions and schemas sent separately

So "tool prompts" are not one single verified bucket.

## Compaction Summary Placement

The compaction summary does not go into the system prompt.

It does not become a user message.

It is carried through the compaction/context path.

Verified in:

- `assets/plugin/index.js`
- `src/context-engine/types.ts`
- `src/agents/pi-embedded-runner/manual-compaction-boundary.test.ts`
- `src/agents/pi-embedded-runner/replay-history.ts`

What the current code shows:

- Moon `compact()` writes a JSONL `compaction` entry with a `summary`
- OpenClaw compaction handling can rebuild context so that summary appears as a
  `compactionSummary` message role
- OpenClaw tests explicitly verify rebuilt context like:
  - `["compactionSummary"]`
  - later `["compactionSummary", "user"]`

So the summary lands in message-history context, not in:

- static system prompt
- dynamic system suffix
- user prompt
- structured tool definitions

## Bottom Line

The verified model is:

- static group = stable part of OpenClaw's system prompt
- dynamic group = volatile suffix of OpenClaw's system prompt
- user prompts = separate `messages` layer
- tool definitions = separate structured layer
- compaction summaries = context/message-history layer as `compactionSummary`

So this statement is basically correct:

> after compaction, the summary goes to the context, not the user/system/tool
> prompts

The more precise verified wording is:

- after compaction, the summary goes into message-history context as a
  `compactionSummary`, not into the system-prompt boundary groups

## 2026-04-08 Addendum: Verified Source Of Truth And Adopted Moon Design

OpenClaw source of truth for this note was re-checked on `2026-04-08` against:

- repo used for verification: local OpenClaw checkout
- verified ref: `origin/main`
- verified commit: `a44a26f0a0a4`

Important repo-state note:

- the checked-out local branch was `codex/context-engine-main-refresh` at
  `b9bb2a1ddb12`
- that branch was `ahead 2, behind 581` vs `origin/main`
- therefore prompt-boundary conclusions below were taken from `origin/main`, not
  from the stale checked-out branch

### Verified Boundary Detail

From `origin/main`, the current OpenClaw flow is:

1. OpenClaw builds its own base `systemPrompt`.
2. OpenClaw inserts `OPENCLAW_CACHE_BOUNDARY` into that prompt.
3. A context engine `assemble(...)` call may return:
   - `messages`
   - optional `systemPromptAddition`
4. If `systemPromptAddition` is present, OpenClaw inserts it **after** the cache
   boundary, at the front of the dynamic system-prompt suffix.
5. The boundary marker itself is stripped before provider transport.

So:

- `systemPromptAddition` is **not** message-history context
- `systemPromptAddition` is still system-prompt text
- it belongs to the dynamic prompt region below the cache boundary

### Verified Cache Boundary Meaning

The current clean boundary is:

1. static prompt:
   - everything above `OPENCLAW_CACHE_BOUNDARY`
2. dynamic prompt:
   - everything below `OPENCLAW_CACHE_BOUNDARY`
   - includes `systemPromptAddition` when present
3. message-history context:
   - `messages`
   - replayed `compactionSummary`
4. tools:
   - separate structured tool definitions and schemas

Static prompt should change only when stable prompt inputs change, for example:

1. OpenClaw hardcoded prompt text changes
2. stable injected workspace files change
3. stable provider prompt contribution changes
4. tool inventory or tool guidance changes
5. prompt mode or other stable runtime configuration changes

Moon checkpointing, `cleanse`, indexing churn, timestamps, and receipts should
not be reasons to mutate the static prompt prefix.

### Adopted Moon Design Decision

For Moon planning, the adopted design is:

1. Moon `cleanse` is the Moon-side compaction stage.
2. Moon `cleanse` summaries should use the verified OpenClaw message-history
   lane:
   - transcript `compaction` entry
   - replayed into model context as `compactionSummary`
3. Moon should **not** use `systemPromptAddition` for routine assembled context
   right now.
4. Moon should keep `systemPromptAddition` reserved for a future case where a
   short, high-priority dynamic instruction truly belongs in system-prompt
   space.
5. Indexing, embed, projection, and operator receipts should stay out of the
   model-facing prompt by default.

## 2026-04-08 Implementation Status

The first Moon cleanup slice from `mip.md` is now implemented.

1. `assets/plugin/index.js` still runs `moon context-engine` during
   `assemble()`, but it no longer returns routine `systemPromptAddition`.
2. Moon `compact()` still appends transcript `compaction` entries from Moon
   `cleanse` output.
3. The on-disk Moon assembly artifact remains available for operator/debug
   inspection.
4. `trimAssemblyText(...)` and runtime `maxAssemblyChars` handling were removed
   from the assemble path because routine Moon assembly is no longer shipped as
   system-prompt text. The manifest still accepts `maxAssemblyChars` as a
   deprecated compatibility no-op for upgraded installs.
5. The intended contract is now:
   - operator artifact on disk
   - compaction summary in transcript `compaction` entries
   - replayed model-facing summary as `compactionSummary`
   - no routine Moon summary duplication in dynamic system-prompt text

Important constraint:

- `compactionSummary` is a verified compaction-summary lane
- use it for Moon `cleanse` output
- do **not** overload it with unrelated indexing telemetry or generic operator
  receipts unless a separate message-history injection contract is later
  verified

## 2026-04-20 Active Packet Status

The active-context packet plan is now implemented in Moon.

1. `moon context-engine` refreshes the hot projection every checkpoint, even
   when `cleanse` does not run.
2. Moon writes a separate active context packet artifact under
   `$MOON_HOME/context-packets/`.
3. `assets/plugin/index.js` reads that packet and injects it into the OpenClaw
   `messages` lane during routine `assemble()`.
4. Routine Moon `systemPromptAddition` remains unused.
5. When replay already contains `compactionSummary`, the plugin tells Moon via
   `--replay-has-compaction-summary` so the packet can avoid duplicating the
   latest `cleanse` summary.
6. A Moon-owned embedded curator subagent is available behind plugin config and
   only runs in gated mode over the bounded packet candidate set.
