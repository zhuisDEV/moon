# SQLite source storage

Moon retains selected text and provenance in SQLite, then supports reviewed
extraction of verbatim source claims. Source storage and learning were added by
schemas 9 and 10. Moon 2.9.4 uses schema 21. For first installation, start with
[getting started](getting-started.md); for an existing runtime, follow
[updating](updating.md) before using these commands. Editing or building a
checkout does not update an installed runtime.

Use this guide when you want Moon to learn from an article, note or other
explicitly selected text. Normal chat learning is covered in
[the learning guide](learning.md). The sequence is **capture → extract → review
and accept → recall**. Optional [topic synthesis](topics.md) comes after source
claims have been accepted.

`source import` is different from `ingest`: it preserves immutable revisions
instead of updating an indexed document. A captured body does not enter current
search, context packets, embeddings, completed-session evidence or L1/L2.
Eligible captures create durable source jobs. Explicit preparation, submission
and operator acceptance are required before supported source-derived claims can
join recall. The CLI commands make no model calls and start no automatic worker.
The subsequent [topic workflow](topics.md) compiles accepted claims with
explicit dependencies and separate review; source extraction itself remains
verbatim.

## Capture a source

Create a fresh, isolated home for every example in this guide:

```bash
moon_source_home="$(mktemp -d /tmp/moon-source.XXXXXX)"
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" init
```

Keep this variable and use the same home when continuing into the topic guide.
These examples do not modify `~/.moon`. For real intake, choose the intended
runtime and scope deliberately rather than copying a test path into production.
The explicit database path also overrides any inherited `MOON_DATABASE`.

Source commands always return JSON, including when `--json` is omitted. Intake
accepts a versioned JSON object from stdin or a regular file:

```bash
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source import --scope project:canary <<'JSON'
{
  "schema_version": 1,
  "source_key": "canary-storage",
  "kind": "external_document",
  "format": "text",
  "origin": "https://example.com/canary",
  "title": "Canary storage note",
  "author": "Example author",
  "content": "Canary uses SQLite."
}
JSON
```

`--input /path/to/capture.json` reads the same object from a regular file;
`--input -` reads stdin. The file is an intake transport, never another
canonical knowledge store. Nothing in the body's text or its `origin` causes a
filesystem read, network request, renderer invocation or command execution.

| Input field       | Contract                                                                                                                    |
| ----------------- | --------------------------------------------------------------------------------------------------------------------------- |
| `schema_version`  | Required, exactly `1`; independent of database schema version                                                               |
| `source_key`      | Required stable identity within the explicit `--scope`; 1–256 ASCII bytes using letters, digits, `.`, `_`, `:`, `/`, or `-` |
| `kind`            | Required: `external_document`, `user_statement`, `reference`, or `generated_analysis`                                       |
| `content`         | Required UTF-8 text, at most 1 MiB before and after sanitisation                                                            |
| `format`          | `text` (default) or `markdown`; Markdown remains stored text                                                                |
| `origin`          | Optional canonical HTTP(S) URL; userinfo, query and fragment are rejected                                                   |
| `title`, `author` | Optional nonempty provenance text, each at most 4 KiB                                                                       |
| `published_at_ms` | Optional publication timestamp in Unix milliseconds; distinct from Moon's capture time                                      |

Unknown fields and unsupported schema versions are rejected. The request cannot
set scope, revision IDs, capture time, hashes, eligibility or execution options.
Scopes are nonempty, at most 256 bytes, with no surrounding whitespace or
control characters. Detected credential shapes are rejected in both scopes and
keys. The CLI bounds the actual JSON read to 8 MiB and rejects nonregular file
inputs; on Unix it also refuses a final symlink and opens nonblocking before
inspecting the file descriptor. Binary attachments, HTML/PDF extraction, URL
fetching and arbitrary metadata are outside this initial text-only interface.

The caller supplies the source kind. `user_statement` does not authenticate an
author or turn a captured assertion into an owner instruction.

## Repetition and revision changes

Repeating the current capture with the same sanitised content and material
provenance returns the existing revision. Capture time alone does not create a
new revision. Different source keys or scopes remain distinct even when their
content matches.

