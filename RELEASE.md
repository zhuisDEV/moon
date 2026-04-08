# Release Process

This document describes the release process for MOON.

## Scope

Use this process for tagged public releases (for example `v1.0.0`).

## Pre-release Checklist

1. Ensure version is updated:
   - `Cargo.toml`
   - `assets/plugin/package.json`
   - `assets/plugin/index.js` plugin runtime info
2. Update `CHANGELOG.md` with release date and highlights.
3. Verify docs are aligned:
   - `README.md`
   - `SECURITY.md`
   - `SUPPORT.md`
4. Run validation:
   - `deno fmt --check assets/plugin/index.js assets/plugin/index.test.ts assets/plugin/openclaw.plugin.json`
   - `deno lint assets/plugin/index.js assets/plugin/index.test.ts`
   - `cargo fmt --check`
   - `cargo clippy --all-targets --all-features -- -D warnings`
   - `cargo test --all-targets --all-features`
   - `deno test --allow-read --allow-write --allow-env --allow-run assets/plugin/index.test.ts`

## Tag and Publish

1. Commit release changes to `main`.
2. Create annotated tag:
   - `git tag -a vX.Y.Z -m "moon vX.Y.Z"`
3. Push branch and tags:
   - `git push origin main --follow-tags`
4. Create GitHub release from the tag and include:
   - summary from `CHANGELOG.md`
   - known upgrade notes
   - checksums/artifacts if applicable

## Crate Publishing (Optional)

If this project is published to crates.io:

1. `cargo login` (once per environment)
2. `cargo publish`
3. Add crates.io link to GitHub release notes

## Post-release

1. Confirm CI green on release tag.
2. Smoke test install path:
   - `cargo install --path . --force`
   - `moon install`
   - `moon verify --strict`
3. Open next-cycle tracking issue for follow-up work.
