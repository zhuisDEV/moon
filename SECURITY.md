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
  injection allowed. Updating Moon does not grant those permissions. Hook
  contributions contain fenced, untrusted context and do not replace system
  instructions or native transcript ownership.
- Moon's model prompts and outputs pass through OpenClaw and its selected
  provider; a remote model receives the selected text. Local-only retrieval does
  not send memory to a remote provider. Turn evidence and distillation proposals
  passed to the Moon binary use stdin rather than process arguments and are
  bounded.
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
- Exact citations prove source occurrence, not semantic entailment. Automated
  extraction must verify support or request review before promoting a claim.
- Captured sources and generated topics remain untrusted references, with
  separate operator acceptance. URLs are labels and are not fetched by intake.
  Withdrawal invalidates recall support immediately; deletion is logical and
  does not securely erase storage or retained backups. See the
  [lifecycle policy](docs/source-storage.md#approved-lifecycle-policy).
- Schema-13 migrations preserve prior rows and run transactionally. Use the
  installed pre-upgrade binary for backups and retain its matching database.
  Recovery refuses to overwrite learning completed after candidate restart.

## Reporting a vulnerability

The 2.7.0 release uses patched bundled SQLite 3.54.1 and refreshed TLS/QUIC
dependencies. CI audits the full lockfile against RustSec. Informational
maintenance warnings for transitive `number_prefix` and `paste` remain; they are
tracked separately from vulnerability findings and are not suppressed.

Use
[GitHub private vulnerability reporting](https://github.com/zhuisDEV/moon/security/advisories/new)
when available. If disabled, open a public issue requesting a private reporting
channel without including exploit details, credentials or private memory.
