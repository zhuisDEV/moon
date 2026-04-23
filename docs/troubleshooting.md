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
   silently fall back to bare `moon` if that resolution returned an empty
   value.
2. In that state, the gateway runtime tried to spawn `moon` from `PATH`
   instead of using the configured absolute binary path.

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
