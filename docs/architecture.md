# Moon architecture

User setup is in [Getting started](getting-started.md); existing installations
should follow [Updating](updating.md). Moon 2.9.4 produces schema 21. This page
describes the engineering contracts; historical benchmarks are documented
separately in [Performance](performance.md).

Moon was built as a greenfield, self-contained memory engine and became the
active Moon runtime on 2026-07-28. It does not share a writable database or
process with the retired legacy implementation.

## Ownership boundary

- `moon.sqlite` is the canonical store for structured memory, chunks, indexes,
  embedding jobs, content-free context metrics, and runtime state.
- SQLite FTS5 provides lexical retrieval.
- `sqlite-vec` is compiled into the Rust binary and provides vector retrieval;
  there is no QMD, Node.js, Bun, MCP, or vector-server process.
- Raw imported transcripts and reference documents remain immutable evidence.
- Imported durable Markdown uses the `legacy-memory` source kind; canonical
  structured claims use `memory` and own a `memory_items` lifecycle row.
- Markdown memory views are exports, not a second writable source of truth.

Explicit source captures have immutable revisions and separate lifecycle rows.
An eligible capture atomically queues source extraction; it never becomes a
completed-session evidence record. Schema 10 stores leased jobs, pending review,
accepted verbatim statements and typed source support in the same database.
Accepted statements use the reserved `source_claim` document kind for indexes
and remain attributed references, separate from personal canonical memories.

## Memory lifecycle

Completed sessions are recorded as immutable, secret-scrubbed evidence.
Scrubbing also covers sensitive quoted keys in JSON embedded in prose or code
fences. Distillation creates a smaller canonical claim linked to an exact
evidence byte and line range. Repeated claims confirm the existing memory;
changed claims require explicit supersession. The old claim remains auditable
but is excluded from normal retrieval.

`distill-batch` commits all proposals in one SQLite transaction. A later invalid
quote or conflicting head rolls back earlier creations, confirmations and
supersessions, so a failed batch leaves no partial memory changes.

The OpenClaw main model selected for the turn decides whether saved facts or
original messages can answer the request. It can ask for clarification or skip
recall entirely. Moon intersects the requested sources with the verified
audience before retrieval. Up to three grounded searches share one result limit
and character budget. For a permitted historical question, a current claim can
carry dated originals and up to two linked earlier revisions. The same model
selects supporting record IDs. Moon validates those IDs and rechecks private
access before handing the chosen evidence to the answer. See
[the audience rules](memory-privacy.md).

Operator-scoped context can also include reviewed source claims and topics. Its
assembly selects pinned summaries and relevant active memories, dedupes chunk
hits by memory, attaches bounded citations, and fits the result to an explicit
character budget. A bounded pinned-summary quota reserves capacity for
query-relevant recall. Remaining capacity can contain deduplicated document
excerpts with source and byte citations. These references remain separate from
reviewed canonical memories. Memory, evidence, and references are rendered as
untrusted data rather than agent instructions.

The adapter delivers the selected chat records through an explicitly permitted
`before_prompt_build` hook on current OpenClaw hosts, so native-owned model
routes can recall even when the host skips context-engine assembly. That mode
makes `assemble` a pass-through to avoid duplicate injection. Legacy assembly
can supply explicitly published records when the host calls it with a resolved
model route.

For an eligible completed chat turn, the adapter records the accepted human
message as immutable private evidence under its verified speaker and original
owners. A confirmed Discord reply has a separate delivery-time ownership
snapshot and can be recalled as assistant history. L1 interprets the human input
from a single turn. L2 can interpret a short human acceptance when Moon can link
it to the exact preceding, independently verified delivered proposal. L2 can
also reconcile several original messages from one scope and group the claims
affected by a correction. The models decide which facts follow from the evidence
and when an ambiguous reference needs review. The adapter verifies exact quotes
and proposal bounds. SQLite verifies scope, evidence, targets and freshness and
commits the accepted changes in a transaction.

A saved-fact retrieval records one local metric row for its searches, with mode,
latency, result counts, packet size/truncation, and optional injection/review
state. The schema intentionally has no query, prompt, response, recalled
content, source, scope, channel, session, credential, or arbitrary-error field.
Internal curator lookups are excluded so observation counts represent actual
context requests. A separate content-free event table records learning,
embedding, and compaction counts without turn or session identity.

See [how-it-works.md](how-it-works.md) for the operator workflows and examples.
Use [memory-improvement-plan.md](memory-improvement-plan.md) to evaluate recall,
correction, redundancy, packet density, and long-window behavior before changing
these contracts.

## Source learning lifecycle

The CLI exposes bounded prepare, submit, inspect and accept/reject operations.
The optional OpenClaw `moon_source_learn` tool invokes the configured L1 route
for one explicitly requested eligible source and submits proposals only. Host
owner and current-invocation checks apply before reads, model attempts, writes
and disclosure. Source text remains untrusted input. A model cannot grant its
own learning permission or accept its output.

Each proposed statement is a verbatim retained quote. Rust verifies exact UTF-8
ranges; an operator reviews its surrounding meaning before acceptance. Each
independent support covers the whole unchanged statement. Matching statements
can share supports without turning duplicate sources into confidence or truth.
Author statements, attributed user statements and generated statements remain
distinct. Reviewed topics consume these accepted statements and current topic
revisions under the separate [topic contract](topics.md); logical combinations
of partial support never become verbatim source claims.

Jobs use bounded attempts, retry delays and expiring leases with a new token per
attempt. Snapshot revision and lifecycle generation are rechecked
transactionally before submission and acceptance. Acceptance commits statements,
citations, indexes and job completion together. A lost worker leaves recoverable
queued or leased work; source changes cancel obsolete attempts and reviews.

