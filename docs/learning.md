# L1 distillation and daily L2 synthesis

L1 extracts small, cited memories after a completed OpenClaw turn. L2 revisits
unprocessed evidence in daily batches, compares it with related memories and
their original evidence, and proposes confirmations, corrections, exact-content
merges, or review items. L2 is disabled by default.

These commands require Moon 2.6.0 or newer. Upgrading does not automatically
enable daily synthesis. For a source checkout, run `cargo build --locked` and
use `./target/debug/moon` in place of `moon` in the isolated examples below.

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

The accepted user request and final answer are committed as immutable evidence
before L1 starts. OpenClaw retries storage failures. L1 model failures do not
erase evidence; an enabled L2 can later revisit it. The default L1 limit is
three proposals per eligible turn, adjustable through the existing
`learningMaxMemories` plugin setting.

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
Expired memories leave normal recall but remain available with their evidence
for audit. This does not retroactively classify every old memory as temporary.

Related-memory selection is bounded, not a full database review. A prepared
packet reports `context_limited` and `omitted_memory_count` when related claims
cannot all fit. The model is told that comparison may be incomplete. If the
oldest pending evidence is too large, or no related claim and its original
evidence can fit, preparation fails and preserves the backlog. Inspect the
packet before increasing its budget; do not treat a partial packet as proof that
no conflict exists. `learning status` exposes review reasons and evidence
session IDs for human inspection; it is not an automatic review-resolution
command.

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

Each batch selects the oldest pending evidence in one scope and respects the
record and packet budgets. The scheduler commits at most `max_batches_per_day`
batches. A database-wide lease prevents overlapping prepared runs. Expired
leases become failed attempts, and three failed attempts for a daily batch key
exhaust that key and stop work for that occurrence. Each attempt can call the
configured primary and, if enabled, one fallback. Failed input remains available
for the next due date; exhaustion does not delete or mark it processed.

SQLite validates the prepared snapshot again before applying. A changed memory
scope, stale target, expired lease, invalid citation, or invalid action rejects
the whole batch. Successful application commits memories, citations, indexes,
embedding work, and the evidence-processing ledger together. A valid empty
action list still marks the selected evidence processed. Cancellation stops
routing without starting another fallback.

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

`status`, `related`, and `prepare --preview` require an existing current-schema
database and do not create, migrate, lease, or change it. Preview returns the
candidate packet with no `run_id`. `related` requires an explicit scope and
returns bounded claims with source citations. Treat this content as private
conversation data, unlike content-free runtime metrics.

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
result and processes selected evidence. Reapplying a committed run returns its
stored result. `--input -` accepts the same JSON through stdin. These commands
do not ask a model to generate proposals.

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

## Before deploying to an existing runtime

This implementation migrates SQLite to schema 8. Its release compatibility range
is schema 6 through 8. Back up the live database, export memories, and preserve
the existing OpenClaw configuration and any `moon.toml` before an authorised
deployment. Keep the previous binary and its matching database backup: an older
binary must not open a migrated schema-8 database.

Validate a copy with the new binary, inspect the L2 preview and proposed
actions, and verify the selected model route before enabling L2 in the live
file. `config validate` alone does not perform these checks. Use the signed
updater's rollback workflow for installation; do not replace a running binary or
plugin by copying development build files over it. See
[updating.md](updating.md), [openclaw-canary.md](openclaw-canary.md), and the
[recall evaluation protocol](memory-improvement-plan.md) for deployment and
quality gates.
