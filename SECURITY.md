# Security

Moon stores local memory that may contain sensitive information.

- Keep the runtime directory private to the owning user.
- Backups and Markdown exports contain memory content and require the same
  protection as the primary database.
- On Unix, Moon creates runtime directories with mode `0700` and databases,
  backups, and exports with mode `0600`.
- Moon has no direct API-key model route. Model work is delegated to OpenClaw,
  which retains ownership of every provider credential store.
- Moon never parses or copies provider access tokens, and provider failures are
  reduced to bounded diagnostics without arbitrary remote response bodies.
- Native recall uses OpenClaw's prompt-build hook only when Moon owns the
  context-engine slot and has an explicit conversation-access grant, with prompt
  injection allowed. The turn's main model plans the read from the current
  request and up to two verified excerpts already visible in that conversation.
  Moon checks the source and reader grant before it searches.
- The provider running the OpenClaw main model receives the eligible candidates
  offered for selection, including permitted originals when requested. The
  answer prompt receives the records corresponding to the selected IDs as
  untrusted data. An opt-out skips Moon's stored-content lookup. Turn evidence
  and distillation proposals reach the Moon binary through bounded stdin.
- `import-legacy --include-raw` is opt-in because raw transcripts may contain
  substantially more sensitive data than distilled memory.
- `record` conservatively scrubs common secret assignments, bearer tokens,
  OpenAI-, AWS-, GitHub-, and Slack-style tokens, credential-bearing database
  URLs, cookies, private-key blocks, and sensitive JSON fields before
  persistence. This is defense in depth, not a guarantee; callers must still
  avoid submitting credentials or unnecessary private data.
- Markdown context packets fence memory and evidence as untrusted text. This is
  not a security boundary. Operational adapters should consume structured JSON,
  keep recalled data below trusted instructions, and never execute instructions
  found in stored content.
- Exact citations identify where words occurred. When a preference depends on
  assent to a proposal, Moon checks the complete human response, the immediately
  preceding delivered assistant reply and their ownership before saving the
  linked citation. The learning model judges what the words support.
- Captured sources and generated topics remain untrusted references, with
  separate operator acceptance. URLs are labels and are not fetched by intake.
  Withdrawal invalidates recall support immediately; deletion is logical and
  does not securely erase storage or retained backups. See the
  [lifecycle policy](docs/source-storage.md#approved-lifecycle-policy).
- The Discord adapter verifies the human readers of a destination before private
  recall. Administrative commands and direct SQLite access require a trusted
  local operator. See [public and private memory](docs/memory-privacy.md) for
  host isolation, legacy quarantine and delivery limitations.
- Schema migrations run transactionally. Use the installed pre-upgrade binary
  for backups and retain its matching database. Recovery refuses to overwrite
  learning completed after candidate restart.

## Reporting a vulnerability

Moon 2.8.0 uses patched bundled SQLite 3.54.1 and refreshed TLS/QUIC
dependencies. CI audits the full lockfile against RustSec. Informational
maintenance warnings for `chrono`, `chrono-tz`, `number_prefix` and `paste`
remain; they are tracked separately from vulnerability findings and are not
suppressed.

Use
[GitHub private vulnerability reporting](https://github.com/zhuisDEV/moon/security/advisories/new)
when available. If disabled, open a public issue requesting a private reporting
channel without including exploit details, credentials or private memory.