A content or material-provenance change requires the current revision ID:

```bash
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source get \
  --scope project:canary --key canary-storage

moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source import \
  --scope project:canary --expected-revision '<current-revision-id>' <<'JSON'
{
  "schema_version": 1,
  "source_key": "canary-storage",
  "kind": "external_document",
  "format": "text",
  "origin": "https://example.com/canary",
  "title": "Canary storage note",
  "author": "Example author",
  "content": "Canary now uses SQLite with WAL."
}
JSON
```

Replace the placeholder with the ID returned by `get` or `import`. A stale
expected revision conflicts when changing the current representation; an
identical retry is a no-op even if its expectation is stale. Inspect and
reconcile a competing update before retrying. Do not blindly substitute the
newest ID. Previous revisions and citations remain inspectable, including after
a newer capture changes the same source. Withdrawal retains that history;
explicit previewed deletion is described below. A reimport cannot silently
restore a withdrawn or deleted source.

```bash
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source list --scope project:canary
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source history \
  --scope project:canary --key canary-storage
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source get \
  --scope project:canary --key canary-storage --revision '<retained-revision-id>'
```

`list` and `history` return bounded summaries without bodies. Their cursor flags
are `--after-key` and `--before-revision`, respectively; use the returned next
cursor to continue. Page limits are 1–100 (default 20). `get`, `list`, `history`
and `cite` require an existing current-schema database and never create or
migrate it.

## Cite retained evidence

Select offsets against the body returned by `get`, after sanitisation. The first
example's complete body occupies bytes `[0, 19)`:

```bash
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source cite \
  --scope project:canary --key canary-storage \
  --revision '<first-revision-id>' --start-byte 0 --end-byte 19
```

Byte offsets are zero-based, with an inclusive start and exclusive end. Both
must be UTF-8 character boundaries. Moon derives the quote and line numbers from
the retained revision; callers do not submit a quotation for Moon to trust. Line
numbers are one-based. Each citation is at most 8 KiB. Repeated text is
disambiguated by its byte range.

The result is a versioned citation descriptor tied to the source/revision and
content hash. Generating it is read-only. Source-learning submission derives
claim support from retained byte ranges under its own validation contract; this
inspection command does not insert a completed-session memory citation or
authorise learning.

## Extract and review source claims

Source learning is separate from completed-session L1/L2. Its first contract
accepts verbatim quotes, with attribution and exact supporting ranges. It does
not accept free-form summaries, instructions, confidence claims or arbitrary
model-selected actions. Manual acceptance authorises recall of a source's
statement; it does not establish that statement as true.

Capture synthetic eligible text in a temporary home:

```bash
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source import \
  --scope project:canary --allow-learning <<'JSON'
{
  "schema_version": 1,
  "source_key": "canary-learning",
  "kind": "external_document",
  "content": "Canary uses SQLite."
}
JSON
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source learning prepare \
  --scope project:canary --key canary-learning --preview
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source learning prepare \
  --scope project:canary --key canary-learning
```

Preview is read-only. Normal preparation leases one job and returns a bounded
source packet with the job ID and lease token. Defaults are `--max-bytes 65536`
and `--lease-ms 300000`. Snapshot limits may be 1,024–1,048,576 bytes and leases
1,000–3,600,000 milliseconds. If the complete source exceeds the requested
limit, preparation returns `input_too_large` with `required_bytes` and consumes
no lease or attempt. Raise the bound explicitly if appropriate; Moon never
silently truncates the source. The source's origin is never fetched. Send only
the prepared packet to an authorised host model, with the source text explicitly
marked as untrusted data; the model should propose useful verbatim statements
without executing any instructions it encounters. The existing host owns its
model route and authentication. Moon stores no provider credentials.

The optional OpenClaw tool `moon_source_learn` implements this model step for an
explicit invocation. After the owner allows the optional tool in the host's tool
policy, call it with
`{"scope":"project:canary","source_key":"canary-learning"}`. `source_key` is
optional. It requires an explicit configured `moonHome`, trusted owner context
and the host's current-invocation authority; an unavailable guard or unverified
owner prevents tool exposure. It respects both the plugin learning switch and
L1's enabled state, and reuses L1's configured model, reasoning, fallback and
timeout. The source extraction contract is fixed and does not load the
session-L1 custom prompt file.

