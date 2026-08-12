# Memory quality evaluation protocol

## Status and purpose

Status: ready to begin after the schema-7 metrics build is installed. The
observation window has not started merely because this document or the collector
exists.

Owner: the Moon runtime owner. Record the actual start and end timestamps in the
review notes or exported metrics artifact for each evaluation window.

Moon is operationally healthy and fast. The next improvement phase focuses on
recall quality and durable-memory lifecycle behavior, using several days of real
usage before changing retrieval policy. Do not tune the engine around one
conversation or a small synthetic benchmark.

## Privacy boundary

Context metrics stay in the canonical local SQLite database. Moon records only:

- an opaque random request ID and timestamp;
- retrieval mode, success or failure, and elapsed microseconds;
- memory/reference counts, packet characters, and truncation state;
- whether the OpenClaw adapter injected the packet; and
- an optional bounded review label and expected rank.

Moon also records content-free operational events: completed-turn learning
status and counts, embedding batch counts and remaining queue depth, and native
compaction status with optional token counts. These events contain no turn,
session, channel, or memory identity.

Moon never stores the query, prompt, response, recalled content, source URI,
scope, channel or session identity, credentials, or arbitrary error text in the
metrics table. The adapter log contains the opaque ID and numeric result summary
so an operator can associate a private, contemporaneous review with the right
row. Exports contain the same content-free fields and are created owner-only.

## Observation window

Observe normal use for 3–7 days and at least 50 context requests spanning
multiple task types. Extend the window when fewer than 20 requests have a known
relevant memory. A zero-result request is only an automatic empty-packet
candidate; it becomes a **correct empty** only after human review.

Start and inspect a window with:

```bash
moon metrics summary --since 7d
moon metrics recent --since 7d --limit 20
```

The context log line includes `request=<opaque-id>`. Review representative
successes, every suspected miss, and every suspected irrelevant injection:

```bash
moon metrics review \
  --request <opaque-id> \
  --outcome useful \
  --expected-rank 1
```

Allowed outcomes are:

- **useful:** the packet supplied the needed memory or reference;
- **partial:** relevant context appeared but was incomplete or crowded;
- **false negative:** a relevant active memory existed but was not returned;
- **false positive:** injected context was unrelated to the task;
- **correct empty:** no relevant memory existed, so no packet was appropriate;
- **stale:** an older active claim was returned after an explicit correction;
- **redundant:** multiple results repeated materially the same claim.

Use `expected-rank` only when a specific expected memory can be ranked. Keep
reviewed queries private. Repository regression fixtures must be synthetic or
redacted and must never contain credentials, conversation bodies, or raw model
prompts.

## Metrics and interpretation

`moon metrics summary` reports:

- total, successful, failed, and empty-packet-candidate requests;
- adapter-observed injection and truncation rates;
- packet size and context-request p50/p95/p99 latency;
- reviewed outcome counts; and
- expected-memory top-three rate for rows with an expected rank;
- completed-turn evidence, eligibility, proposal, and acceptance counts;
- embedding batch success, throughput, and latest remaining queue depth; and
- native compaction attempts, failures, and completed events.

The collector measures volume, delivery, and performance automatically. Recall
quality still requires human labels: a low injection rate can be correct, and an
injected packet can still be a false positive, stale, or redundant.

For a reviewed miss, compare lexical, semantic, and hybrid `search` results
privately. Add only a sanitized reproduction to the regression corpus. Continue
to inspect embedding queue health and correction conflicts with `moon health`;
event counts complement but do not replace the current-state health check.

## Retention and export

Preview retention before deleting anything:

```bash
moon metrics prune --older-than 30d
moon metrics prune --older-than 30d --yes
```

The first command is a dry run. The second permanently deletes only matching
metrics rows; it does not remove memories, evidence, indexes, or runtime state.

Create a content-free owner-only artifact when a window needs to be retained:

```bash
moon metrics export \
  --since 7d \
  --destination /path/to/moon-metrics.json
```

## Current concerns

1. Natural-question false negatives caused by sentence-initial anchor handling.
2. Uneven typo tolerance across paraphrases and misspellings.
3. Explicit corrections that remain evidence-only instead of superseding an old
   canonical claim.
4. Semantically overlapping memories with different canonical keys.
5. Redundant claims or long citations consuming the 3,500-character packet.
6. Lightly exercised native transcript compaction over long conversations.

These are hypotheses to test, not evidence of storage loss, queue failure, or
unacceptable latency.

## Decision gates

Do not change production retrieval until the reviewed corpus reproduces a
problem and distinguishes it from a correct empty result. An improvement is
ready when:

1. the reviewed regression set improves without weakening named-entity safety;
2. expected memories rank within the top three for at least 90% of relevant
   reviewed queries;
3. false positives do not increase materially;
4. explicit corrections supersede safely or produce a visible review state;
5. warm context-request p95 remains below 50 ms on the representative corpus;
6. health retains complete active-memory/reference coverage, zero evidence
   vectors, and no failed or dead embedding work; and
7. an isolated long-window canary confirms native compaction preserves the
   successor conversation without persisting Moon packets.

Close a window only after recording its actual dates, request and review counts,
gate results, sanitized regressions, and one of these decisions: no change,
extend observation, implement a bounded fix, or reject the proposed change.

## Likely implementation order after observation

1. Fix verified interrogative or sentence-initial anchor misses and add them as
   regression tests.
2. Improve bounded typo handling only where the corpus demonstrates a gap.
3. Add a safe review or retry path for explicit canonical corrections.
4. Consolidate overlapping memories without silently merging distinct user
   preferences.
5. Improve packet selection or reranking before increasing its character budget.
6. Run the isolated OpenClaw canary, recall corpus, health audit, and latency
   benchmark before release.

Do not introduce QMD, a remote embedding endpoint, another writable memory
store, or a second long-running service for these improvements.
