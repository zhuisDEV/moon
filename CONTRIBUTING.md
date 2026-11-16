# Contributing

## Scope

MOON is being kept intentionally simple.

When contributing:

1. Prefer one clear primary flow.
2. Do not add fallback paths unless the maintainers explicitly request them.
3. Remove dead code and duplicate paths instead of preserving them.
4. Keep changes narrow and testable.

## Development

Requirements:

1. Rust stable
2. `cargo`
3. Deno 2 for adapter/developer-tool validation (not a runtime dependency)

Basic workflow:

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo build --locked --release
sh tools/test-openclaw-adapter.sh "$PWD/target/release/moon"
deno test --no-config tools/probe-codex-route.test.ts
deno test --no-config --no-remote tools/source-learning-canary.test.ts
deno fmt --check assets/openclaw-plugin tools docs README.md RELEASE.md SKILL.md CHANGELOG.md
deno lint assets/openclaw-plugin tools
```

## Pull Requests

Use an explicit fresh temporary `--home` and `--database` for manual tests.
Inherited `MOON_DATABASE` can override a home unless the database is supplied.
The adapter test helper creates synthetic data and scopes Deno permissions.
Ordinary CI uses no provider credentials or model calls. The optional
[real-model canary](docs/source-learning-canary.md) is an explicitly authorised
operator check with a bounded usage budget; it is not a replacement for offline
tests.

See [RELEASE.md](RELEASE.md) for clean provenance, four-platform validation and
the protected signed-release process. See
[Getting started](docs/getting-started.md) for user installation rather than
copying build outputs over a live symlink.

Please keep pull requests focused.

Include:

1. what changed
2. why it changed
3. any behavior removed
4. how it was tested

If a change introduces a second path for the same behavior, explain why that
duplication is necessary.

## Style

1. Prefer deletion over compatibility layers when old behavior is no longer part
   of the product direction.
2. Keep runtime behavior explicit.
3. Avoid hidden fallback logic.
4. Update `README.md`, `SKILL.md`, and related workflow docs when behavior
   changes.

## Issues

Bug reports are most useful when they include:

1. platform
2. command run
3. relevant environment variables
4. expected behavior
5. actual behavior
6. logs or error output
