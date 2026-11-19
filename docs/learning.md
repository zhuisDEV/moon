# Session learning and reviewed source claims

L1 extracts small, cited memories after a completed OpenClaw turn. L2 revisits
unprocessed evidence in daily batches, compares it with related memories and
their original evidence, and proposes confirmations, corrections, exact-content
merges, or review items. L2 is disabled by default.

For first installation, use [getting started](getting-started.md). For an
existing runtime, use [updating](updating.md), including the skill refresh step.
This guide explains configuration and maintenance after installation. It covers
three distinct paths:

| Path                                  | Trigger                         | Review boundary                                                   |
| ------------------------------------- | ------------------------------- | ----------------------------------------------------------------- |
| Session L1                            | An eligible completed chat turn | Conservative automatic validation                                 |
| Session L2                            | Daily, only after you enable it | Supported actions can commit; uncertain cases remain review items |
| Source extraction and topic synthesis | Explicit tool or CLI invocation | A separate reviewer must accept the exact proposal hash           |

The session-learning commands require Moon 2.6.1 or newer. Upgrading does not
automatically enable daily synthesis. For a source checkout, run
`cargo build --locked` and use `./target/debug/moon` in place of `moon` in the
isolated examples below.

The source-learning workflow is separate: eligible immutable source captures
create durable jobs, and explicitly reviewed verbatim claims can later join
recall. It is accessed through `source learning` and `source claims` and does
not run on the L1/L2 scheduler. See the source workflow below and
[source storage](source-storage.md) for its schema-10 contract.

## Configure the two stages independently

Moon reads `<selected-home>/moon.toml`; the normal runtime path is
`~/.moon/moon.toml`. The file sits outside versioned release directories and
survives a runtime update. The adapter reloads it for each newly recorded turn
and scheduler tick. A running attempt keeps the settings it started with.

Create and inspect a starter in an isolated home:

```bash
learning_home="$(mktemp -d /tmp/moon-learning.XXXXXX)"
moon --home "$learning_home" --json config init
moon --home "$learning_home" --json config show
moon --home "$learning_home" --json config validate
```

`config init` creates an owner-only file and refuses to overwrite an existing
path. None of these commands creates or migrates a database. `show` returns
`path`, `present`, and the parsed `learning` settings, with absolute prompt
paths. Optional unset fields appear as JSON `null`; they are not TOML null
values. `validate` also checks that the IANA timezone exists and that custom
prompt files are readable, regular UTF-8 files of at most 64 KiB. It makes no
model request and cannot prove account access or model quality.

The starter selects Astra with different effort for each stage:

```toml
[learning]
observation_ttl_hours = 24

[learning.l1]
enabled = true
model = "openai/gpt-6-astra"
reasoning = "low"
timeout_ms = 120000
max_output_tokens = 8192
# fallback_model = "provider/model"
# fallback_reasoning = "low"
# fallback_enabled = false
# prompt_file = "prompts/l1-distill.md"

[learning.l2]
enabled = false
model = "openai/gpt-6-astra"
reasoning = "xhigh"
timeout_ms = 600000
max_output_tokens = 32768
daily_at = "03:00"
timezone = "Australia/Sydney"
batch_size = 32
max_batches_per_day = 8
max_actions = 16
max_input_chars = 64000
# fallback_model = "provider/model"
# fallback_reasoning = "high"
# fallback_enabled = false
# prompt_file = "prompts/l2-synth.md"
```

Both stages accept independent `model`, `reasoning`, `fallback_model`,
`fallback_reasoning`, `fallback_enabled`, `timeout_ms`, `max_output_tokens`, and
`prompt_file` settings. Models use OpenClaw's `provider/model` form. Unset model
and reasoning settings inherit the existing plugin/OpenClaw route; an unset
fallback inherits its first configured fallback. Set `fallback_enabled=false` to
disable fallback for that stage. A fallback identical to the primary is not
called twice.

With no `moon.toml`, L1 stays enabled with the existing model routing and L2
stays disabled. Valid inherited Astra effort is preserved; if an inherited
effort such as `off` is unsupported by Astra, the adapter uses `low` for L1 and
`xhigh` for L2. Explicit Astra settings accept `low`, `medium`, `high`, `xhigh`,
`max`, or `ultra` in the native Codex route; `off`, `minimal`, and `adaptive`
are rejected. Other providers still need to support the selected effort
themselves. Use `low`, not `light`, for the lighter Astra effort.

