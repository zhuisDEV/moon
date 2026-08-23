# How Moon works

Moon turns completed work into small, cited context packets for later agent
turns. One Rust binary owns the full path. SQLite stores the evidence, memories,
indexes, citations, and runtime state; FTS5 and statically linked `sqlite-vec`
provide search. QMD and a separate vector service are not involved.

## Key workflows

### 1. After a completed turn: record immutable evidence

After OpenClaw successfully completes a non-heartbeat turn, the adapter keeps
the user request and final assistant answer, discards intermediate tool chatter,
and records that pair as evidence. A stable turn fingerprint makes retries
idempotent. The parent session and channel identity stay in sanitized metadata.

The same operation is available manually:

```bash
moon record \
  --session-id session-2026-07-27-001 \
  --scope moon \
  --title "Memory architecture decision" \
  < /path/to/completed-turn.txt
```

`record` performs conservative secret scrubbing, indexes the sanitized content,
and stores the completed session as immutable evidence. Repeating the same
command is harmless. Reusing the session id with changed content or metadata is
rejected instead of silently rewriting history. Document creation and evidence
identity are committed in one transaction.

OpenClaw calls the hook only after the turn has finished. Heartbeats and turns
without both a user request and final answer are skipped. Evidence can include
ordinary turns, but only durable candidates proceed to distillation.

### 2. Distill evidence into a durable memory

Evidence is history. A memory is the smaller claim Moon should recall. Distill a
stable claim with a canonical key and an exact supporting quote:

```bash
moon distill \
  --key moon:storage-engine \
  --kind decision \
  --scope moon \
  --title "Canonical storage" \
  --content "Moon uses one SQLite database and does not require QMD." \
  --session-id session-2026-07-27-001 \
  --evidence-quote "Moon uses one SQLite database and does not require QMD." \
  --importance 0.95 \
  --confidence 1.0 \
  --pinned
```

Moon stores the exact byte and line range of the quote. The memory can therefore
show where it came from instead of presenting an unsupported recollection.

In OpenClaw, local eligibility rules first reject greetings and trivial turns.
The adapter then asks the configured primary model, with reasoning off by
default, for at most three conservative proposals. It tries the fallback when
the primary request or structured output fails. A proposal is accepted only
when:

- its kind, key, confidence, and importance pass deterministic validation;
- its evidence quote is one exact substring of the completed turn;
- every numeric claim in the memory appears in that quote;
- the quote has sufficient wording overlap with the proposed claim; and
- any requested supersession points to an active retrieved memory and the user
  explicitly expressed a correction.

An assistant merely recalling an existing memory cannot confirm that same
memory. Confirmation of an active claim requires an exact quote from the user's
new message, preventing circular self-citation.

The model prompt stays inside OpenClaw's session-bound embedded runner. Proposal
payloads travel to the Moon binary through stdin, not process arguments.

Canonical keys drive consolidation:

- Same key and same content: confirm the existing memory, add the new citation,
  and raise confidence or importance when appropriate.
- Same key and different content: stop with a conflict. Nothing is replaced.
- Reviewed replacement: rerun with the active document id from the conflict, for
  example `--supersedes 42`. The old memory remains as history but is excluded
  from normal search and context packets.

This makes corrections explicit and prevents a newer sentence from quietly
overwriting an earlier decision. Every changed value receives a new immutable
revision, even when a later decision restores content used by an older revision.
SQLite permits only one active row per canonical key and rejects supersession
cycles.

Use `kind=summary` with `--pinned` for a short project summary. Pinned summaries
form the stable top layer of a project context packet.

### 3. Embed durable memory automatically

Every new active memory enters the embedding queue at high priority. Imported
and indexed reference chunks enter at normal priority. Raw turn evidence stays
lexical and citation-only: it is not duplicated into the vector index.

After a completed OpenClaw turn, the adapter asks a private Moon child process
to drain a bounded batch. The child communicates only through stdin/stdout; it
has no TCP port and is stopped with the adapter. Keeping that process alive
keeps the local model warm without adding a separately installed daemon. Queue
leases survive crashes, failures use bounded exponential backoff, and five
failed attempts become a visible dead-letter health condition. A valid memory
write is never rolled back because its later embedding failed.

The production model is `intfloat/multilingual-e5-small`. Moon uses distinct
`query:` and `passage:` inputs. Oversized chunks are split at tokenizer-safe
boundaries, embedded in subsegments, mean-pooled, and normalized back into one
vector, preserving the original chunk offsets and citations.

For an initial rebuild or model transition:

```bash
moon requeue-embeddings
moon --json embed --provider local --limit 64 --drain
```

