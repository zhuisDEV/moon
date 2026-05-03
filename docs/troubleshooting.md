# M.O.O.N. Troubleshooting

Use this file for known failure modes, diagnostics, and recovery steps.

For first-time setup and normal operations, use:

1. [`README.md`](../README.md)
2. [`BOOTSTRAP.md`](../BOOTSTRAP.md)

## `spawn moon ENOENT` after OpenClaw restart

### Symptom

1. OpenClaw logs repeated context-engine failures such as:
   - `context engine assemble failed, using pipeline messages: Error: spawn moon ENOENT`
   - `context engine afterTurn failed: Error: spawn moon ENOENT`

### Likely cause

1. Older Moon plugin builds could send a configured absolute
   `plugins.entries.moon.config.moonPath` through host path resolution and
   silently fall back to bare `moon` if that resolution returned an empty value.
2. In that state, the gateway runtime tried to spawn `moon` from `PATH` instead
   of using the configured absolute binary path.

### Resolution

1. Upgrade to Moon `1.1.4` or later:

```bash
moon update
```

2. Realign the OpenClaw plugin and restart the watcher path:

```bash
moon restart
```

3. Confirm the runtime is healthy:

```bash
moon --version
moon status
```

4. Send a few normal OpenClaw turns and then inspect:
   - `~/.openclaw/logs/gateway.err.log`
   - `~/.openclaw/logs/gateway.log`

### Expected result after the fix

1. No new `spawn moon ENOENT` entries appear after the restart/alignment point.
2. If launch still fails, Moon `1.1.4+` logs the resolved executable path and
   process cwd so the remaining cause is visible in the gateway error log.

### Scope

1. This was not just a local shell `PATH` problem.
2. The bug lived in Moon's plugin-side path handling, so it could affect any
   install where OpenClaw supplied an empty or lossy resolved path for an
   already-absolute `moonPath`.

## Active context keeps answering the previous topic

### Symptom

1. The user changes topics, but the next assistant answer still continues an
   older topic.
2. The issue is most visible when the new topic is short, multilingual, or
   CJK-heavy and the prior topic contained strong English technical terms.

### Likely cause

1. Moon active packet retrieval builds query terms from the current user turns.
2. Older builds tokenized mostly ASCII text, so Chinese/CJK topic turns could
   fail to produce strong current-query terms.
3. Sparse query fallback then reused whole-session keywords, allowing stale
   terms such as earlier config or gateway work to dominate relevance scoring.
4. If an injected `# Moon Active Context` packet was replayed into the
   transcript, Moon could also treat that synthetic packet as real assistant
   history and reinforce stale context.

### Resolution

1. Upgrade to Moon `1.2.5` or later.
2. Re-run normal install/update alignment:

```bash
moon update
moon install
moon verify --strict
```

3. Confirm active packet logs no longer carry stale-topic evidence after a topic
   switch:
   - `moon context-engine --source <session.jsonl> --session-id <id>`
   - inspect `$MOON_HOME/mcp/<id>.md`

### Expected result after the fix

1. Chinese/CJK topic turns contribute current-query terms.
2. Sparse current prompts borrow from the recent conversation tail, not from the
   whole session.
3. Replayed `# Moon Active Context` packets are filtered during projection.
4. Old packets remain allowed only as an explicit recovery fallback, not as
   normal primary context input.