Setting only `learning.l1.enabled=false` preserves evidence recording and allows
L2 to process it later. OpenClaw's plugin-level `learningEnabled=false` disables
both evidence recording and L2. Compaction continues to use its separate
`compactionModel` and `compactionReasoning` settings.

The TOML schema rejects unknown fields, wrong types, and out-of-range values. It
accepts no credentials or arbitrary provider options. The entire file is limited
to 64 KiB.

| Setting                   | Default when unset          | Allowed range            |
| ------------------------- | --------------------------- | ------------------------ |
| `observation_ttl_hours`   | 24                          | 1–8,760 hours            |
| L1 `timeout_ms`           | inherited, normally 120,000 | 1,000–1,800,000          |
| L2 `timeout_ms`           | 900,000                     | 1,000–1,800,000          |
| L1/L2 `max_output_tokens` | 4,096 / 16,384              | 128–131,072              |
| L2 `batch_size`           | 32                          | 1–128 evidence records   |
| L2 `max_batches_per_day`  | 8                           | 1–64                     |
| L2 `max_actions`          | 16                          | 1–32                     |
| L2 `max_input_chars`      | 64,000                      | 1,024–2,097,152          |
| L2 `daily_at`             | `03:00`                     | `00:00`–`23:59`          |
| L2 `timezone`             | `Australia/Sydney`          | recognised IANA timezone |

`max_input_chars` bounds the prepared evidence/memory JSON packet, not the
complete model prompt; fixed instructions and optional prompt files add to it.
Moon enforces input, timeout, action, batch, and attempt limits. In the native
Codex plugin inspected with OpenClaw 2026.9.4, `max_output_tokens` is only a
request and is ignored by that backend. It is not an enforced output-token or
subscription-usage cap. A timeout does not establish an exact token budget.

## Prompts and native Codex authentication

Built-in prompts give each stage its task, evidence rules, output schema, and
conservative selection criteria. A `prompt_file` adds owner guidance; it does
not replace the fixed instructions or validation. Relative paths resolve against
the directory containing `moon.toml`. Prompt contents are not printed by
`config show`, and invalid prompt errors do not echo their contents.

Useful additional guidance can name priorities, for example “Prefer explicit
user corrections and keep changing service status temporary.” It should not
instruct the model to weaken citation, freshness, or review requirements.
Conversation text, retrieved memories, and quoted instructions remain untrusted
evidence. Exact quotes and deterministic checks reduce some errors; they do not
prove semantic truth or resolve every ambiguous contradiction.

Moon delegates model calls to OpenClaw. In the verified native Codex setup,
OpenClaw's Codex plugin owns the `openai` provider and uses the app's Codex
binary with `homeScope: "user"` and the existing user `CODEX_HOME`
authentication. Moon does not need an API key, create another login, read the
OAuth file, or copy credentials. Selecting another provider uses that provider's
OpenClaw route. Do not assume a different OpenClaw installation maps providers
identically.

Each model attempt has a fresh owner-preserving
`agent:<owner>:internal-session-effects:incognito-<id>` key and detached
OpenClaw persistence. Native Codex recognises that key as ephemeral. Helpers do
not reuse the conversation transcript, and their tool route is disabled.
Conflicting or ambiguous agent ownership fails instead of selecting a different
agent silently. Primary and fallback attempts use separate identities.

In OpenClaw 2026.9.7, a non-empty `streamParams` object counts as a transport
override and makes native Codex ineligible. Sending the advisory
`max_output_tokens` hint this way can redirect a working native helper onto a
host route with no compatible authentication. Moon consults the host's runtime
policy when available and omits that hint for explicit native runtimes or
implicit OpenAI routing. Explicit stock OpenClaw and ordinary non-OpenAI routes
retain it. The selected model, reasoning, timeout, tool restrictions, and
incognito identity remain unchanged; native output tokens are not capped by this
setting. A working chat or direct Codex probe does not validate a fresh OpenClaw
learning-helper route.