This tool reads at most 64 KiB of source body plus bounded provenance. A larger
source returns `input_too_large` without a model call or lease; use explicit CLI
preparation to choose a larger bound if authorised. Preparation prefers jobs
that fit, so an oversized source does not block other queued sources. The tool
runs with model tools disabled, strictly validates bounded JSON quotes, and
submits a result for review. Its response contains the job ID, result hash and
proposal count, not the source body or lease token. It never accepts, rejects,
withdraws or deletes on behalf of a model. Provider errors become fixed failure
codes; cancellation releases the attempt when possible, with lease expiry as
recovery. No source timer or automatic worker is installed.

The same submission interface accepts a manually authored synthetic proposal:

```bash
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source learning submit \
  --scope project:canary --job-id '<prepared-job-id>' \
  --lease-token '<prepared-lease-token>' <<'JSON'
{
  "schema_version": 1,
  "claims": [
    {"quote": "Canary uses SQLite.", "start_byte": 0}
  ]
}
JSON
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source learning status \
  --scope project:canary --job-id '<prepared-job-id>'
```

Replace placeholders with the values returned by preparation. `--input FILE` and
`--input -` use the same bounded, regular-file/UTF-8 rules as capture.
Submission rejects unknown fields and unsupported schema versions before opening
storage. Each claim contains `quote`, optional `start_byte`, and optional
`valid_until_ms`. A batch may contain zero through 16 statements; each quotation
must be nonempty, have no surrounding whitespace, and occupy at most 8 KiB. An
explicit start disambiguates repeated quotes. Byte ranges and citation lines are
derived and validated against the retained sanitised revision. Results that no
longer match the source revision, eligibility, lifecycle generation or active
lease cannot be applied.

Submitted proposals remain outside normal recall until review. Inspect the
result and its supporting source, including the surrounding context. Exact
quotation checks cannot establish that a selected excerpt preserves every
qualification or negation. Reject a misleading or incomplete excerpt even when
its bytes match. Accept only the exact reviewed result hash:

```bash
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source learning accept \
  --scope project:canary --job-id '<prepared-job-id>' \
  --expected-result-hash '<reviewed-result-hash>' --yes
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source claims list --scope project:canary
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source claims get \
  --scope project:canary --claim-id '<claim-id>'
```

`--yes` is a caller-side confirmation, not a field accepted from model output.
Acceptance rechecks current support and commits the reviewed claims and job
completion atomically. A changed or withdrawn source cannot become current
merely because an earlier model request completed later. Accepted claims remain
untrusted, attributed source statements when used in a context packet. Their
eligibility and expiry are checked at retrieval time. Context now returns a
complete accepted statement or omits it when it cannot fit; it never cuts that
statement to an arbitrary prefix or search chunk. This preserves the reviewed
wording, including final qualifications. Identical source quotations appear once
in rendered Markdown, with all citation metadata and complete structured JSON
support retained. If nothing fits, a truncated packet reports budget omission
rather than claiming no evidence exists. A selected statement can still have
been incomplete at review time; this rule cannot repair an incorrectly selected
original quote. `claims list
--include-inactive` exposes retained inactive
claims for inspection.

Reject a reviewed result with its unchanged hash and a concise nonsecret reason:

```bash
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source learning reject \
  --scope project:canary --job-id '<prepared-job-id>' \
  --expected-result-hash '<reviewed-result-hash>' --reason 'Not useful for recall'
```

## Recover source work

`source learning status --scope S` returns bounded job summaries; `--job-id`
inspects one job/result. It is read-only. Pending work and review state survive
process restarts. Expiring leases and bounded attempts prevent an interrupted
worker from holding a source indefinitely or retrying without limit.

Each job permits three attempts for temporary failures. Retryable failures set a
delay of 2, 4 or 8 seconds; the final failure leaves the job `dead`. The next
non-preview preparation recovers expired leases to queued work, or marks a job
dead after its final attempt. `retry` deliberately resets the attempt budget and
delay for a current queued/dead job; it cannot retry completed, cancelled,
withdrawn or otherwise unsupported work. Use it as an operator recovery action,
not an automatic loop that bypasses the retry budget.