Withdrawal invalidates source support immediately. Claims without remaining
support leave recall and embedding work in the same transaction; authoritative
support and expiry views also gate reads and embedding callbacks. Restoration is
an explicit generation-checked operation and can revive previously accepted,
still-current support. Lifecycle history retains reasons until explicit
deletion. Deletion previews fingerprint affected sources, statements, supports,
jobs and history; applying a stale plan fails. Independently supported
statements survive, including expired historical statements. Deleted content is
purged logically while a tombstone prevents accidental reimport. See
[source storage](source-storage.md) for the command contract and limits.

## Retrieval

Fresh reviewed topics use a bounded share of context: at most two topics, one
third of available reference slots and one quarter of the character budget.
Their compact Markdown renders complete typed statements and numbered exact
source-key/revision/byte pointers, deduplicating shared pointers across blocks.
Full premise quotations, provenance and input IDs remain in typed context JSON
and topic inspection. A topic that cannot fit is omitted whole; qualifications
are never trimmed. Ordinary evidence is retrieved separately so topic matches
cannot exhaust its candidate list.

Reference selection scores bounded candidates against meaningful query clauses,
with local inverse-document-frequency weighting to discount common project names
and boilerplate. Each clause must meet a lexical coverage floor; clauses are
balanced so a long Chinese clause cannot suppress a short English qualifier. The
exact normalised scope can establish its own collection name; another named
subject in an admitting query clause must occur in the evidence/title. Named
comparison fragments also require a second global topical match. This reference
matcher does not change personal-memory recall or correction matching. These
scores are ranking heuristics, not truth/confidence.

At most two strongly matched topics can contribute at most sixteen exact source
premises each. Every lookup rechecks the fresh topic and the same-scope valid
source revision, range and full statement. Selected original evidence is packed
before explanations; a topic can use remaining capacity or replace identical
factual evidence, but cannot shorten a selected source statement. Accepted
source claims are returned complete or omitted, even when a search matched a
later chunk. Identical source quotations are rendered once with all citation
metadata retained. Raw document excerpts and personal-memory citations retain
their existing bounded-excerpt behaviour. See the
[four-budget comparison](packet-selection.md) for measurements and limitations.

Lexical and vector retrieval produce independent ranked lists. Hybrid search
combines them with reciprocal-rank fusion, which avoids treating incomparable
BM25 and vector-distance values as though they shared one scale.

The vector provider runs `intfloat/multilingual-e5-small` locally through
FastEmbed and ONNX Runtime. Query and document paths are distinct, and
tokenizer-safe subsegments are pooled back to one vector per canonical chunk.
The OpenClaw adapter owns one private stdio child so the model remains warm.
There is no listening port, separately installed daemon, QMD process, or remote
embedding credential.

Lexical recall keeps FTS5 as the fast path. Shared normalization handles common
English inflections, and a bounded Damerau-Levenshtein fallback checks active
memories only when strict lexical retrieval is empty. Semantic retrieval covers
paraphrases and synonyms.

## Durability

The database uses WAL mode with `synchronous=FULL` on every writable connection,
foreign keys, bounded busy waits, transactional document replacement, content
hashes, and numbered schema migrations. Backup destination connections also set
`FULL` explicitly. The setting is connection-local and is reapplied on every
writer open; inspecting it through a separate connection does not measure the
writer's policy. A model or dimensionality change requires an explicit
re-embedding migration.

`FULL` requests the extra WAL sync at commit before reporting success.
Durability still depends on the operating system and storage honouring sync
requests; it does not replace backups or prove recovery from physical hardware
failure.

Evidence recording and canonical distillation use immediate single transactions.
Every changed canonical claim receives a new immutable revision identity. Schema
constraints allow only one active row per canonical key and reject supersession
cycles. Embedding workers claim expiring leases, prioritize active memories over
references, retry with bounded backoff, and lock the database model space before
local inference. Raw evidence is deliberately excluded from vectors.

The only unsafe boundary registers the statically linked SQLite vector extension
with SQLite. It is isolated in the store initialization path and covered by
database-open, vector-write, vector-query, migration, and backup tests.

`health` opens only an existing current-schema database and checks physical
integrity, foreign keys, canonical heads, supersession cycles, exact citation
ranges, FTS row consistency, failed/dead embedding jobs, and vector coverage for
active memories, references, and evidence. It never creates or migrates storage.

## Migration safety

Legacy import reads the old Moon root only. It never edits, truncates, or
deletes a legacy file. Cutover and deletion remain outside the Moon CLI.

The OpenClaw adapter is a thin bridge loaded by OpenClaw. It invokes the Rust
binary for evidence writes, uses a private long-lived stdio child for local
embedding and hybrid retrieval, and uses OpenClaw's session-bound embedded
runner for selective distillation with explicit reasoning levels. It does not
introduce another installed service or package-manager runtime. It advertises
`ownsCompaction=false`, so the selected agent harness keeps its native automatic
transcript lifecycle. Moon delegates explicit compaction only when the runtime
identifies the stock OpenClaw harness. It safely declines the generic fallback
for Codex or unknown harnesses because the current fallback can produce an empty
summary for a populated Codex transcript. Production loads the adapter from
`~/.moon/openclaw-plugin`.

The same adapter may register `moon-local` through OpenClaw's narrower
compaction-provider interface. That provider owns only summary generation and
runs a configured model with thinking off in an isolated raw-model session.
OpenClaw retains safe tool-pair and turn boundaries, recent-tail preservation,
quality checks, transcript writes, checkpoints, rotation, and rollback. This
does not change `ownsCompaction=false` or give Moon direct transcript-mutation
authority.
