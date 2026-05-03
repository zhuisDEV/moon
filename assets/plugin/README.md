# moon plugin

`moon` now provides two OpenClaw-facing behaviors:

1. a native `context-engine` plugin that invokes `moon context-engine`
2. persist-time `toolResult` compaction through the `tool_result_persist` hook

## Native context-engine behavior

When OpenClaw selects `plugins.slots.contextEngine = "moon"`, the plugin:

1. registers a native context engine under id `moon`
2. invokes the managed `moon` binary as a short-lived checkpoint at `assemble`
3. keeps the assembled MOON artifact as operator/debug state instead of
   injecting it into routine runtime system-prompt text
4. reads the Moon active context packet and injects it into the OpenClaw
   `messages` lane
5. can optionally run a gated Moon-owned curator subagent over that packet
6. syncs post-turn state through `afterTurn`
7. owns same-turn overflow compaction through `compact`
8. appends MOON-owned `compaction` entries into the OpenClaw session transcript
   using the latest `cleanse` summary

Model-facing summary rule:

1. routine Moon assembly does not return `systemPromptAddition`
2. routine Moon assembly injects the active context packet through `messages`
3. Moon `cleanse` summaries travel through transcript `compaction` entries
4. OpenClaw replays those entries as `compactionSummary` message-history context

Synthetic packet replay rule:

1. injected `# Moon Active Context` packets are prompt hints for the current
   provider call
2. if a prior packet appears in a later transcript replay, Moon filters it from
   projection parsing
3. old active packets are not primary source material for building the next
   packet; use a separately gated recovery fallback if a damaged transcript
   truly needs one

Compaction rule:

1. if `moon context-engine` does not emit a readable `cleanse` summary, the
   plugin requests explicit OpenClaw fallback compaction (when fallback mode is
   enabled)
2. fallback handoff is controlled by `fallbackMode` and does not run alongside
   successful MOON compaction

## Persist-time compaction behavior

The plugin still compacts large `toolResult` text blocks at persist time using
the `tool_result_persist` hook.

1. Token-aware + char-aware budget enforcement.
2. Per-tool limits (global defaults with per-tool overrides).
3. JSON projection for high-volume tools (`read`, `message/readMessages`,
   `message/searchMessages`, `web_fetch`, `web.fetch`).
4. Metadata persisted to `details.ocTokenOptim` with before/after estimated
   tokens.
5. Optional full payload retention in `details.ocTokenOptim.fullText` when under
   `maxRetainedBytes`.

## Plugin config

Under `plugins.entries.moon.config`:

1. `moonPath` (optional absolute/relative path to the managed `moon` binary)
2. `moonHome` (optional runtime root override passed as `MOON_HOME`)
3. `memoryDir` (MOON daily-memory directory, typically `$MOON_HOME/memory`)
4. `memoryFile` (MOON durable memory file, typically `$MOON_HOME/MEMORY.md`)
5. `contextEngineTimeoutMs` (default `120000`)
6. `maxAssemblyChars` (deprecated compatibility no-op; accepted but ignored)
7. `contextPacketMaxTokens` (gating threshold for local packet size, default
   `1400`)
8. `contextPacketCandidateThreshold` (gating threshold for candidate count,
   default `10`)
9. `assemblySubagentMode` (`disabled` or `gated`, default `disabled`)
10. `assemblySubagentProvider` (required when gated curation is enabled unless
    the model already implies one; fast defaults: `openai`/`openai-codex` ->
    `gpt-5.4-mini`, `google`/`gemini` -> `gemini-3.1-flash-lite-preview`,
    `anthropic` -> `claude-3-5-haiku-latest`)
11. `assemblySubagentModel` (optional; defaults to a fast provider-specific
    model when omitted)
12. `assemblySubagentTimeoutMs` (default `15000`)
13. `assemblySubagentCacheTtlMs` (default `300000`)
14. `syncAfterTurn` (default `true`)
15. `fallbackMode` (`openclaw` or `disabled`, default `disabled`)
16. `compactFallbackOnSkip` (default `false`)
17. `maxTokens` (default `12000`)
18. `maxChars` (default `60000`)
19. `maxRetainedBytes` (default `250000`)
20. `tools.<tool>.maxTokens`
21. `tools.<tool>.maxChars`

Managed Moon installs and upgrades now stamp `contextEngineTimeoutMs=120000`
explicitly because real `cleanse` + packet assembly can exceed 20s on long
sessions.
