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
- Prompts and model outputs remain inside the OpenClaw runtime. Turn evidence
  and distillation proposals passed to the Moon binary use stdin rather than
  process arguments and are bounded.
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