OpenClaw 2026.9.4 retains completed native incognito subscriptions until its
Codex client shuts down. Moon does not close that shared client after each
helper because it can also own unrelated user threads. Ephemeral persistence
does not mean immediate removal from gateway memory; long-running native memory
usage needs separate measurement.

See [the canary guide](openclaw-canary.md#native-codex-route-check) for the
account/model discovery tool and the separate, opt-in two-turn native smoke
test. A successful constant-response probe verifies routing and effort support;
it does not measure memory accuracy or representative synthesis performance.

## Evidence, corrections, and freshness

For verified chat audiences, Moon commits the accepted human message as
immutable private evidence before L1 starts. Assistant replies and tool results
are excluded. See [public and private memory](memory-privacy.md). OpenClaw
retries storage failures. L1 model failures do not erase evidence; an enabled L2
can later revisit it. The default L1 limit is three proposals per eligible turn,
adjustable through the existing `learningMaxMemories` plugin setting.

L1 compares correction candidates with related active memories. L2 receives
selected unprocessed evidence, related claims, and the original evidence behind
those claims. Each L2 action must cite at least one selected record, use exact
quotes from its prepared snapshot, and stay within one scope. Claims must be
supported by the newest cited evidence; older citations cannot justify a value
contradicted by the newer source. Questions, uncertainty, hypothetical wording,
and negation must not become affirmative user facts. The assistant recalling a
memory is not independent confirmation of that memory.

Explicit corrections may supersede the supplied active head while keeping its
canonical key. Existing kind is preserved when reconciling legacy memories.
Confirmation and merge preserve exact existing content; automatic merge is
limited to identical content in the same scope and kind. The retained head keeps
citations, confirmation dates, expiry, aliases, and unresolved review
information. Different wording alone is not sufficient for an automatic merge.
Uncertain conflicts produce review items instead of rewriting a claim.

Temporary operational results receive `valid_until_ms` based on the newest
supporting evidence time plus `observation_ttl_hours`, not the date a historical
batch happens to run. New temporary claims use `observation`; legacy claims can
receive expiry while retaining their kind. `observed_at_ms` and
`last_confirmed_at_ms` distinguish source observation from later confirmation.
Expired memories leave normal recall. Within the same learning scope, fresh
evidence can still confirm or correct an expired head under its original key.
The learner checks its original citations before reconciling it. This does not
retroactively classify every old memory as temporary.

L2 scores up to 512 recent candidate memories and selects up to 128 matches. A
prepared packet sets `context_limited` when any of those bounds or the character
budget reduces coverage. `omitted_memory_count` counts scored matches left out
of the packet. `candidate_scan_limited` means more candidates remained unscored.
The model is told when comparison is incomplete. If the oldest pending evidence
is too large, or no related claim and its original evidence can fit, preparation
fails and preserves the backlog. Inspect the packet before increasing its
budget; do not treat a partial packet as proof that no conflict exists.
`learning status` exposes review reasons and evidence session IDs for human
inspection; it is not an automatic review-resolution command.

## Daily schedule and recovery

The existing OpenClaw plugin service checks on startup and every 60 seconds. No
separate daemon or cron installation is needed. After the configured local time,
it works on evidence at or before that occurrence's fixed cutoff. If the gateway
starts late, it catches up the most recent due occurrence and can include older
unprocessed evidence. It does not replay a separate job for every missed
calendar day.

The date key uses the configured IANA zone. A skipped daylight-saving wall time
shifts forward by the gap; a repeated wall time shares one date key. Durable
keys such as `l2:2026-09-13:0` prevent a committed batch from running again
after a restart. Evidence newer than that day's cutoff waits for a later
occurrence.

With schema 14, each batch gives fresh evidence priority and respects the record
and packet budgets. The scheduler commits at most `max_batches_per_day` batches.
A database-wide lease prevents overlapping prepared runs. Expired leases become
failed attempts, and three failed attempts for a daily batch key exhaust that
key and stop work for that occurrence. Each attempt can call the configured
primary and, if enabled, one fallback. Failed input remains available for the
next due date; exhaustion does not delete or mark it processed.

Moon checks each proposed memory against its cited evidence. A batch can save
supported claims and defer the sources of rejected claims. If every candidate
fails grounding, Moon records the attributable deferrals. A transport error,
malformed proposal, conflicting targets, or a citation that cannot be traced to
selected evidence fails the batch and preserves each session’s retry allowance.

SQLite validates the prepared snapshot and accepted actions again before
applying. A changed memory scope, stale target, expired lease, invalid citation,
or invalid accepted action rejects the transaction. Successful application
commits accepted memories, citations, indexes, embedding work, and the
non-deferred evidence ledger together. With schema 14, deferred sessions get one
routine retry on a later scheduled occurrence. Other batches continue on the
same day, including after a restart. If the retry still yields no accepted
action citing a previously deferred session, the session stays pending and
parks. An empty model response can finish fresh evidence without creating a
memory. It preserves a prior deferral.

Parked evidence becomes eligible for another review when Moon finds related new
evidence in the same scope, the L2 grounding rules change, or an operator
requests the specific session. Moon matches distinctive words from the new
evidence against the deferred turn and includes the matched turn in the review
packet. This lexical check can miss a paraphrase. A manual review can still
select the session. Each trigger permits one reconsideration under the usual
citation and grounding checks. If the evidence still lacks support, it stays
parked.

From Moon 2.6.2, gateway diagnostics distinguish fixed failure phases and codes,
for example `phase=normalise code=lost_qualifier` or `phase=model code=timeout`.
L1 learning failures now report the same safe phase/code pair as L2 synthesis.
`phase=model code=backend_error` alone does not identify authentication as the
cause; correlate it with content-free host route diagnostics. These fields never
contain generated claims, original conversations or arbitrary provider errors. A
normalisation failure means the proposed JSON or evidence did not pass Moon's
checks; it does not imply that the model timed out. An apply failure means the
SQLite transaction was rejected, which can also happen when L1 changed the
memory scope while a longer L2 request was running.

The schema-13 adapter additionally distinguishes `runtime_unavailable`,
`runtime_setup_failed`, `authentication_failed`, `model_unavailable` and
`model_route_mismatch`. Known setup/auth/route failures do not try Moon's
fallback model. Locally detectable setup is checked before acquiring a source,
topic or daily-L2 lease. Source/topic jobs persist these codes as terminal
failures until explicit retry; daily session-L2 retains its existing bounded
daily-run policy.

Every helper response is checked against the selected provider/model. A
confirmed mismatch or unexpected host fallback prevents submission/application.
Successful daily-L2 `learning apply` metadata may include `execution`,
separating requested provider/model/reasoning from observed identity/backend.
Source and topic tools expose the same receipt as `model_execution`. Missing
observations remain `null`; they are not inferred from request settings.
`identity=verified` means both provider and model matched host metadata, not
independent server attestation. The current host does not expose reliable
transport reasoning effort; absence of fallback metadata is unknown, not a zero
count.

Use the [source-learning canary](source-learning-canary.md) for repeatable
isolated integration checks. It does not exercise the next scheduled session-L2
run or demonstrate general synthesis quality.

Grounding codes include `question_only`, `unsupported_number`, `lost_qualifier`
and `insufficient_overlap`. These are conservative checks, not proof that a
rejected sentence is false. For example, a useful paraphrase can fail wording
overlap; review the meaning before choosing a closer quotation.

For grounding failures, inspect a proposal against an isolated copy first.
Prefer narrow claims and short supporting quotations from the newest cited
evidence; preserve its conditions, uncertainty and negative wording. Omit a
candidate that cannot pass those checks. Do not weaken the validator or rotate
run keys to hide repeated failures. After a deliberate correction, an operator
can use the manual preparation/apply workflow below with the same run key and an
explicit bounded attempt allowance; ordinary daily retries remain limited to
three.

## Inspect and rehearse with the CLI

The commands below continue using the temporary `learning_home` above and an
explicit database. They use synthetic text and make no model or gateway calls.

```bash
moon --home "$learning_home" --database "$learning_home/state/moon.sqlite" init
moon --home "$learning_home" --database "$learning_home/state/moon.sqlite" \
  record --session-id learning-example --scope example <<'EOF'
User:
The Atlas project uses SQLite for its database.

Assistant:
Recorded the Atlas database decision.
EOF
moon --home "$learning_home" --database "$learning_home/state/moon.sqlite" \
  --json learning status
moon --home "$learning_home" --database "$learning_home/state/moon.sqlite" \
  --json learning related --scope example --query "Atlas database" \
  --limit 32 --max-chars 16000
moon --home "$learning_home" --database "$learning_home/state/moon.sqlite" \
  --json learning prepare --run-key manual-example \
  --limit 32 --max-chars 64000 --preview
```

`status`, `deferred`, `related`, and `prepare --preview` require an existing
current-schema database and do not create, migrate, lease, or change it. Preview
returns the candidate packet with no `run_id`. `related` requires an explicit
scope and returns bounded claims with source citations. Treat this content as
private conversation data, unlike content-free runtime metrics.

To rehearse applying a reviewed proposal, prepare a real lease in the same
fixture and copy its returned `run_id`:

```bash
moon --home "$learning_home" --database "$learning_home/state/moon.sqlite" \
  --json learning prepare --run-key manual-example \
  --limit 32 --max-chars 64000 --lease-ms 1200000 --max-attempts 3
```

Preparation is a write. `--before-ms` and `--after-ms` optionally bound evidence
completion times inclusively in Unix milliseconds. Lease length may be
1,000–86,400,000 ms, and manual `--max-attempts` may be 1–10. Keep a stable run
key when retrying; changing it bypasses that key's retry count.

Use `learning deferred --limit 32` to list session IDs and retry states. Page
forward using `--after-id`. The list omits conversation text.

To review one deferred session on request, supply
`--review-session <session-id>` and a new operation-specific `--run-key`. Add
`--preview` to inspect the selection without taking a lease. Preparing the
review does not approve a memory. The scheduler supplies `--occurrence` to group
its daily batches and `--rule-version` to identify the grounding policy. An
ordinary CLI review can omit the version to retain the existing policy identity.

For the fixture above, save this as `$learning_home/proposal.json`:

```json
{
  "actions": [
    {
      "action": "create",
      "canonical_key": "atlas:database",
      "kind": "decision",
      "title": "Atlas database",
      "content": "The Atlas project uses SQLite for its database.",
      "importance": 0.8,
      "confidence": 1.0,
      "evidence": [
        {
          "session_id": "learning-example",
          "quote": "The Atlas project uses SQLite for its database."
        }
      ]
    }
  ]
}
```

Replace `<prepared-run-id>` with the returned ID. First validate with rollback,
then apply only after reviewing the result:

```bash
moon --home "$learning_home" --database "$learning_home/state/moon.sqlite" \
  --json learning apply --run-id '<prepared-run-id>' \
  --input "$learning_home/proposal.json" --dry-run
moon --home "$learning_home" --database "$learning_home/state/moon.sqlite" \
  --json learning apply --run-id '<prepared-run-id>' \
  --input "$learning_home/proposal.json"
moon --home "$learning_home" --database "$learning_home/state/moon.sqlite" \
  --json learning status
```

`apply --dry-run` requires a real active lease, validates and executes the
transaction, then rolls it back; it is not a substitute for read-only preview.
It leaves the lease available for a subsequent apply. Plain `apply` writes the
result and processes selected evidence. From 2.6.2, an optional top-level
`"deferred_evidence": ["selected-session-id"]` leaves those selected sources
pending; IDs must be unique and belong to that prepared selection. This field is
set by the adapter or reviewing operator, never accepted from model output. The
outcome reports `action_count`, `processed_evidence`, `selected_evidence`, the
deferred count as `deferred_evidence`, and `deferred_session_ids`. Committed
`prepare` replies and `status` run records include the deferred count.
Reapplying a committed run returns its stored result. `--input -` accepts the
same JSON through stdin. These commands do not ask a model to generate
proposals.

To abandon an uncommitted prepared run, use the same temporary home:

```bash
moon --home "$learning_home" --database "$learning_home/state/moon.sqlite" \
  --json learning fail --run-id '<prepared-run-id>'
```

`fail` releases its lease without processing evidence. A dry run does not make
the snapshot immune to later changes; fail and prepare again if validation
reports that the scope changed. Manual `apply` accepts at most 32 actions and
enforces storage invariants; model-level correction and uncertainty checks also
run in the adapter, so manually authored proposals still require human review.

## Reviewed learning from captured sources

Completed-session evidence and external sources carry different authority. An
article is not a user instruction, and a captured author's statement is not a
verified fact merely because a model extracted it. Source learning therefore
uses its own durable job and approval contract; it does not submit external text
to the conservative session-L2 action validator.

An import with explicit `--allow-learning` creates its source revision and
required pending work in the same transaction. `source learning prepare` leases
one bounded packet for an authorised caller; `--preview` inspects it read-only.
The caller may use its configured host model to propose useful verbatim quotes.
The optional OpenClaw `moon_source_learn` tool provides this explicit step using
the enabled L1 route, reasoning, fallback and timeout. It requires trusted owner
authority and an explicit configured Moon home, caps the body at 64 KiB, and
submits review proposals only. No source timer is registered. Moon's CLI
performs no inference and stores no provider credentials.

`source learning submit` accepts version-1 JSON with a `claims` array. Each
entry contains an exact `quote`, optional `start_byte`, and optional
`valid_until_ms`. Unknown fields, unsupported versions, altered quotations and
invalid ranges are rejected. The submission is tied to the prepared job and
lease; revision, eligibility and lifecycle changes invalidate late results. It
remains a proposal until a reviewer inspects `source learning status
--job-id`
and approves the exact result hash using `accept --yes`. Model output cannot
supply this caller-side approval.

Accepted source claims retain attribution and exact source support. Recall
checks current support and expiry; source withdrawal stops learning and normal
recall immediately. Restoring a source can reactivate previously accepted
current/eligible quotes, but does not approve new claims. Rejected proposals,
failed attempts and expired leases remain inspectable and recoverable under
bounded retry rules. Failure reporting takes a fixed code rather than a provider
error body.

Use `source claims list/get` to inspect claims, `source state` for lifecycle
state, and `source delete-preview` before any explicitly authorised
`delete-apply --yes`. A changed deletion plan is rejected. Deletion affects the
working database; it cannot erase backup copies or retract text previously
provided to a model.

The
[source-storage workflow](source-storage.md#extract-and-review-source-claims)
contains examples in a temporary home. Source extraction has no automatic
service and only proposes verbatim claims. Existing L1/L2 model configuration
continues to govern completed-session learning; the explicit source tool reuses
L1 routing with its own fixed verbatim-extraction prompt.

The subsequent schema-11 [topic workflow](topics.md) adds revision-bound
cross-source compilation and transitive freshness checks. It has separate jobs
and typed statements; optional `moon_topic_compile` reuses enabled L2 routing
with a fixed prompt and submits review only. The completed-session L2 scheduler
does not compile topics.

## Before deploying to an existing runtime

Moon 2.8.1 accepts supported databases from schema 6 through 20 and migrates
them to schema 20. L1/L2 introduced schema 8; source storage, reviewed learning
and topics add schemas 9–11, retrieval-validity optimisation adds schema 12, and
job diagnostics adds schema 13. Moon 2.8.0 adds the session retry ledger in
schema 14, audience-bound learning in schema 15, and verified per-turn sender
records in schema 16. Schemas 17–19 preserve original ownership, record
reply-time room access and separate private learning by verified speaker. Schema
20 adds bot provenance and tracks linked Discord memory owners. Existing global
evidence stays in the operator review collection. Back up with the currently
installed pre-upgrade executable, export memories, and preserve the existing
OpenClaw configuration and any `moon.toml` before an authorised deployment. The
export is a generated view, not a replacement for the complete SQLite backup.
Keep the previous binary and its matching database backup: an older binary must
not open the newer schema.

Validate a copy with the new binary, inspect the L2 preview and proposed
actions, and verify the selected model route before enabling L2 in the live
file. `config validate` alone does not perform these checks. Use the signed
updater's rollback workflow for installation; do not replace a running binary or
plugin by copying development build files over it. See
[updating.md](updating.md), [openclaw-canary.md](openclaw-canary.md), and the
[recall evaluation protocol](memory-improvement-plan.md) for deployment and
quality gates.
