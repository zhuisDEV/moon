# Memory improvement observation plan

## Purpose

Moon v2 is operationally healthy and fast. The next improvement phase should
focus on recall quality and durable-memory lifecycle behavior, using several
days of real usage before changing retrieval policy.

Do not tune the engine around one conversation or a small synthetic benchmark.
First build a reviewed set of representative successes and failures.

## Current concerns

1. **Natural-question false negatives.** The relevance guard can reject a
   semantically correct vector result when an ordinary sentence-initial word is
   mistaken for a named-entity anchor.
2. **Uneven typo tolerance.** Singular/plural normalization and bounded spelling
   fallback work for many active-memory queries, but not every combination of
   paraphrasing and misspelling.
3. **Explicit corrections can remain evidence-only.** A changed canonical claim
   correctly stops at a conflict, but the automatic flow may leave the older
   active wording in place when a user clearly supplied a replacement.
4. **Semantic redundancy.** Related claims can receive different canonical keys,
   causing overlapping memories to compete for the context budget.
5. **Packet density.** Redundant memories and long citations can consume the
   3,500-character packet even when fewer, sharper claims would be sufficient.
6. **Long-window behavior remains lightly exercised.** Current conversations
   have not yet forced a representative native transcript-compaction event.

These are quality concerns, not evidence of storage loss, queue failure, or
unacceptable latency.

## Observation period

Observe normal use for 3–7 days and at least 50 context requests spanning
multiple channels and task types. Extend the window when fewer than 20 requests
have a known relevant memory; zero-result queries with no relevant memory do not
measure recall.

For each reviewed case, classify the outcome:

- **useful:** the packet supplied the needed memory or reference;
- **partial:** relevant context appeared but was incomplete or crowded;
- **false negative:** a relevant active memory existed but was not returned;
- **false positive:** injected context was unrelated to the task;
- **correct empty:** no relevant memory existed, so no packet was appropriate;
- **stale:** an older active claim was returned after an explicit correction;
- **redundant:** multiple results repeated materially the same claim.

Keep reviewed queries private. Store only redacted examples in a regression
corpus, and never add credentials, private conversation bodies, or raw model
prompts to repository fixtures.

## Metrics to collect

Track the following per observation window and by channel only when useful:

- completed turns versus immutable evidence records;
- eligible turns versus proposed and accepted durable memories;
- automatic embedding count, queue depth, retry, and dead-letter state;
- context-request count, packet-injection rate, and correct-empty rate;
- useful, partial, false-negative, false-positive, stale, and redundant cases;
- expected-memory rank for reviewed queries;
- lexical, vector, and hybrid outcomes for every reviewed miss;
- warm worker and hybrid-search p50/p95/p99 latency;
- packet characters, truncation frequency, and repeated-memory share;
- canonical-key conflicts and whether an explicit correction was eventually
  superseded;
- native compaction events, successor-turn success, and confirmation that Moon
  packets never entered the stored transcript or summary.

Operational logs should contain counts, timings, identifiers, and redacted
errors—not query bodies or recalled content.

## Decision gates

Do not change production retrieval until the reviewed corpus can reproduce the
problem and distinguish it from a correct empty result.

An improvement is ready when:

1. the reviewed regression set improves without weakening named-entity safety;
2. expected memories rank within the top three for at least 90% of relevant
   reviewed queries;
3. false positives do not increase materially;
4. explicit user corrections either supersede safely or produce a visible,
   actionable review state;
5. warm context-request p95 remains below 50 ms on the representative live
   corpus;
6. health retains complete active-memory/reference coverage, zero evidence
   vectors, and no failed or dead embedding work; and
7. an isolated long-window canary confirms native compaction preserves the
   successor conversation without persisting Moon packets.

## Likely implementation order

After the observation period:

1. fix interrogative and sentence-initial anchor classification, then add the
   reviewed false-negative cases as regression tests;
2. improve bounded typo handling only where the reviewed corpus demonstrates a
   gap;
3. add a safe review or retry path for explicit canonical corrections;
4. consolidate semantically overlapping active memories without silently merging
   distinct user preferences;
5. improve packet selection or reranking before increasing the character budget;
   and
6. run the full isolated OpenClaw canary, recall corpus, health audit, and
   latency benchmark before release.

Do not introduce QMD, a remote embedding endpoint, another writable memory
store, or a second long-running service to solve these issues.