Report a failed attempt with its lease token and a fixed error code:

```bash
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source learning fail \
  --scope project:canary --job-id '<prepared-job-id>' \
  --lease-token '<prepared-lease-token>' --error-code model_failed
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source learning retry \
  --scope project:canary --job-id '<prepared-job-id>'
```

Retryable failure codes are `model_failed`, `invalid_response`, `interrupted`
and `model_timeout`. Schema 13 also accepts `runtime_unavailable`,
`runtime_setup_failed`, `authentication_failed`, `model_unavailable` and
`model_route_mismatch`; these immediately leave the job `dead`, even on its
first attempt. Fix the cause before explicitly retrying. Status returns the
specific safe code as `last_error`; migration preserves old job fields and adds
a nullable diagnostic column. Retrying, leasing or cancelling clears obsolete
diagnostics.

The host adapter checks its locally detectable setup before acquiring a lease.
It checks returned provider/model identity before submitting a proposal, and
rejects a confirmed unexpected route or host fallback. Successful tool results
include `model_execution`, separating requested settings from observations;
missing identity, effort or fallback metadata stays unknown. Review remains
required regardless of those observations. See the
[isolated canary](source-learning-canary.md) for reproducible model validation.

Never pass provider response bodies, account information or credentials as
diagnostics. Retry is an explicit write and must still satisfy the source's
current lifecycle and eligibility checks. This milestone has no background
source-learning service or timer; queued work requires an authorised caller.

## Eligibility and security boundaries

Every new capture defaults to `reference_only`. Explicit `--allow-learning`
records `eligible` and creates required durable work transactionally. The flag
is separate from the untrusted JSON payload and must reflect the caller's
authority, never instructions found inside the source. It does not enqueue
completed-session L1/L2 work, call a model, approve a proposal, prove a claim,
or make the content a trusted instruction.

Content and retained prose provenance are sanitised before hashing or
persistence. Identity fields reject detected credential shapes instead of being
redacted into potentially colliding keys. Credential-bearing or signed URLs must
not be submitted as origins; provide a canonical URL without userinfo, query or
fragment, or omit the origin. Hashes describe retained sanitised data, not the
secret-bearing original input.

These safeguards have limits:

- Pattern-based sanitisation cannot recognise every secret. Do not import
  credentials, authentication files or unrelated private data. Capture only the
  material needed for the task.
- Scope is logical separation within Moon's APIs. It is not an ACL against an
  actor who can read the local database or invoke commands with another scope.
- Existing owner-only database permissions protect local access; this milestone
  does not add encryption at rest. Backups contain the retained source text and
  need the same protection.
- Imported prompt injections remain untrusted text. Any later compilation or
  rendering integration must preserve this boundary; sanitisation is not a
  prompt-injection detector.
- A source kind, author string or publication timestamp is a caller-provided
  attribution, not independently verified provenance.

No raw source content, arbitrary parser error bodies or submitted field values
are added to operational metrics. Source stdout is JSON-escaped so terminal
control characters are not interpreted as output formatting.

## Approved lifecycle policy

Approved on 16 November 2026; enforced here for the explicit source-learning
workflow and retained as a requirement for future automatic processing:

1. **Keep history by default.** Retain captured revisions and their citations.
   There is no automatic age-based deletion. A fact becoming stale or expired
   affects its use, not whether its supporting history is retained.
2. **Withdraw without erasing history.** Withdrawal must stop future learning
   and normal recall from that source. Invalidate its support for dependent
   claims and topics, preserving the reason and evidence. Exclude derivations
   left without valid support; revalidate those with independent valid support.
   Retrieval and job application must respect withdrawal immediately, even if
   dependent refresh work remains queued. Reimporting text must not silently
   reactivate a withdrawn source.
