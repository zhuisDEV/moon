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
- Retrieval failures fail open by default and preserve the original messages.
- The after-turn hook stores only the user request and final assistant answer,
  not intermediate tool traffic.
- The configured primary or fallback model may propose at most three durable
  memories; exact-quote, numeric-entailment, confidence, importance, and
  correction checks run before a proposal reaches SQLite.
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

All model work stays inside OpenClaw's provider runtime. The adapter inherits
OpenClaw's primary model and first fallback unless provider-qualified overrides
are configured. Both reasoning levels default to `off` and may be overridden
independently. Moon owns no provider credentials, and bounded failures never
print arbitrary remote response bodies. Turn transcripts and proposals sent to
the Moon binary use stdin and are not exposed in process arguments.

OpenClaw keeps native automatic transcript compaction because the adapter
advertises `ownsCompaction=false`. Moon owns retrieval and bounded packet
assembly, not transcript replacement.

Use the repository's [`SKILL.md`](../../SKILL.md) for agent operations and
[`docs/memory-improvement-plan.md`](../../docs/memory-improvement-plan.md) for
the metrics commands, privacy boundary, and multi-day recall evaluation before
changing retrieval policy.
