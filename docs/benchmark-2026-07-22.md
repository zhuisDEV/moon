# Representative benchmark — 2026-07-22

This is an early engine benchmark, not a cutover approval.

## Corpus

- read-only source: current local Moon memory and projection directories
- source files: 409
- indexed chunks: 6,000
- embedded vectors: 6,000
- vector dimensions: 384
- database size: 45 MB
- build: optimized release
- embedding provider: deterministic `moon-hash-v1`
- SQLite: 3.51.3 bundled into the binary
- vector extension: sqlite-vec 0.1.9 statically linked

## Warm query latency

Representative query: `context assembly memory performance`, 200 iterations.

| Mode     |      p50 |      p95 |      p99 |
| -------- | -------: | -------: | -------: |
| lexical  | 0.102 ms | 0.118 ms | 0.171 ms |
| semantic | 2.113 ms | 2.331 ms | 2.399 ms |
| hybrid   | 2.916 ms | 3.115 ms | 3.312 ms |

## Correctness and isolation

- database integrity: `ok`
- pending embeddings: 0
- shadow query returned eight native and eight direct-legacy results
- four of eight result source files overlapped for the sample query
- old `MEMORY.md` and `moon_state.json` hashes were unchanged before and after
  import

## Caveat

The hash provider validates storage, vector execution, filtering, fusion, and
latency without network noise. It does not establish semantic recall quality.
Semantic acceptance requires the Codex-authenticated rerank path and a larger,
reviewed query set.
