# Moon architecture

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

## Memory lifecycle

Completed sessions are recorded as immutable, secret-scrubbed evidence.
Distillation creates a smaller canonical claim linked to an exact evidence byte
and line range. Repeated claims confirm the existing memory; changed claims
require explicit supersession. The old claim remains auditable but is excluded
from normal retrieval.

Context assembly selects pinned summaries and relevant active memories, dedupes
chunk hits by memory, attaches bounded citations, and fits the result to an
explicit character budget. A bounded pinned-summary quota reserves capacity for
query-relevant recall. Remaining capacity can contain deduplicated document
excerpts with source and byte citations. These references remain separate from
reviewed canonical memories. Memory, evidence, and references are rendered as
untrusted data rather than agent instructions.

The OpenClaw adapter records one immutable evidence document per completed turn,
using a stable parent-session and content fingerprint. It keeps the user request
and final answer and omits intermediate tool traffic. A conservative Luna-medium
curator may propose at most three memories. Deterministic validation requires an
exact supporting quote, complete numeric support, sufficient term overlap,
minimum confidence and importance, and explicit correction intent before
supersession.

Each user-facing context assembly records an opaque local metric row with mode,
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

## Retrieval

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

The database uses WAL mode, foreign keys, bounded busy waits, transactional
document replacement, content hashes, and numbered schema migrations. A model or
dimensionality change requires an explicit re-embedding migration.

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
