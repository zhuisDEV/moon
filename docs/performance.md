# Performance validation

For configuration, start with [Getting started](getting-started.md) and
[Learning](learning.md). The measurements below are retained release evidence;
local `target/` reports and frozen binaries are not distributed release assets.
Use the versioned runners to regenerate synthetic corpora. The 8,000-character
recommendation is explicit configuration, not a change to the compiled default.

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

## Evidence-validity lookup profiling

The schema-12 optimisation addresses repeated work in joined source/topic
lookups. Previously, `source_valid_claims` joined supports and used `DISTINCT`.
In SQLite 3.51.3, some callers materialised all valid claims into a temporary
deduplication structure before looking up one requested claim. Context assembly
loads multiple candidate references, multiplying that cost. Direct lookups by
document ID were already indexed; it was the joined uses that caused the large
repeated scans.

Migration 12 replaces that view with one row per claim guarded by `EXISTS` over
valid support. Claim and document uniqueness already guarantee one result row.
The rewrite keeps read-time expiry and the existing support checks for current
revision, scope, eligibility and withdrawal. It introduces no validity cache,
omits no evidence check and changes no ranking or packet-selection rule.

Hot search and citation queries also reuse prepared SQL programs through
rusqlite's existing bounded statement cache. Each execution binds its current
parameters and evaluates live validity; no query result, source content or
expiry decision is cached. Profiling after the view rewrite found reference
metadata execution around 0.0035 ms but compilation plus execution around 0.171
ms for each candidate. Reusing that program avoids repeated compilation as well
as the collection-wide work removed by the view rewrite.

The versioned profiling example uses Moon's bundled SQLite, copied synthetic
snapshots, three warm-ups and twenty samples per SQL query. It records query
plans, elapsed time, VM operations, full-scan steps and sorts. It verifies that
the retained seed did not change and never opens a live Moon home:

```sh
cargo run --release --locked --example profile_retrieval -- \
  target/packet-runtime-seeds/10000.sqlite \
  migrations/0012_retrieval_validity.sql
```

