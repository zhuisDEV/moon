# Moon -> OpenClaw -> Provider Flow Report

Date: 2026-04-07

## Status

- This report is a historical verification snapshot captured before the
  2026-04-08 Moon prompt-boundary cleanup landed.
- Current Moon behavior differs from the pre-change plugin flow described below:
  - routine `assemble()` no longer injects the Moon assembly artifact into
    `systemPromptAddition`
  - Moon `cleanse` summaries now rely on transcript `compaction` entries and
    downstream `compactionSummary` replay
- Use `mip.md` and `dev-notes.md` for the current implemented boundary.

## Repo Update Status

Moon:

- Local repo was on `main` and fast-forwarded safely from `badcd3c` to
  `c22d84e`.
- Current upstream state matches `origin/main`.
- Latest verified tag fetched during update: `v1.0.9`.

OpenClaw:

- Local repo is checked out on `codex/acp-visible-text-refresh`, not `main`.
- That branch tracks `origin/main` but is diverged:
  - local-only commits: `1`
  - upstream-only commits on `origin/main`: `3436`
- I did not rewrite or rebase the current local branch.
- I fetched the latest remote state and verified the latest upstream code from
  `origin/main` at commit `48a3511233`, which is the `v2026.4.5` release line.

Conclusion:

- Moon was updated locally.
- OpenClaw was verified against the latest fetched upstream `origin/main`, but
  the current local branch was left unchanged because it cannot be safely
  fast-forwarded in place.

## Verified Architecture

High-level flow:

`Moon raw/session processing -> Moon context-engine plugin -> OpenClaw final system prompt assembly -> OpenClaw provider transport -> LLM provider`

This is not a guess. The ownership split is explicit in code and docs.

## Verified Moon Side

Moon documents itself as the active context-preparation controller:

- `moon context-engine` controls normal turn-time preparation:
  `record -> conditional cleanse -> assemble`.
- Source: `README.md`

Moon's core active-lane checkpoint implementation is in
`src/moon/context_engine.rs`:

1. `run_checkpoint()` records the session transcript.
2. It conditionally runs `project` and `cleanse` when pressure requires it.
3. It resolves assemble input and calls `assemble_context(...)`.
4. It writes the assembled output to `paths.context_engine_dir`, which is
   `$MOON_HOME/mce/<session>.md`.

Moon's assembly content is built in `src/moon/assemble.rs`:

- `assemble_context(...)` reads the raw source excerpt and optional cleanse
  summary.
- `render_context(...)` produces a markdown artifact with the header
  `# MOON Assembly Context`.
- `write_assembly_output(...)` persists that artifact under the Moon context
  engine output directory.

So Moon's job is to produce an assembled context artifact, not the final model
request envelope.

## Verified Moon Plugin Boundary

Moon's OpenClaw plugin contract is explicit in `assets/plugin/README.md`:

1. It registers a native context engine under id `moon`.
2. It invokes the managed `moon` binary at `assemble`.
3. It injects the assembled Moon artifact into the runtime system prompt.
4. It syncs post-turn state with `afterTurn`.
5. It owns compaction when selected.

The actual assemble hook implementation is in `assets/plugin/index.js`:

- `assemble(params)` calls `runMoonContextEngine(...)`.
- It reads the returned `assemblyText`.
- It trims that text to `systemPromptAddition`.
- It returns:
  - `messages`: the existing OpenClaw messages array
  - `estimatedTokens`
  - optional `systemPromptAddition`

Important consequence:

- Moon is not replacing the full OpenClaw prompt.
- Moon is returning a `systemPromptAddition` string to OpenClaw.
- The conversation messages remain under OpenClaw runtime control.

## Verified OpenClaw Context-Engine Contract

OpenClaw's context-engine contract is defined in `src/context-engine/types.ts`.

`assemble(...)` returns:

- `messages: AgentMessage[]`
- `estimatedTokens: number`
- optional `systemPromptAddition?: string`

OpenClaw's own docs say the same in `docs/concepts/context-engine.md`:

