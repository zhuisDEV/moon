# Local embedding acceptance benchmark

Measured on the production Moon corpus on 2026-07-28:

- database: 422 pre-canary documents and 6,024 chunks
- eligible vectors: 6,004
- active-memory coverage: 4/4
- reference coverage: 6,000/6,000
- evidence vectors: 0
- retrying or dead jobs: 0
- model: `intfloat/multilingual-e5-small`
- dimensions: 384

## Startup decision

The first isolated run, including model download, took 22.29 seconds. A cached
one-shot hybrid CLI request took 0.95–1.01 seconds because it reloaded ONNX and
the model for each process. That exceeded the 250 ms cold-query threshold in the
reviewed plan.

Moon therefore uses one private JSON-lines child owned by the OpenClaw plugin
lifecycle. It has no port and is stopped with the gateway. A live check showed
the same worker PID across separate OpenClaw turns.

## Warm query result

Against the full production database, 50 hybrid iterations reported:

- p50: 9.54 ms
- p95: 11.75 ms
- p99: 12.27 ms

The timing covers query embedding, FTS/vector retrieval, and reciprocal-rank
fusion inside one warm Moon process. It does not include the surrounding model
answer generation.

## Operational result

The full model transition requeued and embedded all 6,004 eligible chunks while
live retrieval stayed lexical. The final health report had zero pending, leased,
retrying, dead, or failed jobs and zero logical, FTS, vector, queue, or
foreign-key violations.