Use `1000.sqlite` for the smaller retained corpus. The seeds come from the
[packet benchmark](packet-selection.md#runtime-check-and-deployment-gate). These
diagnostic build artefacts are retained locally; the versioned packet runner can
generate fresh corpora when they are absent.

At 10k references, the source-context identity lookup fell from about 3.46 ms to
0.0032 ms, with VM operations decreasing from 124,223 to 91 and full-scan steps
from 2,002 to zero. Topic validity fell from about 4.48 ms to 0.015 ms; exact
topic-premise lookup fell from 13.94 ms to 0.186 ms. Paired SQL row counts
matched. SQL timing alone does not establish complete packet performance.

An indexed scan remains in exact topic-premise lookup, so a zero full-scan
counter does not mean every query has constant cost. Broader corpus and query
distributions still need measurement; preserve these limitations when reporting
the complete packet benchmark below.

## Schema-12 packet comparison at 8,000 characters

On 16 November 2026, optimised release binaries were compared on the retained
1,000- and 10,000-reference corpora from the packet-selection experiment. Each
contains 80% raw references and 20% reviewed claims, plus three canary claims
and one topic. The machine was an Apple M1 iMac21,1, 8 cores, 16 GiB memory,
arm64, macOS 26.6.2. No compiler, model sweep or other benchmark ran
concurrently.

Both binaries used an 8,000-character cap, eight result slots and warm private
stdio with lexical search. The hash provider satisfies worker startup only; no
query embedding or provider-model call occurs. Each of four query types runs
three warm-up rounds and twenty-five measured rounds per variant/corpus. Paired
requests alternate before/after order. Migration and process startup are outside
the timed interval. This gives 400 measured requests per complete comparison.

| Stored references | Before p95 | After p95 | Before p50 | After p50 |
| ----------------- | ---------: | --------: | ---------: | --------: |
| 1,000             |  56.234 ms | 12.327 ms |  22.563 ms |  7.976 ms |
| 10,000            | 473.867 ms | 42.892 ms | 211.643 ms | 27.383 ms |

These figures exclude metric observation writes (`observe: false`), matching the
earlier benchmark. The measured retrieval path now meets the 50 ms warm p95
target at both corpus sizes. This is not a claim about production hybrid
embedding latency or real L2 generation. The full structured packets match
exactly between variants, including evidence, ordering, scores, source IDs and
citation ranges; repeated queries are stable. Scope, budget, expected
empty-result and health assertions pass, and retained seed hashes remain
unchanged.

A separate run with `observe: true` includes the durable local metrics write:

| Stored references | Before observed p95 | After observed p95 |
| ----------------- | ------------------: | -----------------: |
| 1,000             |           54.153 ms |          11.762 ms |
| 10,000            |          481.376 ms |          43.162 ms |

All 400 measured observed requests also preserve exact packet parity. Each of
the four worker databases retains 112 observations (100 measured plus twelve
warm-ups) after shutdown. The report is
`target/retrieval-runtime-observed.json`. Both modes meet the target on this
workload; small differences between separate runs are normal measurement
variation, not evidence that observation makes retrieval faster.

The final binaries are `target/retrieval-before-moon` (schema 11) and
`target/retrieval-after-moon` (schema 12). Their hashes, individual query
results and packet parity checks are in `target/retrieval-runtime-final.json`.
`target/retrieval-runtime.json` records the intermediate view-only experiment;
it is not the final result. The frozen fifteen-question quality experiment also
retains all 38 expected source units at 8,000 characters with no citation,
scope, lifecycle or budget violations
(`target/retrieval-after-cached-lexical.json`).

Reproduce the complete comparison, including durable context observation:

```sh
deno run --allow-read=target --allow-write=target \
  --allow-run="$PWD/target/retrieval-before-moon,$PWD/target/retrieval-after-moon" \
  tools/run-packet-runtime-benchmark.ts \
  "$PWD/target/retrieval-before-moon" "$PWD/target/retrieval-after-moon" \
  "$PWD/target/retrieval-reproduced.json" 1000,10000 25 \
  --seeds "$PWD/target/packet-runtime-seeds" \
  --budgets 8000 --require-parity --observe
```

Omit `--observe` to reproduce the table above. Omit `--seeds` to generate a
fresh synthetic corpus; intake is untimed and can take minutes. Use a new output
path for each run. Every run migrates only disposable copies under `target/` and
verifies the original seed/binary hashes. Observed mode compares the packet
without its unique request-ID wrapper and verifies every observation remains in
SQLite after the worker closes.

The final Rust suite passes 317 tests and the adapter suite passes 119. New
regressions cover old/new view equivalence, migration failure rollback and
long-lived prepared-statement reuse across scopes, kinds and limits. A second
connection expires a warmed claim using DML only, without a schema change;
subsequent lexical, semantic, hybrid and context requests immediately exclude
the affected claim and topic while retaining current alternatives.

This remains a synthetic workload on one machine. Common-word FTS relaxation and
some indexed scans still scale with collection size, and broader tasks or larger
databases can have different tails. No evidence checks were weakened to meet the
timing target. Live hybrid/model validation, production observation and an
authorised backed-up deployment remain separate work.

## Moon 2.7.0 patched SQLite release check

Release security auditing required SQLite 3.54.1 and refreshed TLS/QUIC
libraries. The earlier SQLite 3.51.3 results above remain historical. On the
same Apple M1 machine and retained synthetic corpora, the first patched build
preserved packets but regressed to 62.026 ms p95 at 10k references.

Profiling identified repeated automatic recompilation of cached per-result
metadata lookups as integer IDs changed. The queries now explicitly cast those
already-i64 IDs to INTEGER. This caches only the SQL program: scope, activity,
source revision, withdrawal, expiry and topic freshness are still checked every
execution. The source/topic views and database schema remain unchanged.

The versioned `profile_retrieval` example retains an alternating-ID/cache-return
reproducer with full-row equality checks. At each corpus size it verified 360
comparisons across 18 cases; the lexical metadata lookup changed from 360
reprepares to zero. Separate checks covered active/expired memories, missing IDs
and reference chunk/document mismatches. A cross-engine rehearsal preserved all
44 ordinary/shadow tables and 50 synthetic rows, exact citations,
FTS/hash-vector recall and restoration of backups from both engine versions.

The complete final packet run used schema 13 on both sides, warm private stdio,
lexical search, eight result slots, an 8,000-character cap and `observe: true`.
It reused the four query types, three warm-up rounds and 25 measured rounds,
alternating before/after requests. There were 400 measured requests, 200 paired
packet comparisons and 112 retained observations per worker database.

| Stored references | Deployed SQLite 3.51.3 p95 | Patched SQLite 3.54.1 p95 |
| ----------------- | -------------------------: | ------------------------: |
| 1,000             |                  11.562 ms |                 15.106 ms |
| 10,000            |                  42.363 ms |                 47.692 ms |

Both sizes meet the 50 ms warm lexical p95 target. The patched engine has a
small remaining latency cost in this workload; it is not a new speed
improvement. All structured packets, ordering, evidence and exact citation
ranges match; seed hashes, budget/scope checks and health pass. This does not
establish general answer quality or production hybrid embedding latency.

The retained report is `target/release-2.7.0-sqlite-final-runtime.json`. The
baseline binary SHA-256 is
`995360db6bca39cf6ece728caba1f834d3071ddc02d53d3bcca7fb5bedd2edaa`; the measured
patched candidate is
`7786e4b1389ea393c5460189fa924ee3d8af2c6e378b469141d8da62bc72da43`. Published
clean builds have their own commit-bound identities. The benchmark runner now
accepts schema 13 in addition to the retained schema-11/12 baselines; its
existing health, corpus-size, citation and parity gates remain enforced. Use the
runner command above with explicit before/after release binaries and a new
output path to reproduce this check.
