# Moon production status

## Active production

Moon v2 became the active OpenClaw context engine on 2026-07-29:

- installed command used by OpenClaw: `/Users/lilac/.moon/bin/moon`
- formal source checkout: `/Users/lilac/gh/moon`
- runtime: `/Users/lilac/.moon`
- database: `/Users/lilac/.moon/state/moon.sqlite`
- OpenClaw slot: `moon`
- Moon release: `2.0.0`
- retrieval mode: hybrid, using local `intfloat/multilingual-e5-small`
- model policy: `gpt-5.6-sol` high, with `gpt-5.6-luna` medium for speed
- legacy watcher: stopped and retired
- rollback runtime: `/Users/lilac/.moon-legacy-20260728T080052Z`
- rollback bundle:
  `/Users/lilac/.openclaw/backups/moon-cutover-20260728T080052Z`
- pre-learning and post-deploy recovery bundle:
  `/Users/lilac/.openclaw/backups/moon-learning-20260728T093307Z`
- pre-local-embedding rollback bundle:
  `/Users/lilac/.moon-backups/20260728T113931Z`
- post-local-embedding recovery bundle:
  `/Users/lilac/.moon-backups/20260728T121112Z-post-local-embedding`
- pre-v2 formal-release recovery bundle:
  `/Users/lilac/.moon-backups/20260728T174748Z-pre-v2-formal`
- post-v2 formal-release recovery bundle:
  `/Users/lilac/.moon-backups/20260728T175422Z-post-v2-formal`
- retired v1 source checkout: `/Users/lilac/gh/moon-legacy-source-20260729`
- retired v2 prerelease checkout: `/Users/lilac/gh/moon-v2-prerelease-20260729`

The live schema is v6. All 4 active-memory chunks and 6,000 eligible reference
chunks have local vectors. Raw evidence has zero vectors by design. The
OpenClaw-owned worker stays warm across turns and has no listening port.

## Implemented foundation

- isolated runtime and database paths
- numbered transactional SQLite migrations
- WAL mode, integrity checks, online backup, and FTS rebuild
- canonical structured memory and JSON runtime state
- incremental content-hash ingestion and UTF-8-safe chunking
- FTS5 lexical retrieval
- statically linked vector retrieval with model-space enforcement
- reciprocal-rank hybrid fusion
- offline deterministic vector-plumbing provider
- local multilingual E5 embeddings with separate query/document paths
- tokenizer-safe long-chunk pooling with stable source offsets
- private persistent stdio worker owned by the OpenClaw adapter
- three-level Codex authentication resolution without token copying
- strict auth-only fallback; operational model errors remain visible
- read-only legacy import and shadow comparison
- generated Markdown memory export
- representative latency benchmark command
- immutable completed-session evidence with conservative secret scrubbing
- evidence-backed distillation with canonical-key confirmation
- explicit conflict and supersession handling
- exact byte/line citations from memories to session evidence
- pinned-summary and hybrid-ranked bounded context packets
- schema-enforced single active canonical head and cycle rejection
- atomic evidence and distillation transactions with immutable revisions
- active-memory filtering before FTS candidate limits
- prioritized leased embedding work with retry/backoff, dead-letter diagnostics,
  source-kind coverage, and model-transition locking
- automatic post-turn embedding with raw evidence explicitly excluded
- read-only logical health checks and owner-only local storage permissions
- structured untrusted-data context rendering and bounded input sizes
- bounded cited reference fallback for imported and indexed documents
- isolated OpenClaw context-engine adapter with fail-open retrieval
- automatic completed-turn evidence capture with stable deduplication
- selective Luna-medium memory distillation through the Codex auth chain
- exact-quote, numeric-entailment, confidence, and correction gates
- empty-packet and named-entity relevance gating with a 3,500-character budget
- singular/plural normalization and bounded typo fallback for active memories
- explicit native OpenClaw compaction ownership during the canary
- native-harness compaction guard preventing the lossy generic Codex fallback

## Follow-up lifecycle work

The production retrieval path is active. These lifecycle features remain
follow-up work and are not silently emulated:

- remaining project/cleanse lifecycle compatibility
- installation, daemon management, status, repair, and update flows
- expanded long-run semantic recall-quality corpus and optional reranking
- model-specific token rather than character budgeting in the OpenClaw adapter
- durable retry scheduling for evidence recorded immediately before a gateway
  shutdown interrupts model-assisted distillation
