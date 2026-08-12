# Performance validation

Moon reports warm query latency through `benchmark`. Performance claims must
include corpus size, chunk count, vector dimensions, provider, query mode, build
profile, and hardware.

Measure both the underlying search and the complete warm stdio request. The
second includes query embedding, context assembly, citation loading, rendering,
and transport, and is the better estimate of per-turn Moon overhead. Cold model
startup is a separate gateway-lifecycle measurement.

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

For production-quality observation, use `--provider local` against a backup or
isolated copy with the same embedding model and dimensions. Follow
[memory-improvement-plan.md](memory-improvement-plan.md) for the reviewed query
corpus, privacy rules, and acceptance gates.

The live content-free collector reports complete context-assembly latency with:

```bash
moon metrics summary --since 7d
```

These p50/p95/p99 values include search, assembly, citation loading, and
rendering inside Moon. They exclude adapter transport and message insertion. Use
a separate canary when claiming end-to-end OpenClaw latency.
