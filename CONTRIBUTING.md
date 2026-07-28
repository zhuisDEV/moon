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

Basic workflow:

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
deno test --node-modules-dir=none \
  --allow-read --allow-write --allow-env --allow-run \
  assets/openclaw-plugin/index.test.ts
```

## Pull Requests

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
4. Update docs when the workflow changes.

## Issues

Bug reports are most useful when they include:

1. platform
2. command run
3. relevant environment variables
4. expected behavior
5. actual behavior
6. logs or error output
