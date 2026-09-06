# Release Process

This document describes the release process for Moon.

## Scope

Use this process for tagged public releases (for example `v2.2.0`).

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
   - deterministic unsigned bundle and manifest generation with the commands
     below
   - `deno fmt --check assets/openclaw-plugin tools docs README.md RELEASE.md SKILL.md CHANGELOG.md`
   - `deno lint assets/openclaw-plugin tools`
   - `deno test --node-modules-dir=none --allow-read --allow-write --allow-env --allow-run assets/openclaw-plugin/index.test.ts`
   - an isolated migration and real-binary adapter canary
   - a consistent live backup plus `moon --json health`

## Signed Bundle Inputs

The repository contains a release-operator Rust example that builds the exact
unsigned inputs and signs through either bounded standard input or the offline
macOS recovery keychain. It is not installed with Moon. Ordinary CI uses only
the unsigned commands and cannot access the production signing key.

Build one platform archive and its canonical asset descriptor:

```bash
cargo run --locked --example moon-release -- bundle \
  --binary target/release/moon \
  --minimum-os-version 13.0 \
  --database-schema-min 6 \
  --database-schema-max 7 \
  --output-dir /path/to/release-staging
```

The schema range is intentionally required. Set the minimum to the oldest
installed schema the release can migrate or open, and the maximum to the schema
the release produces. For example, a release that migrates schema 6 to schema 7
must advertise `6..=7`; advertising only `7..=7` would cause the existing
schema-6 updater to reject the release before its transactional migration can
run.

Moon 2.5.1 requires OpenClaw 2026.9.2 for detached model sessions. The bundle
operator defaults `--openclaw-min-version` to that floor; keep it aligned with
the adapter package's `openclaw.compat.minGatewayVersion`. Do not lower the
signed floor without verifying the required host capabilities.

The command executes the candidate's offline JSON version check and refuses
non-release, mismatched, dirty, or unverifiable binaries. `--allow-dirty` exists
only for local development fixtures and must never be used for a published
release. Archive generation runs twice and requires byte-identical output.

After all supported platform asset descriptors have been collected, assemble the
canonical outer manifest:

```bash
cargo run --locked --example moon-release -- manifest \
  --asset /path/to/macos-arm64/release-asset.json \
  --asset /path/to/macos-x64/release-asset.json \
  --asset /path/to/linux-arm64/release-asset.json \
  --asset /path/to/linux-x64/release-asset.json \
  --published-at 2026-08-12T00:00:00Z \
  --output /path/to/release-manifest.json
```

This output is deliberately unsigned. The protected `Release` workflow signs it
only after manual approval of the `production-release` environment. For a local
recovery ceremony, use the Keychain procedure in `docs/release-signing.md`:

```bash
cargo run --locked --example moon-release -- sign \
  --manifest /path/to/release-staging/release-manifest.json \
  --signature-output /path/to/release-staging/release-manifest.sig.json
```

Private signing material must never enter the repository, command arguments,
logs, generated archives, or workflow artifacts. The protected workflow exposes
the GitHub environment secret only to the approved signing step and pipes it to
the signer through standard input. Local recovery signing retrieves it from the
dedicated macOS Keychain and re-locks the Keychain on exit.

Verify the detached result through the exact production trust roots embedded in
Moon:

```bash
cargo run --locked --example moon-release -- verify \
  --manifest /path/to/release-staging/release-manifest.json \
  --signature /path/to/release-staging/release-manifest.sig.json
```

Both CI and the release workflow generate all four native targets: macOS arm64,
macOS x86_64, GNU/Linux arm64, and GNU/Linux x86_64. The aggregate manifest job
must refuse a missing or duplicate target. Ordinary CI publishes only unsigned
workflow artifacts; the release workflow signs only behind the protected
environment.

## Tag and Publish

1. Commit and push release changes to `main`.
2. Create and push an annotated tag:
   - `git tag -a vX.Y.Z -m "moon vX.Y.Z"`
   - `git push origin main --follow-tags`
   - `git push origin vX.Y.Z`
3. Start the workflow at the exact tag:
   - `gh workflow run release.yml --ref vX.Y.Z`
4. Approve the `production-release` deployment after reviewing its tag and
   unsigned build evidence. The workflow creates the release with generated
   notes, the signed canonical manifest, detached signature set, and four
   platform archives. It refuses an existing release.

## Post-release

1. Confirm CI green on release tag.
2. Verify the remote tag and GitHub release target the pushed `main` commit.
3. Smoke test the installed binary, plugin registration, gateway, hybrid recall,
   automatic embedding, and rollback bundle. Confirm that `command -v moon`
   resolves to the same binary configured as OpenClaw's `moonPath`, then check
   `moon --version` and `moon --json health` so a legacy binary cannot shadow
   the release.
4. On an isolated v2.1.0-shaped runtime, perform the bootstrap to the versioned
   layout, then prove a second native update, injected rollback, and interrupted
   journal recovery before promoting the stable release.
