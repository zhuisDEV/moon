# How Moon works

Moon turns completed work into small, cited context packets for later agent
turns. One Rust binary owns the full path. SQLite stores the evidence, memories,
indexes, citations, and runtime state; FTS5 and statically linked `sqlite-vec`
provide search. QMD and a separate vector service are not involved.

Moon 2.9.1 uses database schema 21. For installation, start with
[getting started](getting-started.md); for an existing runtime, follow
[updating](updating.md), including its `SKILL.md` refresh guidance.

Normal chat learning is automatic. Captured sources and reviewed topics are
separate, explicit workflows; enabling daily L2 does not start a source worker
or topic compiler. All retained knowledge lives in SQLite. Documentation and
generated Markdown exports are still files, but are not the writable knowledge
store.

The manual commands below are examples for an isolated runtime. Set it up once
and keep the variable in the same shell:

```bash
moon_example_home="$(mktemp -d /tmp/moon-workflow.XXXXXX)"
moon --home "$moon_example_home" --database "$moon_example_home/state/moon.sqlite" init
```

They do not modify `~/.moon`. Commands containing sample IDs or file paths need
the values returned by your own run; use the [learning](learning.md) and
[source](source-storage.md) guides for complete fixtures. Both `--home` and
`--database` are explicit so an inherited `MOON_DATABASE` cannot redirect the
examples to another database.

## Key workflows

### 1. After a completed turn: record immutable evidence

After OpenClaw completes an eligible turn with a verified audience, the adapter
records the accepted human message as private evidence. The final assistant
answer marks completion. A matching successful Discord delivery can retain the
answer as role-labelled private history. L1 and L2 learn from accepted human
words. L2 can also use a delivered proposal when the responding human message,
transcript adjacency and owners can all be verified. Tool traffic stays outside
the turn history. OpenClaw's `commitTurn` callback supplies only the closed,
accepted transcript range. A SHA-256 identity derived from its advancement key
is committed with the evidence in one SQLite transaction. This makes retries and
concurrent acknowledgements idempotent. The parent session and channel identity
stay in sanitized metadata. Moon does not read the growing transcript during
assembly or commit.

When OpenClaw's native memory slot is disabled, the prompt hook can match an
active user message in the default host SQLite store to one original Discord
post. In a one-to-one DM isolated to the bot account and peer, Moon verifies the
recipient before releasing eligible private context. In an ordinary guild text
room, Moon verifies room access and uses public context. The author owns the
question, and the same durable commit path records the evidence after the reply.

The selected OpenClaw main model reads the full current request before any
stored-memory lookup. For a follow-up, the adapter can include up to two short
text excerpts already present in the same session's prompt history. Their text
must match recent verified message receipts for the same speaker, audience and
owners. The adapter retains only digests for this check. A fresh session or a
restart begins without prior receipts.

