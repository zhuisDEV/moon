# Release Process

This document describes the release process for Moon.

## Scope

Use this process for tagged public releases (for example `v2.0.0`).

## Pre-release Checklist

1. Ensure version is updated:
   - `Cargo.toml`
   - `assets/openclaw-plugin/package.json`
   - `assets/openclaw-plugin/index.js` plugin runtime info
2. Update `CHANGELOG.md` with release date and highlights.
3. Verify docs are aligned:
   - `README.md`
   - `SKILL.md`
   - `docs/how-it-works.md`
   - `SECURITY.md`
   - `SUPPORT.md`
4. Run validation:
   - `cargo fmt --all -- --check`
   - `cargo clippy --locked --all-targets --all-features -- -D warnings`
   - `cargo test --locked --all-targets --all-features`
   - `cargo build --locked --release`
   - `deno fmt --check assets/openclaw-plugin tools docs README.md SKILL.md CHANGELOG.md`
   - `deno lint assets/openclaw-plugin tools`
   - `deno test --node-modules-dir=none --allow-read --allow-write --allow-env --allow-run assets/openclaw-plugin/index.test.ts`
   - an isolated migration and real-binary adapter canary
   - a consistent live backup plus `moon --json health`

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

## Post-release

1. Confirm CI green on release tag.
2. Verify the remote tag and GitHub release target the pushed `main` commit.
3. Smoke test the installed binary, plugin registration, gateway, hybrid recall,
   automatic embedding, and rollback bundle. Confirm that `command -v moon`
   resolves to the same binary configured as OpenClaw's `moonPath`, then check
   `moon --version` and `moon --json health` so a legacy binary cannot shadow
   the release.