`health` reports active-memory and reference coverage separately and confirms
that evidence has zero vectors.

### 4. Search locally first

Every recorded or distilled document is immediately searchable with SQLite FTS5.
This is the normal high-performance path and requires no model login, network
service, QMD process, or API key:

```bash
moon context \
  --query "How should Moon store and retrieve long-term memory?" \
  --scope moon \
  --mode lexical \
  --max-chars 3500
```

Production hybrid retrieval uses the local multilingual model and does not
require a model login, API key, network service, QMD process, or remote
embedding endpoint:

```bash
moon context \
  --query "How should Moon store and retrieve long-term memory?" \
  --scope moon \
  --mode hybrid \
  --provider local \
  --max-chars 3500
```

The optional `hash` provider remains only for deterministic vector-plumbing
tests and is not presented as semantic-quality recall.

One database contains one embedding space. Moon rejects mixed model names or
dimensions until an explicit re-embedding migration is performed.

Lexical retrieval uses FTS5 Porter stemming, shared lightweight inflection
normalization, Unicode lowercasing, and a bounded edit-distance fallback for
active memories when strict search is empty. This handles common singular,
plural, and spelling mistakes without scanning the full reference corpus. Hybrid
vectors add paraphrase and synonym recall.

### 5. Route model work through OpenClaw

Moon has no direct API-key model route or provider credential store. When a
model step is requested, the adapter uses:

1. `agents.defaults.model.primary` from OpenClaw, unless `primaryModel`
   overrides it.
2. The first OpenClaw fallback, unless `fallbackModel` overrides it.

Both values use OpenClaw's provider-qualified `provider/model` form. Providers
such as vLLM, OpenAI, Anthropic, or Google therefore use the same Moon path.
OpenClaw owns their authentication and transport.

If the primary request fails, the adapter tries the fallback once. If both fail,
learning follows the configured fail-open policy. Moon does not persist or print
provider diagnostics because they may contain credentials or arbitrary remote
response bodies. `primaryReasoning` and `fallbackReasoning` default to `off`
independently of provider. Their overrides are passed as OpenClaw `thinkLevel`
values, while visible reasoning output remains disabled, so the two routes may
use different effort without exposing hidden reasoning.

### 6. Before an agent turn: assemble a context packet

The agent asks Moon for context relevant to its current task:

```bash
moon context \
  --query "How should Moon store and retrieve long-term memory?" \
  --scope moon \
  --mode hybrid \
  --provider local \
  --max-chars 3500
```

The packet is assembled in this order:

1. A bounded quota of active pinned summaries for the requested scope, plus
   global pinned summaries.
2. Relevant active memories from lexical and vector retrieval.
3. Memory metadata such as kind, confidence, and canonical key.
4. A bounded number of exact evidence citations.
5. If result slots and character budget remain, deduplicated unreviewed document
   excerpts with source URI and exact chunk byte ranges.

Duplicate chunks collapse back to one memory. Superseded and expired memories
are excluded. Retrieved references never become canonical memories merely
because they were imported or ranked highly. If the character budget is tight,
Moon reduces evidence and truncates content while preserving UTF-8 boundaries.
The output explicitly fences all recalled content, evidence, and references as
untrusted data rather than agent instructions. Pinned context cannot consume
every result slot when relevant search matches exist.

Context candidates must cover enough meaningful query terms. Named entities and
dates act as anchors, preventing a generic chart note from satisfying a query
for Albert Einstein merely because both contain the word `chart`. The adapter
skips greetings before invoking Moon. If no relevant memory or reference
survives, the text command emits no packet and the adapter injects nothing.

Anchor filtering, typo tolerance, and semantic ranking must be evaluated
together against reviewed natural queries. See
[memory-improvement-plan.md](memory-improvement-plan.md) for the observation
period, classifications, and release gates; do not weaken the safety filter from
one isolated miss.

Adapters should use `--json` and keep the packet below trusted instructions.
Without `--json`, `context` emits defensive Markdown for inspection and
low-trust prompt data; its labels are not a security boundary.

### 7. Compact the active context safely

OpenClaw memory search and the legacy memory plugin remain disabled. Moon owns
retrieval and the bounded context packet, but it does not own transcript
compaction. The adapter advertises `ownsCompaction=false`, leaving automatic
compaction enabled in the selected agent harness. When the runtime explicitly
identifies the stock OpenClaw harness, manual compaction can delegate to its
native runtime. For Codex or an unknown harness, Moon refuses the generic
fallback because OpenClaw 2026.7.1-2 was observed to summarize a populated Codex
transcript as empty. A safe no-op is preferable to losing context.

