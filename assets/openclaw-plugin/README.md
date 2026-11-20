# Moon OpenClaw adapter

This adapter registers the `moon` context engine, supplies one bounded SQLite
context packet for the current user message, and records completed turns for
selective durable-memory learning.

This 2.8.2 adapter requires the matching schema-20 binary and 2.8.2 skill. See
[audience rules and legacy review](../../docs/memory-privacy.md). For a first
installation, use [getting started](../../docs/getting-started.md). For an
existing installation, use [updating](../../docs/updating.md); do not copy this
file over a running plugin or link a mutable development checkout into
production.

## Installation and upgrade checks

The signed updater treats the binary, adapter, skill and database as one
compatibility set. Moon 2.8.2 retains the six-file bundle for older updaters.
Its companion documentation must be installed from the same reviewed release tag
when an agent needs local references. Refresh separately installed skill copies
as well. The upgrade guide explains which skill files are updated automatically
and how to preserve local edits.

After installation, check the following before relying on automatic memory:

1. The configured `moonPath` resolves to the intended binary and `moonHome`
   identifies the intended runtime. Inspect `moon --json --version` and health.
2. Moon owns the context-engine slot and the plugin has the required recall hook
   permissions. A loaded plugin alone does not prove native recall.
3. A new real chat turn receives the expected recalled context. Use the
   [isolated canary](../../docs/openclaw-canary.md) before live changes.
4. L1 and, if enabled, L2 can use their configured routes. A healthy database
   does not prove model authentication or account access.

The recommended context cap is `maxChars: 8000`; the compiled fallback remains
3,500 for compatibility. The cap is a ceiling, so small results do not get
padded. Source/topic preparation has a separate input budget. See
[packet selection](../../docs/packet-selection.md) for the measured trade-off.

## Recall and ownership

