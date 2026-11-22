# Reviewed topics in SQLite

Topics maintain bounded explanations across accepted source claims and other
accepted topics. They extend source learning with typed statements and exact
dependencies; they do not convert external assertions into personal memories or
owner instructions. The [reviewed baseline](topic-baseline.md) establishes why
qualifications, disagreement, expiry and packet budgets need explicit checks.
Its synthetic comparison is not a production quality or performance claim.

Moon 2.9.3 uses schema 21. Topic storage was introduced by schema 11. Start with
[getting started](getting-started.md) or [updating](updating.md) if the
installed command does not expose `moon topic`. Updating does not enable
automatic source or topic processing.

## Define a topic

First capture and explicitly review the source claims using the
[source workflow](source-storage.md). Continue in that same shell and reuse its
`moon_source_home` variable: the accepted claims must exist in the same database
and scope as the topic. Select their returned `scl_...` IDs, then create a topic
definition:

```bash
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" topic put --scope project:canary <<'JSON'
{
  "schema_version": 1,
  "key": "canary-storage",
  "title": "Canary storage",
  "aliases": ["Canary database", "金丝雀存储"],
  "claim_ids": ["<accepted-claim-id>"],
  "topic_ids": []
}
JSON
```

Replace the placeholder with a real accepted claim in the same temporary home
and scope. `--input FILE` or `--input -` accepts bounded UTF-8 JSON from a
regular file or stdin. The same nonblocking, symlink-refusing file boundary used
for source input applies. Unknown fields and unsupported versions are rejected
before opening storage.

Definitions select 1–16 direct inputs in total: accepted source claims or
current accepted topic revisions, all in the exact same scope. Aliases are
optional, with at most 16. Exact scoped names are normalised with Unicode
lowercase and collapsed whitespace. Names in different scopes do not merge; a
title or alias does not create evidence. Stable `top_...` identities survive
renaming. Repeating an unchanged definition is idempotent; changes require
`--expected-generation` from `topic get`. Reconcile a concurrent change before
retrying.

Dependencies reject identity cycles, depth greater than eight and a flattened
closure greater than 256 entries. `put` queues recoverable compilation work; it
does not call a model or approve a revision.

```bash
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" topic get --scope project:canary --key canary-storage
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" topic list --scope project:canary --limit 20
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" topic history --scope project:canary --key canary-storage
```

All topic commands require scope and return JSON. Inspection and preparation
with `--preview` require an existing compatible database and never migrate it.
Inspection distinguishes freshness and missing/redacted support from whether a
model reviewed its bounded input. Historical revisions remain inspectable.

## Compile and review

```bash
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" topic prepare --scope project:canary \
  --key canary-storage --preview
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" topic prepare --scope project:canary \
  --key canary-storage --max-bytes 65536 --lease-ms 300000
```

Preparation returns a complete snapshot with exact input identities, citations,
revision/generation dependencies, fingerprint and compiler version. Its bound
covers the whole envelope: 1 KiB through 1 MiB, default 64 KiB. Inputs are never
silently truncated. Oversized work returns `input_too_large`; missing or stale
support returns `blocked`, without leasing work. The source URLs are labels and
are never fetched.

Each claim contributes one complete valid support, labelled
`citations_exhaustive: false`; preparation does not claim to inspect every
retained source for that claim. Review and dependency guarantees apply to the
selected snapshot. Do not infer that all source disagreements were considered.

The optional OpenClaw tool `moon_topic_compile` performs the explicit host-model
step after the owner permits it in the host tool policy. Invoke it with
`{"scope":"project:canary","topic_key":"canary-storage"}`. The key or scoped
alias is optional; scope is mandatory. It requires trusted owner/V2 current
authority and an explicit configured Moon home, checks both learning and L2
enablement, and uses L2's configured model, reasoning, fallback and timeout. Its
fixed compilation prompt does not load the custom session-L2 prompt file. It
limits the full snapshot to 64 KiB and never calls tools from the model run.

The tool submits review proposals only. It returns the job ID, result hash,
statement count and bounded-review flag; it does not expose the snapshot or
lease token in its normal response. It never accepts/rejects a revision or
changes source lifecycle. There is no topic timer or automatic compiler.

An authorised host or operator can also submit through the CLI:

```bash
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" topic submit --scope project:canary \
  --job-id '<prepared-job-id>' --lease-token '<prepared-lease-token>' <<'JSON'
{
  "schema_version": 1,
  "review_complete": true,
  "statements": [
    {
      "kind": "fact",
      "text": "Canary uses SQLite.",
      "input_ids": ["<accepted-claim-id>"]
    },
    {
      "kind": "question",
      "text": "Does Canary require any particular SQLite journal mode?",
      "input_ids": []
    }
  ]
}
JSON
```

