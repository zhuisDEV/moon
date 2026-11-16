# Packet size and evidence selection

For current setup, use [Getting started](getting-started.md). This page retains
the historical selection experiment and its initial failed latency gate. The
subsequent
[retrieval optimisation](performance.md#schema-12-packet-comparison-at-8000-characters)
passed the 50 ms warm lexical p95 target at both corpus sizes with exact packet
parity. The recommendation is 8,000 characters; the compiled default remains
3,500. The earlier failed timing table below is not the final release result.

This experiment compares 3,500, 6,000, 8,000 and 14,000-character packets before
and after the focused evidence-selection change. It uses the unchanged reviewed
[topic fixture](topic-baseline.md): fifteen natural-language questions, 38
expected exact source units and manually prepared topic explanations. The
questions, scopes, evidence, labels, topic titles and reference limit of eight
stay fixed across budgets and binaries.

The frozen schema-11 **before** binary is `target/packet-before-moon`, SHA-256
`c451c3adcfc0ef1ea94b8ec9ea1c7b089e454bbafdbadf79f1c1b96711c93d87`. This is the
topic milestone's final candidate, not the older schema-10 source-only baseline.
The earlier baseline reports remain unchanged.

## Measurement boundaries

A complete source unit is the entire selected original quotation displayed as a
source reference. Partial quotations and complete prepared topic explanations
are counted separately. Full quotations available only inside structured topic
citation metadata do not count as displayed source units. Returned references
outside the selected labels are recorded as unlabelled; that does not by itself
mean they are irrelevant or incorrect.

Every packet is checked against the retained original UTF-8 byte ranges and line
numbers, exact scope, inactive/withdrawn exclusions, topic slot quotas and
character budget. Database health must pass. The experiments use isolated homes
and databases, clear inherited environment variables and make no provider-model
calls. No live database, configuration or plugin is changed.

Lexical and actual local-hybrid retrieval are measured separately. The optional
hybrid experiment copies the existing complete public embedding-model cache into
the temporary home and runs every Moon subprocess with macOS's `sandbox-exec`
network-deny profile. It does not download a model or write to the installed
cache. Missing model files cause an offline failure rather than a download. CLI
timings include process startup and, for hybrid queries, local model loading;
they are not warm production-service latency measurements.

This is a small diagnostic corpus with agent-prepared reference explanations. It
does not measure human answer usefulness, L2 synthesis quality, token billing,
or saved research time. The existing production observation and human-review
protocol remains necessary before claiming a general quality improvement.

## Reproduction

From the repository root, run the lexical comparison against an explicitly
selected binary and a distinct report path:

```sh
deno check tools/run-topic-baseline.ts
deno lint tools/run-topic-baseline.ts
deno run \
  --allow-read=tests/fixtures,docs,target/packet-before-moon \
  --allow-write=target \
  --allow-run="$PWD/target/packet-before-moon" \
  tools/run-topic-baseline.ts --packet-sweep \
  "$PWD/target/packet-before-moon" "$PWD/target/packet-before-lexical.json"
```

On macOS, to use an existing complete local cache without network access:

```sh
deno run \
  --allow-read=tests/fixtures,docs,target/packet-before-moon \
  --allow-write=target \
  --allow-run=/usr/bin/sandbox-exec,/bin/cp \
  tools/run-topic-baseline.ts --packet-sweep \
  "$PWD/target/packet-before-moon" "$PWD/target/packet-before-hybrid.json" \
  --hybrid-cache /absolute/path/to/models/fastembed
```

Use the corresponding after binary and new output paths for the candidate.
Reports include both hashes, actual packet text, per-question missing/partial
units, unlabelled references, original-source checks, used characters and
process-inclusive timings. Each run cleans up its temporary home and copied
cache, including after a failed assertion.

## Before-change lexical result

| Character limit | Complete source units | Partial units | Complete prepared explanations | Total rendered characters |
| --------------- | --------------------: | ------------: | -----------------------------: | ------------------------: |
| 3,500           |                 15/38 |             2 |                              1 |                    28,210 |
| 6,000           |                 17/38 |             1 |                              3 |                    34,661 |
| 8,000           |                 17/38 |             1 |                              3 |                    34,661 |
| 14,000          |                 17/38 |             1 |                              3 |                    34,661 |

All sixty natural-question packets pass the citation, scope, lifecycle and
budget checks. The additional fifteen keyword diagnostics pass too. At 6,000
characters the largest actual packet is 5,332 characters; larger limits produce
identical content. Two repository questions remain empty despite expected source
claims appearing in search, and the Chinese withdrawal qualification is found by
search but absent from context. Increasing the cap does not repair those
selection misses. A 533-character source statement is also shortened to 500
characters even when more packet space is available, cutting the final sentence
midword.

## Before-change local-hybrid result

| Character limit | Complete source units | Partial units | Complete prepared explanations | Total rendered characters |
| --------------- | --------------------: | ------------: | -----------------------------: | ------------------------: |
| 3,500           |                 15/38 |             1 |                              1 |                    27,800 |
| 6,000           |                 17/38 |             1 |                              3 |                    34,661 |
| 8,000           |                 17/38 |             1 |                              3 |                    34,661 |
| 14,000          |                 17/38 |             1 |                              3 |                    34,661 |

The actual cached local model finds all 38 expected units in hybrid top-50
search, compared with 36 for lexical search. Yet normal context still includes
only fifteen complete units, and all three larger budgets produce identical
content. All seventy-five packets, including keyword diagnostics, pass their
safety checks. This isolates a post-search selection bottleneck in this corpus;
replacing lexical retrieval with the real local model does not by itself repair
the exclusions.

## Selection and presentation changes

The reference-selection path is separate from the stricter personal-memory
matcher. An evidence statement can support one part of a multi-part question
without repeating the whole question. Ranking discounts words common throughout
the bounded candidate set, considers distinct query clauses and handles Chinese
bigrams alongside shared English identifiers. These are bounded local text
operations, not a new model, service or database.

Reviewed topic relationships can supply their exact, current source premises
even when a premise does not repeat the question's wording. Original topic
search ordering remains a distinct signal from the finer-grained ranking of
individual source statements. Expansion still checks the exact scope, accepted
claim and current supporting source revision; it cannot substitute a different
source into an old topic or turn generated interpretation into fact.

Accepted source statements now retain their complete wording. The fixed
500-character context cap and budget-driven prefix clipping no longer cut those
claims mid-sentence. If a whole statement cannot fit, it is omitted and the
packet reports truncation. Source evidence is selected before admitting a
summary, and topic quotas remain unchanged. Raw document excerpts and personal
memories retain their separate existing budget rules.

The source-reference Markdown view previously repeated an identical quotation
both as the statement and in its citation. It now displays that complete quote
once, retaining attribution and exact range metadata; full structured citations
remain unchanged. This removes duplicate presentation rather than shortening
evidence or omitting a qualification.

## Final comparison and recommended default

The final candidate is `target/packet-after-moon`, SHA-256
`ef0911cc290a252b298a3529b018b475315dcb009bd9dca863437741d4414dc7`. Both
retrieval modes use the same frozen fixture and return the same complete source
coverage. The actual local model is
`intfloat/multilingual-e5-small@fastembed-5.17.3:query-passage-v1:max512`; its
process runs with networking denied. Detailed reports are
`target/packet-after-lexical.json` and `target/packet-after-hybrid.json`.

| Character limit | Before complete source units | After complete source units | After partial units | Complete explanations, lexical / hybrid | After actual characters, lexical / hybrid |
| --------------- | ---------------------------: | --------------------------: | ------------------: | --------------------------------------: | ----------------------------------------: |
| 3,500           |                        15/38 |                       24/38 |                   0 |                                   0 / 0 |                           37,491 / 37,491 |
| 6,000           |                        17/38 |                       37/38 |                   0 |                                   3 / 4 |                           55,019 / 55,151 |
| 8,000           |                        17/38 |                       38/38 |                   0 |                                   4 / 4 |                           63,453 / 63,453 |
| 14,000          |                        17/38 |                       38/38 |                   0 |                                   5 / 5 |                           65,464 / 65,464 |

Counts and total characters aggregate the fifteen questions, rather than
describing one packet. All 150 final inspected packets (four budgets plus a
keyword diagnostic in each mode) pass grounding, scope, lifecycle and budget
checks. The deliberately withdrawn topic remains excluded. Both languages'
qualifications survive; no accepted source statement is partially quoted.

**Recommend 8,000 characters for the next authorised deployment trial.** It is
the smallest tested cap retaining all 38 expected source quotations. At 6,000,
the source-authority question still omits the statement explaining that capture
is inert. At 14,000 no additional expected source evidence appears; it admits
one extra complete source-authority explanation and one extra unlabelled source
reference. Reserve that larger cap for explicit expanded research. This is a
provisional default supported by this corpus, not a claim of universal
optimality.

At 8,000 the median actual packet is 4,182 characters and the maximum is 7,908.
The cap does not force padding to 8,000. Configured limits and the CLI/adapter's
3,500-character defaults have **not** been changed in this implementation turn;
the recommended deployment configuration is `maxChars: 8000`, with CLI callers
using `--max-chars 8000`. Source storage and topic compilation snapshot budgets
are separate and are not being increased.

Extra context still needs judgement. At 8,000 there are twenty source references
outside the fixture's expected labels, compared with twelve at 6,000 and
twenty-one at 14,000. Several are useful background, but spot review still finds
weak additions such as migration-backup guidance on the native-recall question
and metrics fields on the capture question. Full expected coverage does not
prove semantic precision, answer accuracy or that every selected reference is
useful. Some explanations are omitted when selected source evidence fills the
eight available slots. These limits remain visible rather than being removed
from the fixture or counted as successful human answers.

## Regression and integration validation

The final full suite passes **313 Rust tests**, including sixteen independent
selection regressions, and **119 OpenClaw adapter tests**. Clippy with warnings
denied, Rust formatting, Deno type/lint checks and diff whitespace checks pass.
The held-out cases cover long questions, request-word handling, Chinese/English
clauses, exact named subjects and comparisons, weak matches even when no
stronger candidate exists, irrelevant secondary topics, complete final
qualifications, budget omission, personal-memory separation and withdrawal of
topic premises.

Review caught and fixed two unsafe retrieval shortcuts: a collection name could
substitute for another named subject, and global clause scores could drop a
shorter language clause. A scope now establishes only its own name; another
explicit subject must match the admitting clause's evidence. Named comparison
fragments require a second topical match. General request words are removed
before stemming, so “does” cannot become a spurious “doe” name anchor. The
stricter existing personal-memory/correction matcher remains unchanged.

The code adds no runtime, dependency, model call, migration, credential or
permission change. Ranking is bounded to the existing candidate limits, a
2,048-character query, 256 normalised terms, sixteen split segments plus the
whole-query/optional language facets, two topics and thirty-two exact premise
lookups. Purely Chinese lexical candidate generation remains limited by the
existing FTS tokenisation; better post-search matching is not a general Chinese
search-quality claim. Real L2 generation and human answer-quality review remain
outside these deterministic checks.

## Runtime check and deployment gate

The warm-worker benchmark uses synthetic corpora of 1,000 and 10,000 reference
documents: 80% raw documents, 20% reviewed source claims, plus three reviewed
canary claims and one topic. Four query types cover cross-source recall,
migration, a raw reference and a correct empty result. Every response is checked
for exact citations, scope, slot/character bounds and final schema-11 health. No
embedding query or provider model is executed in this benchmark.

The initial debug baseline ran 100 measured requests per budget at each corpus
size. A paired **optimised release-build** comparison then reused copies of the
same consistent seed snapshots, with three warm-up rounds and five measured
rounds of all four query types per budget. That gives twenty measured requests
per budget/variant/corpus and 320 measured release requests in total. Process
startup, corpus intake, database copying and migration are outside the timing.
Warm stdio round-trip measurements include local IPC and JSON serialisation.
Model-heavy sweeps and compiler processes had finished before release timing.

| Character limit | Before p95, 1k | After p95, 1k | Before p95, 10k | After p95, 10k |
| --------------- | -------------: | ------------: | --------------: | -------------: |
| 3,500           |       48.10 ms |      52.80 ms |       428.50 ms |      467.77 ms |
| 6,000           |       48.42 ms |      53.43 ms |       423.76 ms |      468.10 ms |
| 8,000           |       50.89 ms |      51.44 ms |       418.38 ms |      466.73 ms |
| 14,000          |       46.11 ms |      52.14 ms |       415.21 ms |      466.69 ms |

Packet size has little effect on this workload's tail latency. The 10k scaling
problem already exists in the before version; the cross-source evidence path
adds roughly 48 ms at the recommended budget. Although some median query times
fall, the measured tail increases, so this is **not a retrieval-speed
improvement**. Twenty requests per cell and fixed-order shared-machine runs do
not establish precise small differences or production latency distributions. The
much larger gap to the existing 50 ms p95 target is clear: that deployment
performance gate is not met. Profile and optimise dependency-validity and exact
premise queries, then repeat the same corpus before deployment. A query-plan
inspection suggests repeated source-validity materialisation may contribute;
that remains a profiling hypothesis, not a confirmed sole cause.

Untimed intake of the 10k corpus also took approximately fourteen minutes in the
before build, largely while creating its 2,000 reviewed claims. That is a
separate pre-existing write-scaling concern, not timed retrieval or a
packet-size effect. It should remain visible in subsequent capacity work.

The release binaries were built from the saved before source files and the final
checkout. Their complete four-budget lexical quality results exactly reproduce
the corresponding debug results. Reports are
`target/packet-runtime-before.json`, `target/packet-runtime-release.json`,
`target/packet-before-release-quality.json` and
`target/packet-after-release-quality.json`. The source snapshots, frozen
binaries and synthetic seed databases are retained under `target/` for
follow-up.

For a complete fresh reproduction using explicit release binaries:

```sh
deno run --allow-read=target --allow-write=target \
  --allow-run="$PWD/target/packet-before-release-moon,$PWD/target/packet-after-release-moon" \
  tools/run-packet-runtime-benchmark.ts \
  "$PWD/target/packet-before-release-moon" \
  "$PWD/target/packet-after-release-moon" \
  "$PWD/target/packet-runtime-reproduced.json" 1000,10000 5
```

This command includes the untimed corpus-generation cost. The retained-seed
replay used for the reported release comparison runs the same validation and
measurement loop without repeating intake. Neither the benchmark nor the
recommended configuration authorises a live deployment.
