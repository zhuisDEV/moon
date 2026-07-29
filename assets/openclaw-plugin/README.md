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
- Luna-medium may propose at most three durable memories; exact-quote,
  numeric-entailment, confidence, importance, and correction checks run before a
  proposal reaches SQLite.
- Greetings and irrelevant queries inject no context packet.

Lexical and hybrid retrieval both require no credentials and do not transmit
memory content. Hybrid mode uses Moon's local multilingual model through one
private stdio child owned by the adapter. The child has no network port and is
stopped when the adapter is disposed. Raw evidence is not embedded; active
memories are queued ahead of reference documents. Hash vectors exist only for
offline plumbing tests.

All model work uses one strict authentication resolver: OpenClaw's session-bound
Codex runtime first, Moon's isolated Codex login second, and the normal local
Codex login last. It falls through only for authentication failures, never for
rate limits, timeouts, network failures, or model errors. Turn transcripts and
proposals sent to the Moon binary use stdin and are not exposed in process
arguments.

OpenClaw keeps native automatic transcript compaction because the adapter
advertises `ownsCompaction=false`. Moon owns retrieval and bounded packet
assembly, not transcript replacement.

Use the repository's [`SKILL.md`](../../SKILL.md) for agent operations and
[`docs/memory-improvement-plan.md`](../../docs/memory-improvement-plan.md) for
multi-day recall observation before changing retrieval policy.