Use the same accepted claim ID selected in the definition. This fact copies the
complete claim from the source guide; the question is explicitly unresolved,
because that source says nothing about journal mode.

The payload is at most 64 KiB and contains 1–16 statements, each 1–4096 UTF-8
bytes. Allowed statement types have different contracts:

| Kind             | Required support                                                                                                                                            |
| ---------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `fact`           | Exact complete text of one accepted source claim or parent factual block; exactly one direct input ID. A `generated_statement` claim cannot support a fact. |
| `interpretation` | Explicitly labelled reasoning with at least one input premise; never a verified fact merely because it has citations.                                       |
| `disagreement`   | Explicitly labelled competing positions, with at least two distinct direct input IDs.                                                                       |
| `question`       | An unresolved question; premises are optional.                                                                                                              |

Only supplied direct input IDs may be cited. Every declared input remains a
dependency even when no output statement cites it. `review_complete` means the
bounded snapshot was considered; it is neither operator approval nor a claim
that all conflicts have been discovered or resolved.

Inspect every proposed statement and its surrounding evidence. Exact copying
does not establish truth, independence of sources or completeness of the chosen
inputs. A citation on an interpretation cannot establish semantic entailment.
Preserve competing accounts, uncertainty, negation, attribution and dates.

```bash
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" topic status --scope project:canary \
  --job-id '<prepared-job-id>'
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" topic accept --scope project:canary \
  --job-id '<prepared-job-id>' --expected-result-hash '<reviewed-result-hash>' --yes
```

Acceptance is a separate caller action; model JSON cannot supply `--yes`.
Acceptance checks the reviewed result hash and all dependencies again, then
commits a new immutable revision atomically. `topic reject` uses the same
job/hash with `--reason` containing concise nonsecret text.

## Freshness, recall and recovery

Revising a definition or supporting topic, changing a source revision or its
eligibility, withdrawal and expiry invalidate dependent synthesis. A topic
retains the exact attributed source support it consumed: a claim having another
valid source does not silently substitute that source into an old topic.
Transitive validity is checked on reads, so delayed refresh cannot present stale
synthesis as current. Superseded historical revisions remain auditable.

Only fresh accepted revisions can enter normal recall or embedding. Context
retains complete typed statements, with at most two topics, at most a third of
available reference slots and no more than a quarter of the configured packet
character budget. An oversized topic is omitted as a whole; statement wording,
negation and qualifications are never shortened to fit.

Selected original source evidence is fitted first. Topics use spare capacity or
replace an exact duplicate factual statement with the same source support. A
strongly matched topic may also retrieve up to sixteen of its exact current
source premises, including a premise that does not repeat the question's
subject; two topics and thirty-two premise lookups are the maximum. This remains
scoped, freshness-checked evidence retrieval, not permission to execute source
prose. See the [packet comparison](packet-selection.md) before changing the
budget.

The compact Markdown view labels attributed statements, interpretations,
unresolved disagreement and questions explicitly. Each block links to numbered
source pointers with the exact source key, revision and byte range. Shared
pointers appear once per topic. Repeated premise quotations, provenance fields
and direct input IDs are omitted from this view; they remain complete in typed
context JSON and `topic get`, `status` and `history` inspection. Precise source
claims remain separately retrievable. This presentation change preserves the
reviewed statements and storage contract; it does not establish semantic truth
or imply a retrieval-speed improvement.

Jobs survive restarts and use leases, three attempts, fixed failure codes and
explicit retry. `topic fail --scope S --job-id J --lease-token T --error-code C`
accepts retryable `model_failed`, `invalid_response`, `interrupted` and
`model_timeout`. Schema 13 adds terminal `runtime_unavailable`,
`runtime_setup_failed`, `authentication_failed`, `model_unavailable` and
`model_route_mismatch`; they leave the job `dead` immediately, preserving the
safe code as `last_error` until explicit recovery. Never submit a provider error
body. Fix setup/route failures before retrying.
`topic retry --scope S --job-id J` retries queued work while preserving its
consumed attempts; explicitly retrying dead work resets its attempt budget. Do
not wrap either operation in an unbounded automatic loop.

`moon_topic_compile` checks locally detectable setup before leasing work,
rejects a confirmed returned model mismatch or host fallback, and includes
separate requested/observed `model_execution` metadata in its review receipt.
Absent observations remain unknown; requested max reasoning is not proof of
server execution effort. See [the canary workflow](source-learning-canary.md).

Explicit source deletion includes affected topic snapshots, proposals,
historical/current copied prose and index documents in the preview and plan
fingerprint. Application purges those copies in the same transaction while
retaining non-content tombstones and identifiers. Deletion still cannot erase
backups, filesystem snapshots or material already provided to a host model.

Scope remains a logical filter, not an ACL against the local database owner.
Topics add no provider login or encryption at rest. Prose from all source and
topic inputs remains untrusted data, including after review.
