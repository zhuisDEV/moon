# Topic synthesis baseline

This offline baseline was established on 16 November 2026 before implementing
topic synthesis. It compares the existing schema-10 source-claim/context
workflow with editorial reference explanations assembled from the same evidence.
The fixture and explanations were prepared by an agent and reviewed
independently by the parent engineering and security-review agents. They are not
human usefulness ratings or answers sampled from Moon's runtime model route. The
fixture's “no model generation” label refers to the absence of runtime inference
calls in the evaluation; the reference prose was drafted by an agent.

The existing [memory quality protocol](memory-improvement-plan.md) remains the
production gate. This exercise does not satisfy its 3–7 day observation window,
50-request minimum or human review requirements. It does not authorise
deployment or a production retrieval-policy change.

## Frozen inputs and reproduction

- Corpus:
  [tests/fixtures/topic_baseline.json](../tests/fixtures/topic_baseline.json).
- Runner: [tools/run-topic-baseline.ts](../tools/run-topic-baseline.ts).
- Frozen binary: `target/topic-baseline-moon`, copied from the tested release
  before topic work; SHA-256
  `3f2ef1225d6d09ae3c4c186170e94d077571facbe00aac528b3d91e943990108`.
- Detailed output: `target/topic-baseline-report.json`, including actual
  packets, source-citation checks, ranks and labelled reference explanations. It
  contains only the selected repository text and synthetic material. It is a
  review artefact, not Moon's content-free operational metrics.

The runner rejects a different binary hash. The frozen binary is a local build
artefact; preserve it for comparisons. The corpus freezes each selected quote,
its original byte range, source path and complete document hash. Subsequent
documentation drift is reported without replacing the frozen evidence. At the
baseline run all six source-document hashes and original ranges matched.

From the repository root, run:

```sh
deno check tools/run-topic-baseline.ts
deno lint tools/run-topic-baseline.ts
deno run \
  --allow-read=tests/fixtures,docs,target/topic-baseline-moon \
  --allow-write=target \
  --allow-run="$PWD/target/topic-baseline-moon" \
  tools/run-topic-baseline.ts
```

The binary and output path can be supplied as two positional arguments; the
output must remain inside this checkout's `target/`. Each run creates and
removes its own temporary home and database there. Every Moon command explicitly
supplies both paths, clears inherited environment variables, and sets `HOME` to
that temporary home. There are no network permissions, model calls, downloads,
credentials, external packages or live runtime accesses.

## Corpus and review method

The corpus has 19 sources and 43 retained claim revisions: six selected
repository document excerpt sets and thirteen explicitly synthetic challenge
sources, including one corrected revision. Repository excerpts come from source
storage, architecture, learning, native recall, migration and the quality
protocol. The runner verifies the selected text survives sanitisation unchanged,
imports it, submits the exact manually selected quotations, and accepts the
exact review hash in the isolated fixture database. This measures the current
reviewed source-claim path, rather than an invented raw-document-only baseline.

Eight real repository questions require evidence across documents. Seven
labelled challenge questions exercise conditional qualifications, genuine
disagreement, correction, fixed historical expiry, exact scopes, Chinese/English
negation and source fallback after withdrawal. Challenge assertions are not
product facts or real user preferences. An expiry timestamp fixed in the past
avoids timing-based flakiness; elapsed-time transitions are covered by lifecycle
tests separately.

Each reference explanation lists its supporting claim IDs. A complete evidence
unit means the **entire selected quotation** appears in a returned reference;
partial wording is counted separately. This deliberately strict, mechanical
measure can undercount a packet that a human would consider sufficient. It does
not judge whether prose is semantically entailed, whether the final model uses
the evidence correctly, or whether unrelated-looking references are harmful.
Unlabelled returned references are recorded rather than called false positives.

All packets use lexical retrieval, limit 8 and the normal 3,500-character
budget. The runner also records the same natural-language question at 14,000
characters, a keyword diagnostic at 3,500 characters, and lexical top-50 search.
These are diagnostics, not improvements to the baseline. They distinguish absent
search matches, relevance/selection filtering, quotation truncation and packet
pressure. Hash embeddings are not used as a substitute for semantic-quality
measurement.

The runner verifies exact UTF-8 source bytes, line numbers, displayed quotation
alignment, scope, packet bounds and exclusion of named
obsolete/expired/withdrawn claims. Health must remain good at schema 10. Any
such violation fails the run; coverage gaps remain measured results rather than
being weakened into passes.

## Measured result