- the engine returns ordered messages for the model
- `systemPromptAddition` is prepended to the runtime system prompt

So the contract itself confirms that context engines provide:

1. message selection / ordering
2. optional system-prompt addition

They do not own the entire OpenClaw system prompt.

## Verified OpenClaw System Prompt Ownership

OpenClaw documents in `docs/concepts/context.md` and
`docs/concepts/system-prompt.md` that:

- the system prompt is OpenClaw-owned
- it is rebuilt each run
- it includes tooling guidance, skills metadata, workspace info, runtime info,
  and injected project-context files

In code, the system prompt body is built in `src/agents/system-prompt.ts`.

That file explicitly builds the OpenClaw-owned sections such as:

- identity line
- `## Tooling`
- safety guidance
- skills section
- memory/docs/workspace sections

This confirms the final system prompt is assembled by OpenClaw, not by Moon.

## Verified Run-Time Merge Point

The concrete merge point is in
`origin/main:src/agents/pi-embedded-runner/run/attempt.ts`.

Verified sequence:

1. OpenClaw builds its own base prompt with `buildEmbeddedSystemPrompt(...)`.
2. It stores that as `systemPromptText`.
3. It creates the agent session with `createAgentSession(...)`.
4. It applies the prompt to the session with
   `applySystemPromptOverrideToSession(session, systemPromptText)`.
5. Later, if a context engine is active, it calls
   `assembleAttemptContextEngine(...)`.
6. If the engine returns `systemPromptAddition`, OpenClaw prepends it with
   `prependSystemPromptAddition(...)`.
7. OpenClaw reapplies the updated `systemPromptText` to the active session.

This is the key verified ownership rule:

- Moon contributes an addition.
- OpenClaw still owns the final composed `systemPromptText`.

## Verified Provider Dispatch Path

OpenClaw resolves the provider stream function in
`src/agents/provider-stream.ts`:

- `registerProviderStreamForModel(...)` selects either:
  - a provider plugin stream function, or
  - a transport-aware built-in stream function

So OpenClaw owns the dispatch path from session context to the actual provider
request function.

For OpenAI-family transports, the verified provider payload shaping is in
`src/agents/openai-transport-stream.ts`:

- if `context.systemPrompt` exists, it is inserted into the outbound request
  first
- for Responses transport, OpenClaw sends:
  - `input: messages`
- for Chat Completions-style transport, OpenClaw sends:
  - `messages: convertMessages(...)`

The same file shows the system prompt is taken from `context.systemPrompt`,
which came from OpenClaw's assembled `systemPromptText`.

For proxy/provider wrappers,
`src/agents/pi-embedded-runner/proxy-stream-wrappers.ts` shows OpenClaw can
still patch provider payloads further for cache semantics or provider quirks,
but that happens after OpenClaw has already assembled the runtime
prompt/context.

## Concrete Ownership Boundary

Moon owns:

1. raw transcript checkpointing
2. conditional cleanse/project decisions
3. assembly artifact generation
4. context-engine plugin output
5. optional compaction ownership

OpenClaw owns:

1. the final system prompt structure
2. tool definitions and tool schema exposure
3. runtime session messages
4. the merge of `systemPromptAddition` into the final system prompt
5. provider stream resolution
6. provider-specific request payload shaping
7. actual dispatch to the LLM provider

## Bottom Line

The verified flow is:

1. Moon prepares context and writes a Moon assembly artifact.
2. Moon's plugin returns that artifact to OpenClaw as `systemPromptAddition`.
3. OpenClaw builds its own system prompt and prepends Moon's addition.
4. OpenClaw keeps ownership of the final prompt envelope and messages.
5. OpenClaw resolves the provider transport and sends the final payload to the
   LLM provider.

Therefore:

- Moon is a context producer and context-engine controller.
- OpenClaw is the final prompt assembler and provider transport owner.
- Provider-facing cache behavior is primarily determined by OpenClaw's final
  prompt assembly, but Moon can still affect cache reuse indirectly by changing
  the added context text it provides.
