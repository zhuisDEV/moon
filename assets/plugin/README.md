# moon plugin

`moon` now provides two OpenClaw-facing behaviors:

1. a native `context-engine` plugin that invokes `moon context-engine`
2. persist-time `toolResult` compaction through the `tool_result_persist` hook

## Native context-engine behavior

When OpenClaw selects `plugins.slots.contextEngine = "moon"`, the plugin:

1. registers a native context engine under id `moon`
2. invokes the managed `moon` binary as a short-lived checkpoint at `assemble`
3. injects the assembled MOON artifact into the runtime system prompt
4. syncs post-turn state through `afterTurn`
5. owns same-turn overflow compaction through `compact`
6. appends MOON-owned `compaction` entries into the OpenClaw session transcript
   using the latest `cleanse` summary

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
5. `contextEngineTimeoutMs` (default `20000`)
6. `maxAssemblyChars` (default `24000`)
7. `syncAfterTurn` (default `true`)
8. `fallbackMode` (`openclaw` or `disabled`, default `disabled`)
9. `compactFallbackOnSkip` (default `false`)
10. `maxTokens` (default `12000`)
11. `maxChars` (default `60000`)
12. `maxRetainedBytes` (default `250000`)
13. `tools.<tool>.maxTokens`
14. `tools.<tool>.maxChars`
