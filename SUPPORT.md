# Support

Start with [Getting started](docs/getting-started.md), the
[upgrade checklist](docs/updating.md#upgrade-checklist), or the
[skill refresh instructions](docs/updating.md#refresh-the-agent-skill).

## Where to get help

1. Usage questions and troubleshooting:
   - Open a GitHub issue in this repository.
2. Bugs:
   - Use the Bug Report issue template.
3. Feature requests:
   - Use the Feature Request issue template.
4. Security vulnerabilities:
   - Follow `SECURITY.md` and do not disclose publicly first.

## What to include

1. Moon version (`moon --version`) and `moon --json health` output.
2. OS + architecture.
3. Command you ran.
4. Moon's fixed error code and redacted diagnostic output.
5. Minimal reproducible steps.

For automatic recall problems, also include the OpenClaw version, the main model
selected for that turn, the context-engine and native-memory slots, the Moon
hook permissions, and content-free `moon metrics summary --since 7d` output. For
learning problems, include the L1/L2 model and reasoning settings. Use Moon's
fixed failure phase/code instead of arbitrary provider response bodies. A
working chat does not establish that a new learning helper can authenticate, and
an old injection metric does not prove current native recall. Do not post
credentials, private memory, evidence transcripts, or unredacted configuration.
See the
[native recall canary](docs/openclaw-canary.md#native-recall-compatibility).

For signed upgrades, include schema version, the installed skill's
`moon-version` marker and whether the signed bridge update completed.
Source/topic issues need job state and fixed failure code, not the source body
or raw model response. Unknown observed reasoning effort is a host metadata
limitation, not proof of the wrong model. Preserve failed update journals and
rollback bundles privately.

## Response expectations

This project is maintained on a best-effort basis. Critical security issues are
handled with priority according to `SECURITY.md`.
