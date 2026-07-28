# Performance validation

Moon reports warm query latency through `benchmark`. Performance claims must
include corpus size, chunk count, vector dimensions, provider, query mode, build
profile, and hardware.

Minimum release measurements:

- incremental unchanged-document ingest
- changed-document replacement
- FTS5 lexical p50/p95/p99
- vector p50/p95/p99
- hybrid p50/p95/p99
- context-packet p50/p95/p99 at representative character budgets
- citation coverage and superseded-memory leakage rate
- WAL database size after checkpoint
- full re-embedding throughput, excluding remote-provider queue time

The `hash` provider makes repeatable local vector benchmarks possible without
network latency. Retrieval-quality approval must use the intended production
embedding model and a representative query set.

Suggested command:

```bash
cargo run --release -- --home /tmp/moon-shadow benchmark \
  --query "representative memory query" \
  --mode hybrid \
  --provider hash \
  --iterations 500
```
