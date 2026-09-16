# Moon OpenClaw adapter

This adapter registers the `moon` context engine, injects one bounded SQLite
context packet immediately before the current user message, and records
completed turns for selective durable-memory learning.

The adapter keeps strict ownership boundaries:

- Moon retrieves canonical memories and cited source references.
- The selected agent harness continues to own automatic transcript compaction.
  Moon delegates explicit compaction only for an identified stock OpenClaw
  harness and safely refuses the lossy generic fallback for Codex or an unknown
  harness.
- Moon also registers the optional `moon-local` compaction provider. When
  selected through `agents.defaults.compaction.provider`, it replaces only the
  safeguard summarization call. OpenClaw still chooses safe transcript and tool
  boundaries, preserves recent turns, writes checkpoints, and owns rollback.
- Retrieval failures fail open by default and preserve the original messages.
- The durable `commitTurn` hook stores only the accepted user request and final
  assistant answer. OpenClaw supplies the closed transcript range; Moon never
  rereads the growing transcript. Its advancement identity and evidence commit
  in one SQLite transaction, so retries cannot duplicate evidence or learning.
- Storage failures reject the commit for OpenClaw's durable retry queue, even
  with `failOpen` enabled. Model-based extraction is best effort after the
  commit; a crash after the evidence write can skip extraction, but preserves
  evidence for optional L2 synthesis. Heartbeats, plugin-level disabled
  learning, and turns without a visible answer are no-ops. Disabling only L1
  still records evidence.
- L1's configured primary or fallback model may propose up to
  `learningMaxMemories` memories, three by default. Exact-quote, numeric,
  confidence, importance, correction, and freshness checks run before a proposal
  reaches SQLite.
- L2 is disabled by default. When enabled, the plugin service reconciles bounded
  evidence batches daily, using durable leases, date keys, and three attempts
  per batch key. It retains review items for uncertain conflicts, preserves
  evidence, and never merges differently worded claims automatically.
  Independent candidates that pass validation can commit while sources behind
  rejected candidates remain pending. A partial commit stops the daily
  occurrence, also after a restart. Fixed failure phases and codes never include
  generated claims or provider error bodies.
- Greetings and irrelevant queries inject no context packet.
- Non-trivial context requests update a local, content-free metric row with
  injection state. Logs expose only its opaque request ID and numeric summary
  for optional human review.
- Completed-turn learning and native compaction emit content-free operational
  events; Moon's embedding worker records its own batch counts.

Lexical and hybrid retrieval both require no credentials and do not transmit
memory content. Hybrid mode uses Moon's local multilingual model through one
private stdio child owned by the adapter. The child has no network port and is
stopped when the adapter is disposed. Raw evidence is not embedded; active
memories are queued ahead of reference documents. Hash vectors exist only for
offline plumbing tests.

All model work stays inside OpenClaw's provider runtime. The runtime's
`moon.toml` supplies independent L1 and L2 models, reasoning, fallbacks,
timeouts, requested output limits, and optional prompt files. Unset fields
inherit the existing plugin/OpenClaw settings. With no file, L1 keeps its
existing route and L2 is disabled. The Astra starter uses `low` for L1 and
`xhigh` for L2. Custom prompts supplement fixed evidence guards and must be
readable UTF-8 files no larger than 64 KiB. Config is reloaded per completed
turn and scheduler tick; in-flight work keeps its initial settings.

On the verified native Codex setup, the `openai` provider uses OpenClaw's Codex
plugin, the app's Codex binary, and `homeScope: "user"` with the existing user
`CODEX_HOME` OAuth. Moon owns no provider credentials, creates no second login,
and prints no arbitrary provider failure bodies. Turn transcripts and proposals
sent to the Moon binary use stdin and are not exposed in process arguments.

OpenClaw 2026.9.2 model calls use the neutral `runEmbeddedAgent` runtime API.
Every learning or summarization attempt gets a fresh
`agent:<owner>:internal-session-effects:incognito-<id>` session key with
`sessionPersistence="detached"`, including fallback attempts. This preserves
agent ownership, selects an ephemeral native Codex thread, and keeps OpenClaw's
session in memory. Ambiguous ownership is rejected. Helpers do not reuse the
completed turn's transcript, and their tool route is disabled. File-backed
session targets are not accepted by this runtime. Cancellation stops routing
without starting a fallback, and only final answer payloads are accepted.

The native Codex backend inspected with OpenClaw 2026.9.4 ignores
`max_output_tokens`; Moon passes it as a provider request, not an enforced token
cap. Input size, timeout, actions, daily batches, and attempts have separate
limits. A successful model-route probe does not establish synthesis quality or
an exact subscription-usage budget.

Moon retains its existing configured model routing through this API. Switching
to OpenClaw's session-bound `llm.complete` API requires a separate operator
decision because explicit model selection needs an `llm.allowModelOverride`
grant. This release does not change those permissions.

The compaction provider uses `compactionModel` (default: `primaryModel`) with
`compactionReasoning=off` by default, disables the underlying model fallback
chain, and runs in an isolated raw-model session without Moon retrieval or
tools. The configured OpenClaw provider must support its native thinking-off
request shape. Host identifier-preservation settings are included in the summary
prompt. OpenClaw falls back to its configured built-in compaction model if the
provider fails or returns no summary. For a local-only privacy boundary, point
both settings at the same local provider/model and keep OpenClaw's explicit
compaction model local as well.

OpenClaw keeps native automatic transcript compaction because the adapter
advertises `ownsCompaction=false`. Moon owns retrieval and bounded packet
assembly, not transcript replacement. Selecting `moon-local` changes the summary
generator, not that ownership boundary.

The summary input contains semantic messages rather than raw provider objects:
stored thinking and message bookkeeping are excluded, visible text and complete
tool exchanges remain, and recognised media bodies receive omission markers.
This only changes the model input, not the stored source transcript.

The opt-in [proactive compaction profile](../../docs/compaction.md) uses the
host's supported preflight check at 240,000 active transcript bytes, with a
20,000-token recent-tail setting and three recent turns of safeguard context.
Older eligible history is summarised before the next request. The byte limit is
a starting point for local-model testing, not an exact token count or a
guarantee of completion within 180 seconds. Native Codex harnesses keep their
own compaction policy. Moon installs no wall-clock compaction timer.

Use the repository's [`SKILL.md`](../../SKILL.md) for agent operations and
[`docs/learning.md`](../../docs/learning.md) for configuration, daily
scheduling, freshness, review limits, schema-8 migration, and isolated CLI
rehearsals. See
[`docs/memory-improvement-plan.md`](../../docs/memory-improvement-plan.md) for
the metrics commands, privacy boundary, and multi-day recall evaluation before
changing retrieval policy.