With an explicit `plugins.entries.moon.hooks.allowConversationAccess=true` grant
and prompt injection allowed, recall uses `before_prompt_build` for both native
and stock runtimes. The hook returns a defensive `prependContext` packet;
`assemble` then passes messages through to prevent duplicate injection. This
fixes OpenClaw 2026.9.7 native-owned routes, which omit context-engine assembly.
Without the hook API or permission, stock assembly can retrieve public memories
only. Native routes that skip assembly require the authorised prompt hook. See
[native recall compatibility](../../docs/openclaw-canary.md#native-recall-compatibility)
for the concrete deployment settings and synthetic acceptance check.

The adapter keeps strict ownership boundaries:

- Chat recall retrieves eligible public and private memories with
  audience-checked evidence.
- The selected agent harness continues to own automatic transcript compaction.
  Moon delegates explicit compaction only for an identified stock OpenClaw
  harness and safely refuses the lossy generic fallback for Codex or an unknown
  harness.
- Moon also registers the optional `moon-local` compaction provider. When
  selected through `agents.defaults.compaction.provider`, it replaces only the
  safeguard summarization call. OpenClaw still chooses safe transcript and tool
  boundaries, preserves recent turns, writes checkpoints, and owns rollback.
- Retrieval failures fail open by default and preserve the original messages.
- The durable `commitTurn` hook stores the accepted human message for its
  recorded owners with a verified speaker identity. It excludes assistant
  answers and tool traffic from learning. Confirmed Discord replies can be
  recalled as role-labelled history by their verified delivery owners. OpenClaw
  supplies the closed transcript range; Moon never rereads the growing
  transcript. Its advancement identity and evidence commit in one SQLite
  transaction, so retries cannot duplicate evidence or learning.
- With schema 20 and a
  [personal profile](../../docs/memory-privacy.md#personal-openclaw-profile),
  authenticated Discord, Control UI, TUI and enrolled direct chat messages can
  share the same private owner. Private recall uses verified Discord delivery.
  Fresh posts by other bots in configured Discord text rooms can enter the
  profile owner's L2 queue with bot authorship.
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
  rejected candidates remain pending. With schema 14, the scheduler continues
  through fresh batches and gives rejected evidence one routine retry on a later
  occurrence. Further attempts require related new evidence, changed grounding
  rules, or an explicit session review. Fixed failure phases and codes never
  include generated claims or provider error bodies.
- Greetings and irrelevant queries inject no context packet.
- The optional `moon_source_learn` tool explicitly processes one queued eligible
  source and submits verbatim proposals for separate operator review. It does
  not approve claims or run from the scheduler.
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

To use source extraction, the owner must allow the optional `moon_source_learn`
tool in the host tool policy and configure an explicit `moonHome`. Invoke it
with `{"scope":"project:canary","source_key":"canary-learning"}`; the key is
optional and scope is mandatory. The tool is available only to trusted owner
contexts with V2 current-invocation authority. It reloads L1's enabled state,
model, reasoning, fallback and timeout for each invocation; its fixed source
prompt does not reuse the optional session-L1 prompt file. It reads at most 64
KiB of body plus bounded provenance, disables model tools, and accepts only
strict bounded JSON with exact verbatim quotes. It returns a review job ID and
result hash without the captured text or lease token. Failures use fixed codes
and release the current attempt when possible. Review through
`source learning status` and explicit `source learning accept --yes` remains a
separate operator action. No login, automatic source worker or source lifecycle
action is added. See the [source workflow](../../docs/source-storage.md).

The optional `moon_topic_compile` tool uses the same owner authority, explicit
home, current-invocation, cancellation and review boundaries for topic jobs.
Invoke it with `{"scope":"project:canary","topic_key":"canary-storage"}` after
allowing it in host tool policy. It uses enabled L2 settings and a fixed typed
statement prompt, with a 64 KiB full-snapshot bound. Factual blocks must match
an accepted source claim or parent fact exactly; generated statements cannot be
promoted to facts. Interpretation/disagreement/question blocks stay labelled.
The tool submits a review result and never calls `topic accept`. See
[reviewed topics](../../docs/topics.md). Both optional tools are declared in the
plugin manifest; neither adds scheduled source/topic processing.

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
`max_output_tokens`; Moon cannot enforce that limit on native routes. Supported
stock routes receive an advisory provider request. Input size, timeout, actions,
daily batches, and attempts have separate limits. A successful model-route probe
does not establish synthesis quality or an exact subscription-usage budget.

OpenClaw 2026.9.7 treats a non-empty `streamParams` object as a transport
override that excludes native Codex. Moon consults the supported runtime-policy
helper and omits the advisory output-token hint for native runtimes and
implicitly routed OpenAI models. Explicit stock runtimes and ordinary local
providers retain the hint. Moon leaves route and authentication selection to
OpenClaw instead of forcing a runtime or changing credentials. L1 and L2
failures report fixed phase/code diagnostics without provider error bodies.

Moon 2.7.0 verifies the returned provider/model for each model helper, rejecting
confirmed route mismatches or unexpected host fallback before submission.
Source/topic results include `model_execution`; daily-L2 apply records the same
optional `metadata.execution`. Requested reasoning is kept separate from
observed execution, with missing values represented as `null`. In particular,
missing fallback metadata does not mean zero fallback attempts. Known
setup/auth/route failures stop Moon fallback; source/topic jobs retain a
terminal fixed diagnostic until explicit retry. The binary and adapter must be
deployed together for schema-13 diagnostic codes. See
[the reusable canary](../../docs/source-learning-canary.md).

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
Older eligible history is summarised before the next request. Both host and
model compaction deadlines are five minutes to allow existing backlogs more
time. The byte limit is a starting point for local-model testing, not an exact
token count or a completion guarantee. Native Codex harnesses keep their own
compaction policy. Moon installs no wall-clock compaction timer.

Use the repository's [`SKILL.md`](../../SKILL.md) for agent operations and
[`docs/learning.md`](../../docs/learning.md) for configuration, daily
scheduling, freshness, review limits, migrations, and isolated CLI rehearsals.
See [`docs/memory-improvement-plan.md`](../../docs/memory-improvement-plan.md)
for the metrics commands, privacy boundary, and multi-day recall evaluation
before changing retrieval policy.