| Question                                  | Expected units | Complete normal | Partial normal | Complete at 14k | Found in top 50 |
| ----------------------------------------- | -------------: | --------------: | -------------: | --------------: | --------------: |
| Capture to recall and acceptance meaning  |              4 |               1 |              1 |               1 |               4 |
| Article authority and explicit learning   |              4 |               1 |              0 |               1 |               4 |
| Withdrawal, restoration and history       |              3 |               2 |              0 |               3 |               3 |
| Deletion, backups and independent support |              3 |               1 |              0 |               1 |               2 |
| Correction, uncertainty and expiry        |              4 |               0 |              0 |               0 |               4 |
| Native recall delivery                    |              3 |               1 |              0 |               1 |               3 |
| Meaning of quality metrics                |              4 |               0 |              0 |               0 |               4 |
| Migration and durability                  |              3 |               1 |              0 |               1 |               2 |
| Compatible conditional proposals          |              2 |               2 |              0 |               2 |               2 |
| Genuine same-condition disagreement       |              2 |               2 |              0 |               2 |               2 |
| Corrected source support                  |              2 |               2 |              0 |               2 |               2 |
| Expired observation                       |              1 |               1 |              0 |               1 |               1 |
| Empty unrelated scope                     |              0 |               0 |              0 |               0 |               0 |
| Chinese/English negation                  |              2 |               1 |              0 |               1 |               2 |
| Current source fallback after withdrawal  |              1 |               1 |              0 |               1 |               1 |
| **Total**                                 |         **38** |          **16** |          **1** |          **17** |          **36** |

The eight repository questions contain 28 required evidence units: 26 appear in
top-50 search, seven appear completely in normal packets, and one appears
partially. None of these eight normal packets contains every selected unit. The
seven challenges contain ten units, nine of which appear completely; six
challenge packets satisfy every labelled unit, including the correct empty case.
Normal packets contain 18 exact original-source citations. All 45 inspected
packets pass grounding, scope, lifecycle-exclusion and budget checks. Database
health passes. These are deterministic fixture results, not semantic recall
rates.

The larger budget recovers only one complete unit. Two natural repository
questions return no references despite four expected claims appearing in search.
This is evidence of a context relevance/selection problem, not proof that packet
capacity is the main constraint or that persistent topic synthesis is required.
The multilingual case returns the English qualification but misses the Chinese
negation in context despite both being found in search. Retain that miss as a
regression case rather than silently removing it.

## Review decisions and interpretation limits

The parent engineering review checked the eight repository reference
explanations against their cited excerpts. It caught an initially incorrect
disagreement label: “explicit while contradictions remain” and “automatic only
after review resolves them” are compatible conditions. The fixture now preserves
that apparent-conflict case and adds a genuinely incompatible pair under the
same reviewed-input condition. Neither pair is approved product policy.

The independent security review verified all 29 repository quote byte ranges and
document hashes, the corrected conditional interpretation, and the separate
same-condition contradiction. It found no remaining reference-answer concern.

The prepared explanations make the intended cross-source joins inspectable. They
do not prove that a model can generate those explanations faithfully, that
stored topics save research time, or that users prefer their output. Repeated
research effort, real-model usefulness, local semantic/hybrid retrieval,
representative latency, production model routes and live observation remain
unmeasured. Corpus size is intentionally too small for a retrieval-performance
claim.

Schema 10 has no topics. The stale-topic case therefore verifies only that the
withdrawn source claim disappears and current independent source evidence
remains available. It cannot test topic invalidation. The next milestone must
separately verify all consumed premises, exact selected source attribution,
transitive freshness and deletion, identity-level cycle rejection, stale-topic
exclusion, source fallback, lease fencing, reviewed acceptance and atomic
rollback. A shared claim surviving through source B must not silently keep a
topic that specifically consumed withdrawn source A current. Deletion must
account for copied content in topic revisions, jobs and snapshots, not merely
remove an index row.

After implementation, rerun the unchanged questions and evidence against
labelled, reviewed topic interpretations. Keep the original baseline frozen,
report any topic contribution separately from exact source units, and use
neutral topic titles rather than copying full questions into titles. A
comparison can establish deterministic availability and provenance behaviour; it
still cannot establish a real-model quality or performance improvement.

## Candidate comparison method

The schema-11 comparison uses the same frozen corpus, questions, source claims,
scopes, lexical mode and budgets. Thirteen reference explanations are explicitly
submitted and reviewed as interpretations; the genuinely conflicting pair uses a
disagreement block. Neutral titles come from scenario identifiers, not the full
questions. An additional cached topic is compiled before its source is
withdrawn. The empty-scope case has no topic. The original fallback explanation
describes schema 10, so it is not stored as a current candidate assertion.