3. **Preview explicit deletion.** Before removal, show the selected records and
   affected citations, claims and topics. Apply only the authorised scope and
   reject a stale plan if its inputs changed. Remove or invalidate affected
   support atomically with deletion and exclude derivations left without valid
   support. Claims with independent valid support may remain after revalidation
   and citation reconciliation. Do not silently cascade into unrelated sources.
   Deletion means logical removal, not guaranteed secure erasure of database,
   journal or filesystem-snapshot bytes. Existing backup copies remain governed
   by their own explicitly chosen retention; deleting from the working database
   does not erase those copies.
4. **Recover interrupted processing.** Capture and required job creation must be
   transactional once automatic processing is enabled. Use idempotency keys,
   expiring leases and bounded retries. Check source revision and eligibility
   again before applying results, and commit results and job completion
   together. Failed or interrupted processing must preserve captured evidence
   and leave inspectable, recoverable work.

The current implementation applies this policy to source revisions, source jobs,
source-derived claims and the subsequent [topic dependency workflow](topics.md).
No policy approval by itself deletes retained data.

Inspect current lifecycle state before changing it:

```bash
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source state \
  --scope project:canary --key canary-learning
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source withdraw \
  --scope project:canary --key canary-learning \
  --expected-revision '<current-revision-id>' \
  --expected-generation '<state-generation>' --reason 'Source was retracted'
```

Replace the generation placeholder with the integer from `state`. Withdrawal
retains evidence but immediately blocks processing and normal recall support.
Reimporting a withdrawn source is not restoration. To restore deliberately, read
its new state and use `source restore` with the same flags and current
expectations. Restoration can reactivate previously accepted full quotes only
when their revision and eligibility are still current; it queues a new
processing generation and never approves new proposals automatically. `state`
also returns the latest 100 lifecycle events and `history_truncated`. Withdrawal
and restoration reasons are retained in append-only generation history, so
restoration does not erase an earlier withdrawal reason.

Explicit deletion has a separate preview/apply boundary:

```bash
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source delete-preview \
  --scope project:canary --key canary-learning
moon --home "$moon_source_home" --database "$moon_source_home/state/moon.sqlite" source delete-apply \
  --scope project:canary --key canary-learning \
  --expected-plan-hash '<reviewed-plan-hash>' --yes
```

Review affected records before applying. The plan digest includes current state
and historical lifecycle reasons. A stale plan is rejected if its dependencies
changed. Apply removes the selected source content and its affected
jobs/support, lifecycle-history reasons and transitively affected topic copies
atomically. Claims can remain only with independent valid support; a deletion
event and tombstone prevent accidental reimport under the same scoped identity.
It does not cascade into unrelated sources, erase backups, guarantee removal
from WAL/filesystem snapshots, or revoke text already supplied to an external
host/model. Retention or deletion of those copies requires a separate authorised
workflow.

## Write durability

Writable Moon connections use SQLite WAL with `synchronous=FULL`, applied on
every open. Backup destination connections also set `FULL`. This requests WAL
synchronisation at commit before success is returned. The regression test checks
the actual Moon writer connection, including a second writer and reopening;
checking a new SQLite inspection connection would not verify the writer's
connection-local setting. Tests exercise configuration and database behaviour,
not a physical power-cut scenario. Backups remain necessary.

## Migration and validation

The numbered source migration is transactional and retains the existing
documents, session evidence, memories, citations and learning state. Rehearse it
with a disposable database; a source-storage test should cover:

1. Import, identical replay, a revision change and inspection of the old body.
2. Exact citations before and after sanitisation, including Unicode boundaries.
3. Scope separation, stale/concurrent updates and rollback after failure.
4. Secret scrubbing across body/provenance, malformed and oversized input,
   special-file rejection, and untrusted instructions remaining inert.
5. Source jobs, expired leases, failed attempts, explicit review, late result
   rejection, withdrawal/restoration, and stale deletion plans.
6. Existing L1/L2 and retrieval regressions, database integrity, and restoration
   from a consistent SQLite backup.

Before an authorised production migration, retain a consistent online database
backup, configuration backup and matching prior binary. An old binary must use
its matching backup, not open the upgraded schema. The full SQLite knowledge
design includes [topic compilation](topics.md) and transitive cross-source
dependency invalidation as a separate workflow; those capabilities are not
implied by quote extraction or manual source-claim acceptance.
