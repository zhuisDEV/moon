# Proactive compaction with Moon

Use this profile with the stock OpenClaw harness and the `moon-local` safeguard
provider. It keeps summarisation inputs smaller while OpenClaw remains the only
owner of transcript replacement. It is opt-in; updating Moon does not change
OpenClaw configuration.

## Trigger and retained history

The [configuration patch](proactive-compaction.patch.json) enables OpenClaw's
native preflight size check at **240,000 active transcript bytes**. Before the
next request, OpenClaw checks the context still active since the most recent
compaction or reset, including its retained tail. It does not count all archived
history. This is a byte threshold, not a tokenizer measurement or a promise of
60,000 tokens. Tune it using representative local-model measurements.

Compaction summarises the oldest eligible prefix. OpenClaw chooses the cut and
keeps a recent tail with `keepRecentTokens=20000`, adjusting boundaries for tool
exchanges. Its safeguard also retains context from three recent turns through
`recentTurnsPreserve=3`; that setting is not a guarantee that three arbitrarily
large turns remain verbatim. Existing summaries are carried into subsequent
summary requests. Unfinished work, constraints and relevant exact identifiers
must survive regardless of age.

There is no wall-clock timer or parallel summarisation of a running response. An
idle session needs no model calls. A new request may wait for preflight
compaction. The existing token/overflow checks remain enabled. The host avoids
repeated size-triggered compaction when the retained tail is still over the byte
limit, until sufficient additional history accumulates.

The inspected OpenClaw 2026.9.2 durable `commitTurn` path does not invoke the
ordinary post-turn maintenance hook. Adding an `afterTurn` timer to Moon would
therefore not reliably provide proactive compaction. Use the supported preflight
trigger instead. Native Codex harnesses manage their own compaction; do not
apply this profile to them, because the host's byte cap can restart an oversized
native Codex thread.

## Smaller summary input

Moon projects messages into conversation text, complete tool calls and results,
tool identifiers and error flags. It drops message-envelope bookkeeping and
stored reasoning blocks. Recognised media bodies become explicit omission
markers. Text, commands and errors are not truncated; arbitrary tool arguments
and business fields inside structured results remain intact.

This changes only the temporary summary prompt. Moon does not delete the source
transcript or rewrite its stored reasoning. OpenClaw retains responsibility for
applying the resulting summary and keeping the existing transcript on failure.
The provider rejects empty/error results; this is not proof that a model has
preserved every semantic detail. Review summary quality in a long-session canary
before enabling the profile for important ongoing work.

## Review and enable

First install the candidate Moon adapter through the normal approved update
workflow. Keep `agents.defaults.compaction.mode=safeguard` and
`agents.defaults.compaction.provider=moon-local`. The patch leaves the chosen
model, credentials, reasoning, summary output limit, timeout and other
compaction settings unchanged.

From the repository root, validate the proposed settings without changing them:

```bash
openclaw config set --batch-file docs/proactive-compaction.patch.json --dry-run
```

After approval, back up the live configuration and database, then apply and
restart at an idle boundary:

```bash
openclaw config set --batch-file docs/proactive-compaction.patch.json
openclaw config validate
openclaw gateway restart --safe
```

Read back the four changed settings and the loaded plugin path. Verify an
isolated session compacts an older prefix, retains complete recent tool pairs,
and resumes the next request. A failed preflight preserves the transcript but
can still prevent that request from proceeding. Removing unnecessary input does
not guarantee that every model or transcript will finish within its deadline;
OpenClaw and Moon impose separate timeouts.

## Offline validation

Run the adapter suite with a compatible Moon binary and the native-host fixture
with an installed OpenClaw package directory:

```bash
sh tools/test-openclaw-adapter.sh /path/to/moon
node tools/test-openclaw-compaction.mjs /path/to/node_modules/openclaw
```

The first command uses an explicit temporary Moon home and real SQLite. The
second uses OpenClaw's public `agent-core` SDK and synthetic in-memory messages.
It checks oldest-prefix selection, complete retained tool pairs, previous
summary handoff, candidate provider input and unchanged source on errors. Model
responses are mocked; neither test establishes real-model latency or summary
quality. The host fixture uses OpenClaw's existing Node runtime, with no new
Moon runtime dependency.