Moon packets are ephemeral assembly input. They are not copied into the stored
transcript, do not become durable memory by themselves, and must not be
duplicated into a compaction summary.

OpenClaw 2026.7.1 also exposes a narrower compaction-provider seam. When
`agents.defaults.compaction.provider=moon-local`, the adapter sends only the
prepared safeguard-summary input to `compactionModel` in an isolated raw-model
session with `compactionReasoning=off` by default. The configured provider must
support its native thinking-off request shape. The call has no tools, Moon
retrieval, or hidden provider fallback. OpenClaw still chooses tool-safe chunk
boundaries, retains recent and split turns, appends the compaction entry, and
owns checkpoints, transcript rotation, and rollback. If the provider fails,
OpenClaw uses its explicitly configured built-in compaction model.

### 8. Correct stale memory

Suppose `moon:search-policy` currently points to document 42, but a later review
approves a new policy. The first changed `distill` call reports a conflict and
names document 42. After review:

```bash
moon distill \
  --key moon:search-policy \
  --kind decision \
  --scope moon \
  --content "Moon uses the newly approved semantic rerank policy." \
  --session-id session-2026-07-27-002 \
  --evidence-quote "Moon uses the newly approved semantic rerank policy." \
  --supersedes 42
```

The previous row and its evidence remain auditable. Only the new head is used
for recall.

### 9. Back up, export, and verify

Create a consistent SQLite backup before operational experiments:

```bash
moon backup \
  --destination /path/to/moon-backup.sqlite
```

Exporting memory creates a generated Markdown view. SQLite remains canonical:

```bash
moon export \
  --destination /path/to/MEMORY.export.md
```

Use `health`, `benchmark`, and `shadow` to verify database integrity, retrieval
latency, and behavior against the read-only legacy Moon corpus. `health`
requires an existing current-schema database; it never creates or migrates one.

## The three layers

### Evidence

Evidence is a sanitized, immutable completed-session record. It answers: “What
actually happened, and where did this memory come from?”

### Durable memory

A durable memory is a compact claim with a canonical key, kind, scope,
importance, confidence, validity, and optional pinned status. It answers: “What
should Moon recall next time?”

### Context packet

A context packet is an ephemeral, bounded selection of active memories and
supporting citations plus clearly separated unreviewed references for one query.
It answers: “What does this agent need right now?”

Keeping these layers separate is the important context-engine design principle.
Evidence is not injected wholesale, memory is not treated as unquestionable
truth, and the context window is not filled with everything Moon has stored.

## SQLite ownership

The main lifecycle tables are:

- `documents`, `chunks`, `chunk_fts`, and `chunk_vectors`: canonical content and
  retrieval indexes.
- `evidence_sessions`: immutable completed-session identities and metadata.
- `memory_items`: durable claims and lifecycle state.
- `memory_heads`: the current document for each canonical key.
- `memory_citations`: exact links from a claim to evidence byte and line ranges.
- `embedding_queue`: prioritized, leased, retryable vector work.
- `context_metrics`: content-free request performance, delivery, and bounded
  human review labels.
- `runtime_metrics`: content-free learning, embedding, and compaction events.
- `runtime_state`: adapter checkpoints.

Numbered migrations update these tables transactionally. Generated Markdown is
never a second writable source of truth.

On Unix, runtime directories are owner-only (`0700`) and SQLite databases,
backups, and exports are owner-only files (`0600`).

## Current integration boundary

The production OpenClaw adapter owns six bounded operations:

1. retrieve relevant context before a turn;
2. mark whether the content-free request metric was injected;
3. record the completed user/final-answer pair after a turn;
4. selectively distill evidence-backed durable memories; and
5. optionally generate a safeguard compaction summary through a configured local
   model; and
6. drain a bounded local-embedding batch after completed turns.

Retrieval and learning failures fail open by default and never suppress the
agent's reply. OpenClaw still owns its transcript and compaction lifecycle even
when Moon supplies the summary text. Moon does not copy tool traces or
reasoning, run a watcher, or require QMD.

Automatic extraction remains deliberately conservative. A changed canonical
claim is replaced only when the user explicitly corrects it and the extraction
proposal names the active head that was supplied for comparison. Other conflicts
remain recorded as evidence but do not overwrite durable memory.

Recall and lifecycle improvements follow the executable evaluation protocol in
[memory-improvement-plan.md](memory-improvement-plan.md). Automatic metrics
measure volume, packet delivery, and latency; human labels remain required for
recall-quality claims. The protocol is not authorization to change production
policy.
