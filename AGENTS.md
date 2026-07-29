# Moon agent instructions

## Runtime safety

- Use an explicit temporary `--home` for development and tests.
- Treat legacy Moon files and rollback bundles as read-only evidence.
- Do not replace the installed binary, live plugin, `~/.moon`, or OpenClaw
  configuration without explicit approval in that turn.
- Back up the live database and configuration before migrations or deployment.

## Engineering

- Keep Moon self-contained: one Rust binary and one SQLite database.
- Do not introduce Node.js, Bun, QMD, a vector server, or another runtime.
- Preserve numbered, transactional database migrations.
- Benchmark retrieval against representative corpus sizes before claiming a
  performance improvement.
- Keep `SKILL.md`, `README.md`, and workflow documentation aligned whenever the
  agent-facing command surface or automatic lifecycle changes.
- Never persist or print provider credentials or arbitrary remote error bodies.