Moon can verify the accepted sender against a
[personal OpenClaw profile](memory-privacy.md#personal-openclaw-profile). In
configured Discord text rooms, a fresh bot post can also become private L2
evidence for the linked human owner. Its citation preserves the bot as author.

Storage failures remain in OpenClaw's durable retry queue. Extraction runs only
after a new evidence commit and is best effort: a crash between the commit and
extraction can skip L1, while the evidence remains available for optional daily
L2 synthesis. Heartbeats, plugin-level disabled learning, and turns without a
visible final answer are no-ops. Disabling only the L1 stage still records
evidence.

The same operation is available manually:

```bash
moon --home "$moon_example_home" --database "$moon_example_home/state/moon.sqlite" record \
  --session-id session-2026-07-27-001 \
  --scope moon \
  --title "Memory architecture decision" \
  < /path/to/completed-turn.txt
```

`record` performs conservative secret scrubbing, indexes the sanitized content,
and stores the completed session as immutable evidence. Repeating the same
command is harmless. Reusing the session id with changed content or metadata is
rejected instead of silently rewriting history. Document creation and evidence
identity are committed in one transaction.

Scrubbing includes quoted credential fields in JSON embedded in conversation
text. Redaction improvements apply to new writes; they do not silently rewrite
historical evidence or its citations. Replaying old content whose sanitised form
has changed can therefore produce an immutable-evidence conflict.

OpenClaw calls the hook only after the turn has finished. Heartbeats and turns
without a verified admission, a user request or a visible final answer are
skipped. A recognised credential also skips the evidence write. Only durable
candidates proceed to distillation. See
[public and private memory](memory-privacy.md).

### 2. Distill evidence into a durable memory

Evidence is history. A memory is the smaller claim Moon should recall. Distill a
stable claim with a canonical key and an exact supporting quote:

```bash
moon --home "$moon_example_home" --database "$moon_example_home/state/moon.sqlite" distill \
  --key moon:storage-engine \
  --kind decision \
  --scope moon \
  --title "Canonical storage" \
  --content "Moon uses one SQLite database and does not require QMD." \
  --session-id session-2026-07-27-001 \
  --evidence-quote "Moon uses one SQLite database and does not require QMD." \
  --importance 0.95 \
  --confidence 1.0 \
  --pinned
```

Moon stores the exact byte and line range of the quote. The memory can therefore
show where it came from instead of presenting an unsupported recollection.

L1 receives the current eligible turn and a bounded selection of active claims
from the same learning scope. It decides what is worth remembering and whether
the speaker is making a correction. The default limit is three proposals. When a
correction needs an earlier original message, L1 can leave it for L2. The
runtime's `moon.toml` selects L1's model and reasoning effort independently.
Known setup, authentication and route failures stop the attempt.

Moon verifies the proposed keys, score bounds, numeric literals, exact source
quotes and correction targets. L1 writes the turn's proposals together. A
confirmation of an active user claim needs evidence from the user. An assistant
repeating that claim cannot confirm it.

Temporary observations expire relative to the source evidence time. Their
revisions and evidence remain available after expiry.

The model prompt stays inside OpenClaw's embedded runner. Proposal payloads
travel to the Moon binary through stdin, not process arguments.

Canonical keys drive consolidation:

- Same key and same content: confirm the existing memory, add the new citation,
  and raise confidence or importance when appropriate.
- Same key and different content: stop with a conflict. Nothing is replaced.
- Reviewed replacement: rerun with the active document id from the conflict, for
  example `--supersedes 42`. The old memory remains as history but is excluded
  from normal search and context packets.

This makes corrections explicit and prevents a newer sentence from quietly
overwriting an earlier decision. Every changed value receives a new immutable
revision, even when a later decision restores content used by an older revision.
SQLite permits only one active row per canonical key and rejects supersession
cycles.

Use `kind=summary` with `--pinned` for a short project summary. Pinned summaries
form the stable top layer of a project context packet.

### 3. Embed durable memory automatically

Every new active memory enters the embedding queue at high priority. Imported
and indexed reference chunks enter at normal priority. Raw turn evidence stays
lexical and citation-only: it is not duplicated into the vector index.

After a completed OpenClaw turn, the adapter asks a private Moon child process
to drain a bounded batch. The child communicates only through stdin/stdout; it
has no TCP port and is stopped with the adapter. Keeping that process alive
keeps the local model warm without adding a separately installed daemon. Queue
leases survive crashes, failures use bounded exponential backoff, and five
failed attempts become a visible dead-letter health condition. A valid memory
write is never rolled back because its later embedding failed.

The production model is `intfloat/multilingual-e5-small`. Moon uses distinct
`query:` and `passage:` inputs. Oversized chunks are split at tokenizer-safe
boundaries, embedded in subsegments, mean-pooled, and normalized back into one
vector, preserving the original chunk offsets and citations.

For a deliberately authorised full rebuild or model transition (not routine
maintenance):

```bash
moon --home "$moon_example_home" --database "$moon_example_home/state/moon.sqlite" requeue-embeddings
moon --home "$moon_example_home" --database "$moon_example_home/state/moon.sqlite" --json embed --provider local --limit 64 --drain
```

`health` reports active-memory and reference coverage separately and confirms
that evidence has zero vectors.

### 4. Search locally first

Every recorded or distilled document is immediately searchable with SQLite FTS5.
This is the normal high-performance path and requires no model login, network
service, QMD process, or API key:

```bash
moon --home "$moon_example_home" --database "$moon_example_home/state/moon.sqlite" context \
  --query "How should Moon store and retrieve long-term memory?" \
  --scope moon \
  --mode lexical \
  --max-chars 8000
```

Production hybrid retrieval uses the local multilingual model and does not
require a model login, API key, network service, QMD process, or remote
embedding endpoint after its initial model download:

```bash
moon --home "$moon_example_home" --database "$moon_example_home/state/moon.sqlite" context \
  --query "How should Moon store and retrieve long-term memory?" \
  --scope moon \
  --mode hybrid \
  --provider local \
  --max-chars 8000
```

The optional `hash` provider remains only for deterministic vector-plumbing
tests and is not presented as semantic-quality recall.

The examples request the recommended 8,000-character cap explicitly. The CLI and
adapter's compiled defaults remain 3,500; an existing adapter uses its
configured `maxChars`. A larger cap permits more evidence but does not force
padding or increase the number of stored references.

One database contains one embedding space. Moon rejects mixed model names or
dimensions until an explicit re-embedding migration is performed.

Lexical retrieval uses FTS5 Porter stemming, shared lightweight inflection
normalization, Unicode lowercasing, and a bounded edit-distance fallback for
active memories when strict search is empty. This handles common singular,
plural, and spelling mistakes without scanning the full reference corpus. Hybrid
vectors add paraphrase and synonym recall.

### 5. Route model work through OpenClaw

Moon has no direct API-key model route or provider credential store. L1 and L2
read separate settings from `<selected-home>/moon.toml`, outside installed
release directories. The adapter reloads that file on each accepted turn and
scheduler tick. Optional stage fields override the inherited route:

1. `agents.defaults.model.primary` from OpenClaw, unless `primaryModel`
   overrides it.
2. The first OpenClaw fallback, unless `fallbackModel` overrides it.

Both values use OpenClaw's provider-qualified `provider/model` form. Providers
such as vLLM, OpenAI, Anthropic, or Google therefore use the same Moon path.
OpenClaw owns their authentication and transport. In the verified native Codex
setup, the `openai` provider uses the app's Codex binary with the user's
existing `CODEX_HOME` OAuth and `homeScope: "user"`; Moon reads no auth file and
creates no second login.

Each model attempt uses OpenClaw's `runEmbeddedAgent` capability with detached
in-memory persistence and an owner-preserving
`agent:<owner>:internal-session-effects:incognito-<id>` key. Native Codex
recognises that shape as an ephemeral thread. Helpers have tools disabled,
reject ambiguous owner selection, and never reuse the live conversation's
transcript. This requires OpenClaw 2026.9.2 or newer, which the signed release
manifest enforces. It retains existing model routing without adding
model-override permissions for the newer `llm.complete` capability.

For eligible temporary failures, the adapter tries an enabled fallback once.
Known setup, authentication and route mismatches stop without Moon fallback.
Cancellation stops routing immediately; partial output from failed or timed-out
runs is not accepted. If both routes fail, the recorded evidence remains
available. Moon does not persist or print arbitrary provider diagnostics.
Independent stage effort is passed as OpenClaw `thinkLevel`, with visible
reasoning output disabled. The starter chooses Astra `low` for L1 and `xhigh`
for L2. Missing configuration preserves the existing route and disables L2;
valid inherited Astra effort is retained, and incompatible inherited `off` is
adapted to the stage's supported default.

Custom UTF-8 prompts up to 64 KiB supplement fixed guards rather than replacing
them. Input, timeout, action, and attempt limits are enforced separately. The
native Codex backend inspected with OpenClaw 2026.9.4 ignores the requested
`max_output_tokens`; it must not be presented as an enforced token-usage cap.
OpenClaw 2026.9.7 also treats that hint in `streamParams` as a transport
override, excluding native Codex during route selection. Moon omits it for
native runtimes and implicitly routed OpenAI helpers, while preserving it for
explicit stock OpenClaw routes and ordinary local providers. The host still
chooses the configured runtime and authentication; Moon does not force a native
route or copy credentials. See [learning.md](learning.md) for the complete
configuration and limits.

### Daily reconciliation alongside turn-time learning

L2 runs only when enabled in `moon.toml`. The existing plugin service checks
every minute and on startup, using an IANA timezone and the most recent due
daily cutoff. DST gaps shift the occurrence forward; repeated wall times share
one date key. Restart catch-up includes older pending evidence without creating
a separate model job for every missed day. Each date's batches share one fixed
evidence cutoff.

In Moon 2.8.0, `learning prepare` selects fresh evidence before eligible
retries. Each snapshot stays within one scope and includes related active
memories and their original source evidence. A second unsuccessful review parks
the evidence until related new evidence, changed grounding rules, or an explicit
session review makes it eligible again. It reports partial comparison context
explicitly. The default daily budget is eight batches of at most 32 selected
records and 64,000 packet characters, with at most 16 proposed actions per
batch. A batch key permits three failed attempts before that occurrence stops;
each attempt can use one primary and one enabled fallback. Lease expiry and
committed run keys survive restarts.

L2 can create, confirm, supersede, merge, or request review. Each action needs
an exact citation from selected evidence. A correction can also cite older
original messages in the same snapshot. For example, one message gives a
notebook its label and the next corrects its colour. L2 can cite both, update
the old notebook and record the label on the right notebook as one change. It
should defer or request review when several notebooks fit the correction.

Moon validates linked actions together. Confirmations preserve exact content.
Merges require identical content, scope and kind, and retain citations, aliases,
expiry, confirmation history and unresolved reviews. The model only sees the
related records that fit in the prepared snapshot.

`learning apply` checks that the leased snapshot is still current and commits
all actions and evidence-processing marks atomically. Failed or cancelled work
leaves evidence pending. `prepare --preview` is read-only; `apply --dry-run`
requires a real active lease and rolls back its transaction. The Rust commands
do not run a model. The full isolated workflow and review limits are in
[learning.md](learning.md#inspect-and-rehearse-with-the-cli).

### 6. Before an agent turn: assemble a context packet

On hosts with Moon's conversation-hook permission enabled, automatic recall uses
`before_prompt_build` and returns a defensive `prependContext` packet. The
adapter prefers the current admitted user request over reconstructed history,
skips internal learning and heartbeat turns, and avoids a second injection
through `assemble`. Without that permission or hook API, the legacy assembly
path remains. OpenClaw 2026.9.7 native-owned routes skip assembly, so they
require the hook; see the
[canary settings](openclaw-canary.md#native-recall-compatibility).

The model can request up to three grounded searches, request recent original
messages, decline recall or ask for clarification. Original-message requests
default to the current conversation. Each search uses the same reader grant,
count and character limits. On historical questions with permission for
originals, a matching current record can carry dated sources and up to two
directly linked earlier revisions. Their timestamps describe when something was
recorded. The model judges what the sources establish about the date in the
question.

After retrieval the same chat model selects supported IDs. Moon renders the
corresponding records, preserves source speakers and rechecks access before
returning a private contribution. Empty selections produce a short unavailable
notice. An opt-out makes zero stored-content reads. If OpenClaw cannot identify
or run the turn's model, the adapter contributes no stored evidence. Legacy
assembly needs a host-resolved model and has access to explicit publications.

The operator CLI below demonstrates a separate scoped lookup that can also
include reviewed sources and topics.

```bash
moon --home "$moon_example_home" --database "$moon_example_home/state/moon.sqlite" context \
  --query "How should Moon store and retrieve long-term memory?" \
  --scope moon \
  --mode hybrid \
  --provider local \
  --max-chars 8000
```

The scoped operator packet is assembled in this order:

1. A bounded quota of active pinned summaries for the requested scope, plus
   global pinned summaries.
2. Relevant active memories from lexical and vector retrieval.
3. Memory metadata such as kind, confidence, and canonical key.
4. A bounded number of exact evidence citations.
5. Deduplicated source evidence: complete accepted source statements and
   unreviewed document excerpts with exact source ranges.
6. Fresh reviewed topic explanations that fit the remaining capacity and topic
   quota, without displacing the selected complete source evidence.

Duplicate chunks collapse back to one memory. Superseded and expired memories
are excluded. Retrieved references never become canonical memories merely
because they were imported or ranked highly. If the character budget is tight,
Moon can reduce personal-memory citations and raw document excerpts while
preserving UTF-8 boundaries. Accepted source statements and typed topics are
kept whole or omitted, so a budget cut cannot drop their final qualification.
The output explicitly fences all recalled content, evidence, and references as
untrusted data rather than agent instructions. Pinned context cannot consume
every result slot when relevant search matches exist.

Reference selection balances meaningful query clauses, discounts common words
and can follow a strongly matched topic to bounded exact current source
premises. The [four-budget comparison](packet-selection.md) records the current
quality trade-offs; increasing a packet cap alone does not fix selection misses.

Context candidates must cover enough meaningful query terms. Named entities and
dates act as anchors, preventing a generic chart note from satisfying a query
for Albert Einstein merely because both contain the word `chart`. The adapter
skips greetings before invoking Moon. If no relevant memory or reference
survives, the text command emits no packet and the adapter injects nothing.

Anchor filtering, typo tolerance, and semantic ranking must be evaluated
together against reviewed natural queries. See
[memory-improvement-plan.md](memory-improvement-plan.md) for the observation
period, classifications, and release gates; do not weaken the safety filter from
one isolated miss.

Adapters should use `--json` and keep the packet below trusted instructions.
Without `--json`, `context` emits defensive Markdown for inspection and
low-trust prompt data; its labels are not a security boundary.

### 7. Compact the active context safely

OpenClaw memory search and the legacy memory plugin remain disabled. Moon owns
retrieval and the bounded context packet, but it does not own transcript
compaction. The adapter advertises `ownsCompaction=false`, leaving automatic
compaction enabled in the selected agent harness. When the runtime explicitly
identifies the stock OpenClaw harness, manual compaction can delegate to its
native runtime. For Codex or an unknown harness, Moon refuses the generic
fallback because OpenClaw 2026.7.1-2 was observed to summarize a populated Codex
transcript as empty. A safe no-op is preferable to losing context.

Moon packets are ephemeral assembly input. They are not copied into the stored
transcript, do not become durable memory by themselves, and must not be
duplicated into a compaction summary.

OpenClaw 2026.7.1 also exposes a narrower compaction-provider seam. When
`agents.defaults.compaction.provider=moon-local`, the adapter sends only the
prepared safeguard-summary input to `compactionModel` in an isolated raw-model
session with `compactionReasoning=off` by default. The configured provider must
support its native thinking-off request shape. The call has no tools, Moon
retrieval, or hidden provider fallback. OpenClaw still chooses tool-safe chunk
boundaries, retains recent and split turns, appends the compaction entry, and
owns checkpoints, transcript rotation, and rollback. If the provider fails,
OpenClaw uses its explicitly configured built-in compaction model.

### 8. Correct stale memory

Suppose `moon:search-policy` currently points to document 42, but a later review
approves a new policy. The first changed `distill` call reports a conflict and
names document 42. After review:

```bash
moon --home "$moon_example_home" --database "$moon_example_home/state/moon.sqlite" distill \
  --key moon:search-policy \
  --kind decision \
  --scope moon \
  --content "Moon uses the newly approved semantic rerank policy." \
  --session-id session-2026-07-27-002 \
  --evidence-quote "Moon uses the newly approved semantic rerank policy." \
  --supersedes 42
```

The previous row and its evidence remain auditable. Only the new head is used
for recall.

### 9. Back up, export, and verify

Create a consistent SQLite backup before operational experiments. This example
backs up only the isolated home above; for a live upgrade, use the currently
installed pre-upgrade executable and the complete database/configuration/skill
procedure in [updating](updating.md):

```bash
moon --home "$moon_example_home" --database "$moon_example_home/state/moon.sqlite" backup \
  --destination /path/to/moon-backup.sqlite
```

Exporting memory creates a generated Markdown view. SQLite remains canonical:

```bash
moon --home "$moon_example_home" --database "$moon_example_home/state/moon.sqlite" export \
  --destination /path/to/MEMORY.export.md
```

Use `health`, `benchmark`, and `shadow` to verify database integrity, retrieval
latency, and behavior against the read-only legacy Moon corpus. `health`
requires an existing current-schema database; it never creates or migrates one.

## The three layers

### Evidence

Evidence is a sanitized, immutable completed-session record. It answers: “What
actually happened, and where did this memory come from?”

### Durable memory

A durable memory is a compact claim with a canonical key, kind, scope,
importance, confidence, validity, and optional pinned status. It answers: “What
should Moon recall next time?”

### Context packet

A context packet is an ephemeral, bounded selection of active memories and
supporting citations plus clearly separated unreviewed references for one query.
It answers: “What does this agent need right now?”

Keeping these layers separate is the important context-engine design principle.
Evidence is not injected wholesale, memory is not treated as unquestionable
truth, and the context window is not filled with everything Moon has stored.

## SQLite ownership

Explicit external source capture is a separate lifecycle. The `source` commands
retain immutable, sanitised source revisions and generate exact revision-bound
citations. Eligible capture creates durable source jobs; explicit preparation
and submission produce bounded verbatim quote proposals. Review and acceptance
are required before supported source-derived claims can join recall. Source
bodies and these jobs do not enter completed-session L1/L2. See
[source storage](source-storage.md) for intake, revision/lifecycle checks,
review and the trust boundary.

The main lifecycle tables are:

- `documents`, `chunks`, `chunk_fts`, and `chunk_vectors`: canonical content and
  retrieval indexes.
- `evidence_sessions`: immutable completed-session identities and metadata.
- `memory_items`: durable claims and lifecycle state.
- `memory_heads`: the current document for each canonical key.
- `memory_citations`: exact links from a claim to evidence byte and line ranges.
- `memory_aliases`: historical keys retained when identical claims consolidate.
- `learning_runs`, `learning_processed_evidence`, and `learning_reviews`:
  reconciliation snapshots, expiring leases, processing history, and unresolved
  conflicts with evidence identifiers.
- `embedding_queue`: prioritized, leased, retryable vector work.
- `context_metrics`: content-free request performance, delivery, and bounded
  human review labels.
- `runtime_metrics`: content-free learning, embedding, and compaction events.
- `runtime_state`: adapter checkpoints.
- `sources`, `source_revisions`: scoped source identities and immutable
  sanitised captures, kept outside direct recall and session learning.
- `topics`, `topic_aliases`, `topic_inputs`: stable scoped compilation
  definitions.
- `topic_jobs`, `topic_dependencies`, `topic_revisions`: recoverable proposals,
  exact flattened input dependencies and immutable reviewed typed explanations.

Numbered migrations update these tables transactionally. Generated Markdown is
never a second writable source of truth. Moon 2.7.0 added schemas 9–13 to the
existing schema-8 session lifecycle: source storage, reviewed source learning,
topics, retrieval optimisation and bounded job diagnostics. Moon 2.8.0 adds the
deferred-learning retry ledger and the audience, ownership and speaker records
used in chat recall. See [Public and private memory](memory-privacy.md). Back up
using the currently installed pre-upgrade executable, retain configuration and a
matching database for the previous binary, then use the signed update procedure.
Merely building this checkout does not change an installed runtime.

Source learning is an explicit CLI/host workflow rather than an eighth automatic
adapter operation. Its accepted claims retain exact source quotes and
attribution. Source withdrawal, revision changes, eligibility and expiry are
checked before application and normal recall, so delayed processing cannot make
stale support appear current. [Topic compilation](topics.md) adds explicit
cross-source synthesis over accepted inputs, with transitive read-time freshness
checks and separate operator acceptance. Optional `moon_source_learn` and
`moon_topic_compile` tools use L1 and L2 routes respectively; neither is a
scheduled worker. Topic facts remain exact quotations, while interpretation,
disagreement and question blocks retain their labels and bounded packet share.

On Unix, runtime directories are owner-only (`0700`) and SQLite databases,
backups, and exports are owner-only files (`0600`).

## Current integration boundary

The OpenClaw adapter owns seven bounded operations:

1. retrieve relevant context before a turn;
2. mark whether the content-free request metric was injected;
3. record the completed user/final-answer pair after a turn;
4. selectively distill evidence-backed memories through L1;
5. optionally generate a safeguard compaction summary through a configured local
   model; and
6. drain a bounded local-embedding batch after completed turns and successful L2
   batches; and
7. optionally reconcile pending evidence through daily L2 synthesis.

Retrieval and learning failures fail open by default and never suppress the
agent's reply. OpenClaw still owns its transcript and compaction lifecycle even
when Moon supplies the summary text. Moon does not copy tool traces or
reasoning, run a separate watcher, or require QMD.

Automatic extraction remains deliberately conservative. A changed canonical
claim is replaced only when an explicit correction is supported by newer
evidence and the proposal names the active head supplied for comparison. Other
conflicts retain their evidence and may produce review items without replacing
the claim. Temporary observations can expire from normal recall without being
deleted.

Recall and lifecycle improvements follow the executable evaluation protocol in
[memory-improvement-plan.md](memory-improvement-plan.md). Automatic metrics
measure volume, packet delivery, and latency; human labels remain required for
recall-quality claims. The protocol is not authorization to change production
policy.
