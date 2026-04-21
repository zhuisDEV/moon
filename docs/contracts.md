# moon System Contracts

## Scope

This document defines the Moon v1 primary-path contracts under `mip-moonv1.md`.

It is intentionally limited to the normal MOON-owned control flow:

1. `record`
2. `project`
3. `cleanse`
4. `assemble`

OpenClaw fallback behavior is out of scope here and must not be mixed into these
contracts.

## Control Rules

1. MOON owns the normal-path context preparation flow before model dispatch.
2. `record` always runs at a stable checkpoint.
3. `cleanse` runs only when pressure policy requires compaction.
4. the hot `project` refresh runs every checkpoint so active retrieval stays
   current even when `cleanse` does not fire.
5. `project` is deterministic projection work and is not the same as `cleanse`.
6. `assemble` is the pre-dispatch control boundary for the next active context
   window.
7. Search and memory maintenance are downstream support systems, not the control
   plane.

## OpenClaw Boundary

Moon and OpenClaw have separate ownership boundaries.

1. Moon owns:
   - checkpointing
   - conditional `cleanse`
   - operator/debug assembly artifacts
   - model-facing active context packet generation
   - transcript `compaction` entry creation
2. OpenClaw owns:
   - final system prompt assembly
   - message-history replay
   - structured tool definitions
   - provider dispatch
3. Routine Moon context must not be reintroduced as dynamic system-prompt text
   through `systemPromptAddition`.
4. Moon compaction summaries must travel through transcript `compaction` entries
   and replay downstream as `compactionSummary` message-history context.

## Controller Form

1. `moon-context-engine` is short-lived and runs when OpenClaw needs the active
   context window prepared.
2. The watcher, if present, is a separate long-running maintenance worker.
3. The watcher must not be described or treated as the normal-path context
   controller.

## Runtime Root

The canonical runtime root is `$MOON_HOME`.

Primary contract paths:

1. raw capture: `$MOON_HOME/raw/`
2. projection markdown: `$MOON_HOME/mds/`
3. compaction summaries: `$MOON_HOME/cleanse/`
4. daily memory: `$MOON_HOME/memory/`
5. durable memory: `$MOON_HOME/MEMORY.md`

Legacy archive-era paths are migration debt and must not define the target
architecture.

## `record`

Purpose:

1. capture the active session into MOON-owned raw state
2. establish the single raw source consumed by downstream MOON stages

Input contract:

1. a readable active-session source
2. a resolvable `session_id`
3. stable checkpoint timing chosen by MOON

Output contract:

1. write `$MOON_HOME/raw/<session_id>.jsonl`
2. preserve full-fidelity source content
3. update runtime state with the latest recorded session identity

Rules:

1. `record` is unconditional in the primary path
2. `record` must not depend on `cleanse`
3. `record` must not perform summarisation, projection, or search maintenance
4. repeated runs against the same unchanged source should be operationally safe

## `project`

Purpose:

1. transform raw session documents into Moon-managed projection markdown
2. create the deterministic bridge between raw capture and downstream L1 memory
   work

Input contract:

1. a readable raw session document from `$MOON_HOME/raw/`
2. a resolvable `session_id`

Output contract:

1. write `$MOON_HOME/mds/<session_id>.md`
2. preserve high-signal user, assistant, and tool activity
3. remove obvious transport noise deterministically
4. refresh the hot searchable session projection every checkpoint, not only
   during `cleanse`

Rules:

1. `project` is not an LLM compaction step
2. `project` must be deterministic for the same raw input
3. `project` is background/deferred work, not the active-window recovery path
4. `distill --mode norm` consumes projection markdown, not cleanse summaries

## `cleanse`

Purpose:

1. reduce active-context pressure under MOON control
2. produce a compact recovery summary for the next active context window

Input contract:

1. a readable active raw session source
2. pressure metadata or policy state sufficient to justify compaction
3. a dedicated cleanse model configuration separate from `syns`

Output contract:

1. write `$MOON_HOME/cleanse/<session_id>.md`
2. emit a compact recovery summary, not projection markdown
3. preserve current goal, active work, decisions, blockers, and relevant
   evidence

Policy anchors:

1. trigger: `60k`
2. target: `40k`
3. emergency: `100k`

Rules:

1. `cleanse` is conditional, not unconditional
2. `cleanse` output does not replace raw capture
3. `cleanse` output does not replace `project`
4. `cleanse` config and model role must remain separate from `syns`

## `assemble`

Purpose:

1. checkpoint and compose the next Moon operator artifact for the active window
2. define the exact Moon-side control boundary before OpenClaw builds the final
   provider-facing prompt envelope

Input contract:

1. current session identity and control state
2. latest applicable raw-session context
3. latest applicable `cleanse` summary when compaction has occurred
4. optional minimal search/indexing anchor when it materially helps recovery

Output contract:

1. write an operator/debug assembly artifact for the active window
2. write a separate model-facing active context packet for message-lane
   injection
3. preserve the latest `cleanse` summary in the operator artifact when
   compaction has run
4. exclude bulk search receipts, embed logs, and low-signal transport noise from
   model-facing prompt text
5. avoid routine `systemPromptAddition` injection for normal Moon context

Rules:

1. `assemble` is the primary control boundary before model dispatch
2. the model-facing packet must travel through the OpenClaw `messages` lane, not
   through routine system-prompt injection
3. `assemble` must not treat the operator artifact as the final provider-facing
   prompt
4. `assemble` must stay focused on prompt/context composition, not background
   maintenance
5. fallback behavior must not be embedded into the normal-path `assemble`
   contract
6. normal-path Moon summary content should reach the model through the verified
   transcript compaction lane, not through routine system-prompt injection

## Search Support Contracts

These are important Moon subsystems, but they are not the primary control path.

### `embed`

Purpose:

1. refresh the searchable Moon corpus from projection markdown
2. keep retrieval current after projected documents change

Rules:

1. `embed` operates on `$MOON_HOME/mds/`
2. `embed` is bounded maintenance work
3. full embed receipts do not belong in model-facing prompt context

### `recall`

Purpose:

1. retrieve relevant prior Moon-managed context
2. supply at most a minimal retrieval anchor to the active workflow when needed

Rules:

1. `recall` returns structured search results
2. no-match is a valid non-fatal outcome
3. retrieval is support for the control path, not the control path itself

## Memory Distillation Contracts

These stages are downstream from projection and separate from active context
control.

### `distill --mode norm`

Purpose:

1. normalise projection markdown into daily memory artifacts

Rules:

1. consumes `$MOON_HOME/mds/*.md`
2. remains deterministic and bounded

### `distill --mode syns`

Purpose:

1. synthesize durable memory from daily memory inputs

Rules:

1. uses its own synthesis model role
2. must remain separate from `cleanse`
3. watcher-triggered `syns` must synthesize the previous completed local daily
   memory file, never the current day's file
4. if the scheduled previous-day daily-memory file is missing, watcher-triggered
   `syns` must skip rather than silently fall back to current-day memory
5. writes durable memory outcomes, not active context recovery summaries

## Transitional Runtime Note

The watcher may continue to exist during migration, but it is transitional
infrastructure only.

It must not be treated as the final architectural owner of:

1. context capture
2. compaction policy
3. pre-dispatch context assembly
