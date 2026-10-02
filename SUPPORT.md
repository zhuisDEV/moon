# Support

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
4. Exact error output.
5. Minimal reproducible steps.

For learning or automatic recall problems, also include the OpenClaw version,
selected context-engine slot, Moon hook permission flags, exact L1/L2 model and
reasoning settings, and content-free `moon metrics summary --since 7d` output.
Use Moon's fixed failure phase/code instead of arbitrary provider response
bodies. A working chat does not establish that a new learning helper can
authenticate, and an old injection metric does not prove current native recall.
Do not post credentials, private memory, evidence transcripts, or unredacted
configuration. See the
[native recall canary](docs/openclaw-canary.md#native-recall-compatibility).

## Response expectations

This project is maintained on a best-effort basis. Critical security issues are
handled with priority according to `SECURITY.md`.