Run against an explicitly selected schema-11 binary:

```sh
deno run \
  --allow-read=tests/fixtures,docs,target/topic-candidate-moon \
  --allow-write=target \
  --allow-run="$PWD/target/topic-candidate-moon" \
  tools/run-topic-baseline.ts --candidate \
  "$PWD/target/topic-candidate-moon" "$PWD/target/topic-candidate-report.json"
```

Candidate mode refuses to overwrite the frozen baseline report. The report
records the binary hash, topic IDs/revisions, source provenance drift and actual
typed packets. Documentation changes during implementation do not change the
frozen source excerpts. The runner separately verifies that the withdrawn topic
is stale and absent from indexed recall, then checks current-source fallback.

Exact original-source units and complete prepared explanations are counted
separately. A paraphrase is never counted as an exact quotation. The comparison
does not rate model reasoning, infer research time saved or establish latency
gains. Dependency, concurrency, recovery, deletion and security behaviour are
validated separately in `tests/topic_synthesis.rs` and `tests/cli_topics.rs`.

The first candidate retained full repeated premise quotations in rendered topic
packets. It preserved baseline source coverage (16/38 complete units normally,
17/38 at 14,000 characters) but delivered no topic at the normal 3,500-character
budget; three prepared explanations fit the larger diagnostic. That finding
prompted a focused presentation change under the same quota: retain full typed
original-source citations in JSON/inspection, render complete statements with
compact source pointers, and avoid repeating every premise quotation in
Markdown. The final comparison below measures that change; the initial run does
not establish a normal-packet improvement.

## Final candidate result and remaining gap

The final candidate binary has SHA-256
`c451c3adcfc0ef1ea94b8ec9ea1c7b089e454bbafdbadf79f1c1b96711c93d87`. The fixture
identity, all fifteen questions and all scopes match the frozen baseline. A
repeat run of the frozen binary reproduced every original aggregate count.
Detailed evidence is in `target/topic-candidate-report.json`; the earlier
candidate is retained in `target/topic-candidate-initial-report.json`.

| Measure                                                     | Frozen source-claim baseline | Final candidate |
| ----------------------------------------------------------- | ---------------------------: | --------------: |
| Complete source units, normal 3,500-character packet        |                        16/38 |           15/38 |
| Partial source units, normal packet                         |                            1 |               2 |
| Complete source units, 14,000-character diagnostic          |                        17/38 |           17/38 |
| Complete prepared explanations, normal packet               |                            0 |               1 |
| Complete prepared explanations, 14,000-character diagnostic |                            0 |               3 |
| Source-reference citations, normal packets                  |                           18 |              18 |
| Additional typed topic citations, normal packets            |                            0 |               3 |
| Grounding, scope, lifecycle and budget violations           |                            0 |               0 |

The normal packet now includes the complete withdrawal/restoration explanation.
This is a delivery trade-off: its separate original withdrawal-history reference
shrinks from 459 to 120 characters, changing one complete source unit into a
partial unit. The full 459-character original citation remains in the topic's
typed JSON/inspection, and the complete interpretation retains the restoration
qualifications. The Markdown packet contains compact original-source pointers;
it does not repeat the complete premise quotation. Do not describe this as an
unqualified coverage or usefulness improvement.

At 14,000 characters the complete capture-to-recall, withdrawal/restoration and
migration explanations appear. Across all 45 candidate packets, 61 ordinary
source citations and 27 topic citations were checked against original retained
UTF-8 bytes and line ranges; all passed. Nine topic references appear across
those normal, larger-budget and keyword diagnostics. The deliberately withdrawn
cached topic reports `missing_support` and returns zero indexed topic search
results; current independent source fallback remains available. Database health
passes.

The final Rust suite contains 297 passing tests; 42 new topic integration/CLI
tests cover lifecycle, dependencies, concurrency, recovery, migration and
security. The OpenClaw adapter suite has 119 passing tests. The Deno runner
passes type and lint checks. These results establish implementation behaviour,
not model quality.

The topic quota and selection remain unchanged beyond the compact rendering in
this milestone. Usefulness remains unproven. The next quality work should
address the measured selection and source-coverage gaps and obtain
human-reviewed answer quality evidence before claiming a net benefit. The
existing normal-packet misses, including the multilingual negation, remain
visible in the corpus.
