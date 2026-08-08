# Changelog

## Unreleased

## 2.1.0 - 2026-08-08

- Added a canonical Moon v2 AI-agent skill and a multi-day observation plan for
  recall, correction, redundancy, packet density, and compaction quality.
- Removed completed status and dated benchmark reports; retained the reusable
  release canary, migration guide, and performance methodology.
- Refreshed the public repository, package, plugin, issue, and contribution
  metadata for the local-first Moon v2 architecture.
- Pinned the RustSec audit action to its upstream Node 24 migration, removing
  the deprecated action-runtime warning.
- Documented explicit binary-resolution checks for installation, operation, and
  release validation so a legacy Cargo binary cannot run against a newer Moon
  database schema.

## 2.0.0 - 2026-07-29

Moon v2 formally replaces the v1 QMD/watcher architecture with the
production-validated SQLite-native engine. The command, runtime root, and
OpenClaw context-engine id are now consistently named `moon`.

- Added local multilingual E5 embeddings with distinct query/document paths,
  tokenizer-safe subsegment pooling, and no API-key dependency.
- Added a private persistent stdio worker so OpenClaw gets warm hybrid queries
  without a port or separately installed daemon.
- Added schema v6 priority queueing, bounded retry/backoff, dead-letter status,
  automatic post-turn draining, source-kind coverage, and explicit exclusion of
  raw evidence vectors.
- Added one-call batch distillation and lexical inflection, Unicode, and bounded
  typo tolerance for active-memory recall.
- Guarded explicit compaction by harness ownership after a real Codex canary
  proved OpenClaw 2026.7.1-2's generic fallback can summarize populated
  transcripts as empty.
- Created the isolated Moon Rust project.
- Added a single canonical SQLite store with WAL, migrations, structured memory,
  runtime state, FTS5, statically linked vectors, and hybrid retrieval.
- Added a deterministic offline embedding provider for vector plumbing tests.
- Added read-only legacy import, shadow comparison, health, backup, FTS rebuild,
  re-embedding, export, and benchmark commands.
- Added isolation, migration, security, and performance contracts.
- Added immutable, secret-scrubbed session evidence and exact memory citations.
- Added canonical-key distillation, idempotent confirmation, explicit
  supersession, and stale-memory filtering.
- Added bounded context packets that prioritize pinned summaries, combine
  lexical/vector recall, deduplicate memory chunks, and include cited evidence.
- Added a workflow-first explanation of evidence, durable memory, context
  assembly, correction, backup, and integration boundaries.
- Added schema v5 lifecycle invariants, immutable revision identities, and
  single-transaction evidence/distillation writes.
- Fixed active-memory filtering before FTS ranking and reserved context capacity
  for query-relevant recall when many summaries are pinned.
- Added owner-only runtime, backup, and export permissions on Unix.
- Made health read-only and non-creating, with foreign-key, lifecycle, citation,
  FTS, and failed-embedding checks.
- Added leased embedding jobs, model locking, retry diagnostics, requeue
  protection, finite-vector validation, and bounded batches.
- Expanded secret scrubbing, structured JSON errors, bounded inputs, expired
  memory export filtering, and explicit untrusted-data context rendering.
- Added migration rollback, concurrency, supersession restoration, retrieval
  saturation, permission, redaction, queue, health, and CLI regression tests.
- Separated imported `legacy-memory` documents from canonical structured
  `memory` rows and added adaptive lexical fallback without regressing the
  normal fast FTS path.
- Added a bounded, source-cited reference lane so imported legacy documents can
  contribute context without being mislabeled as reviewed canonical memories.
- Added a separate OpenClaw context-engine adapter with fail-open retrieval,
  native OpenClaw compaction ownership, unit tests, and isolated-profile
  validation.
- Fixed chunk byte offsets around trimmed Unicode content and added
  backward-compatible citation repair for already imported databases.
- Added strict OpenClaw, Moon, then local Codex authentication resolution.
  Removed the direct OpenAI API-key embedding route so model credentials have
  one owner-controlled path.
- Kept byte and line citations exact when a context-budget boundary truncates a
  reference or evidence quote.
- Promoted the validated SQLite runtime to `~/.moon`, installed Moon as `moon`,
  activated the `moon` OpenClaw context engine, and retired the legacy watcher
  and plugin while retaining a dated read-only rollback bundle.
- Relocated imported source URIs to the retained legacy runtime so exact
  citations remain navigable after promotion.
- Added automatic completed-turn evidence capture and conservative Luna-medium
  durable-memory distillation to the OpenClaw adapter.
- Added greeting suppression, empty context packets, named-entity relevance
  anchors, a smaller default packet budget, and weak relaxed-match rejection.
- Added exact evidence-substring, numeric-entailment, confidence, importance,
  and explicit-correction gates before automatic memory writes.
- Prevented an assistant's recalled answer from becoming circular confirmation
  evidence for the same active memory.
- Delegated manual and overflow compaction to OpenClaw's native runtime instead
  of returning an unsafe no-op from the non-owning context engine.
- Added stdin-only turn and proposal payloads plus an isolated OpenClaw session
  replay tool that does not send channel messages.
