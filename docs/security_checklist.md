# Security Checklist

1. Never commit real API keys.
2. Keep `.env` and `.env.*` ignored.
3. Use least-privilege API keys for your selected `syns` provider.
4. Treat Moon runtime session data (`raw`, `mlib`, `cleanse`, `memory`) as
   sensitive; set file permissions appropriately.
5. Keep Moon-managed secret/runtime log paths owner-only:
   - `$MOON_HOME/.env`
   - `$MOON_HOME/auth/`
   - `$MOON_HOME/auth/openai-codex.json`
   - `$MOON_HOME/logs/`
   - `$MOON_HOME/logs/audit.log`
   - `$MOON_HOME/logs/distill.audit.log`
6. `moon verify --strict` should fail if those paths drift broader than
   owner-only.
7. Audit logs must not include raw provider response bodies; keep only status
   and request ids when available.
8. Rotate keys immediately if exposure is suspected.
9. Use HTTPS-only model endpoints.
10. CLI diagnostics must mask API keys (`status`, `config --show`,
    `verify`/`status` output).
