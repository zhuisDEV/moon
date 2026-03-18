# Security Checklist

1. Never commit real API keys.
2. Keep `.env` and `.env.*` ignored.
3. Use least-privilege API keys for your selected `syns` provider.
4. Treat Moon runtime session data (`raw`, `mlib`, `cleanse`, `memory`) as sensitive; set file permissions appropriately.
5. Rotate keys immediately if exposure is suspected.
6. Audit logs must not include secrets.
7. Use HTTPS-only model endpoints.
8. CLI diagnostics must mask API keys (`status`, `config --show`, `verify`/`status` output).
